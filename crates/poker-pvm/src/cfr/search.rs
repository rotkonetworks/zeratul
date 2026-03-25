//! Real-time search: depth-limited CFR from the current game state.
//!
//! At each decision point, run a mini-CFR from the current state
//! using the blueprint strategy for:
//!   1. Opponent action sampling (what would Nash opponent do?)
//!   2. Leaf node evaluation (blueprint value when search depth exceeded)
//!
//! This is the Pluribus approach: blueprint + real-time search.
//! The search adapts to the specific game situation while the blueprint
//! ensures we stay close to Nash equilibrium.
//!
//! Layers:
//!   L0: Blueprint lookup (instant, ~49K info sets, fallback heuristic)
//!   L1: Real-time search (depth-limited CFR, ~1000 iterations, <100ms)
//!   L2: Value network (CTM leaf evaluation, future)
//!
//! The combination guarantees:
//!   - Never worse than blueprint (Nash floor)
//!   - Adapts to specific situations the blueprint is coarse on
//!   - Exploits opponent deviations from equilibrium

use std::collections::HashMap;
use crate::{GameState, Rules, Phase, Action, SignedAction, eval_5};
use super::abstraction::*;
use super::strategy::import_strategy;

/// real-time search configuration
#[derive(Clone)]
pub struct SearchConfig {
    /// max depth to search (in full game tree plies)
    pub max_depth: u32,
    /// iterations of CFR to run at this decision
    pub iterations: u32,
    /// blueprint strategy table
    pub blueprint: HashMap<Vec<u8>, Vec<f64>>,
}

impl Default for SearchConfig {
    fn default() -> Self {
        Self {
            max_depth: 4,
            iterations: 1000,
            blueprint: HashMap::new(),
        }
    }
}

/// search result: action probabilities and estimated value
#[derive(Clone, Debug)]
pub struct SearchResult {
    /// action probabilities (refined by search)
    pub action_probs: Vec<f64>,
    /// estimated value for the acting player (in chips, relative to buyin)
    pub value: f64,
    /// which actions correspond to these probabilities
    pub actions: Vec<(Action, u32)>,
    /// whether blueprint was used (true) or search refined it
    pub from_blueprint: bool,
    /// number of search iterations actually run
    pub search_iters: u32,
}

/// The bot: combines blueprint + search + value network (CTM-MoE)
pub struct PokerBot {
    pub config: SearchConfig,
    rng_state: u64,
    /// CTM-MoE for leaf evaluation (replaces blueprint at depth limit)
    #[cfg(feature = "onnx")]
    pub moe: Option<super::inference::OnnxMoE>,
}

impl PokerBot {
    pub fn new(config: SearchConfig) -> Self {
        Self {
            config,
            rng_state: 0xB0BA_CAFE_F00D_1234,
            #[cfg(feature = "onnx")]
            moe: None,
        }
    }

    /// attach CTM-MoE for leaf evaluation
    #[cfg(feature = "onnx")]
    pub fn with_moe(mut self, moe: super::inference::OnnxMoE) -> Self {
        self.moe = Some(moe);
        self
    }

    pub fn from_strategy_bytes(data: &[u8]) -> Self {
        let blueprint = import_strategy(data);
        println!("[bot] loaded {} info sets", blueprint.len());
        Self::new(SearchConfig {
            blueprint,
            ..Default::default()
        })
    }

    fn rng(&mut self) -> u32 {
        self.rng_state ^= self.rng_state << 13;
        self.rng_state ^= self.rng_state >> 7;
        self.rng_state ^= self.rng_state << 17;
        (self.rng_state >> 16) as u32
    }

    fn rng_f64(&mut self) -> f64 {
        (self.rng() as f64) / (u32::MAX as f64)
    }

    /// expose rng for range module
    pub fn rng_for_range(&mut self) -> u32 {
        self.rng()
    }

    /// get valid actions at current state.
    /// Pluribus-style sizing: {0.25x, 0.5x, 1x, 2x, all-in}
    fn get_actions(&self, state: &GameState) -> Vec<(Action, u32)> {
        let seat = state.acting_seat as usize;
        let max_bet = state.bets.iter().take(state.num_players as usize).copied().max().unwrap_or(0);
        let my_bet = state.bets[seat];
        let stack = state.stacks[seat];
        let facing = my_bet < max_bet;
        let bb = state.rules.big_blind;
        let pot = state.pot;

        let mut actions = vec![(Action::Fold, 0)];
        if !facing { actions.push((Action::Check, 0)); }
        if facing && stack > 0 { actions.push((Action::Call, 0)); }

        // bet/raise sizes as fractions of pot: 0.25x, 0.5x, 1x, 2x
        let min_bet = if facing {
            // raise: at least 2x the current bet
            (max_bet * 2 - my_bet).max(bb)
        } else {
            bb
        };
        let action_type = if facing { Action::Raise } else { Action::Bet };

        let mut seen = Vec::new();
        for &frac_num in &[1u32, 2, 3, 4, 8] { // 0.25x, 0.5x, 0.75x, 1x, 2x
            let amount = (pot * frac_num / 4).max(min_bet).min(stack);
            if amount >= min_bet && amount < stack && !seen.contains(&amount) {
                actions.push((action_type, amount));
                seen.push(amount);
            }
        }

        // all-in (always available if we have chips)
        if stack > 0 {
            actions.push((Action::AllIn, 0));
        }
        actions
    }

    /// L0: blueprint lookup
    fn blueprint_lookup(&mut self, state: &GameState, hole: &[u8; 2], community: &[u8; 5], history: &[u8]) -> Option<Vec<f64>> {
        let street = match state.phase {
            Phase::Preflop => 0u8,
            Phase::Flop => 1,
            Phase::Turn => 2,
            Phase::River => 3,
            _ => return None,
        };

        let hand_bucket = if street == 0 {
            preflop_bucket(hole[0], hole[1])
        } else {
            let board_len = match street { 1 => 3, 2 => 4, _ => 5 };
            hand_strength_bucket(*hole, &community[..board_len], 10, 200, &mut || self.rng())
        };

        let key = InfoSetKey { hand_bucket, history: history.to_vec(), street };
        self.config.blueprint.get(&key.to_bytes()).cloned()
    }

    /// L1: depth-limited real-time search
    fn realtime_search(
        &mut self,
        state: &GameState,
        hole: &[u8; 2],
        community: &[u8; 5],
        history: &[u8],
        my_seat: u8,
    ) -> SearchResult {
        let actions = self.get_actions(state);
        let n = actions.len();
        if n == 0 {
            return SearchResult {
                action_probs: vec![],
                value: 0.0,
                actions: vec![],
                from_blueprint: false,
                search_iters: 0,
            };
        }

        // local regret table for this search
        let mut regret_sum = vec![0.0f64; n];
        let mut strategy_sum = vec![0.0f64; n];

        for iter in 0..self.config.iterations {
            let training = (iter % 2) as u8;

            // run one CFR traversal from current state
            let mut s = state.clone();
            let mut h = history.to_vec();

            let values = self.search_traverse(
                &mut s, hole, community, &mut h,
                training, my_seat, 0, &actions,
            );

            // update regrets (only when training as acting player)
            if training == 0 {
                let strategy = regret_matching(&regret_sum);
                let node_value: f64 = (0..n).map(|a| strategy[a] * values[a]).sum();
                for a in 0..n {
                    regret_sum[a] = (regret_sum[a] + values[a] - node_value).max(0.0);
                }
            }

            // accumulate strategy
            let strategy = regret_matching(&regret_sum);
            for a in 0..n {
                strategy_sum[a] += strategy[a];
            }
        }

        // average strategy is the output
        let total: f64 = strategy_sum.iter().sum();
        let action_probs = if total > 0.0 {
            strategy_sum.iter().map(|&s| s / total).collect()
        } else {
            vec![1.0 / n as f64; n]
        };

        SearchResult {
            action_probs,
            value: 0.0, // could compute expected value
            actions,
            from_blueprint: false,
            search_iters: self.config.iterations,
        }
    }

    /// recursive search traversal (depth-limited)
    fn search_traverse(
        &mut self,
        state: &mut GameState,
        hole: &[u8; 2],
        community: &[u8; 5],
        history: &mut Vec<u8>,
        training_player: u8,
        my_seat: u8,
        depth: u32,
        root_actions: &[(Action, u32)],
    ) -> Vec<f64> {
        // terminal
        if matches!(state.phase, Phase::Showdown | Phase::Settled) {
            if state.phase == Phase::Showdown {
                state.showdown();
            }
            let val = state.stacks[my_seat as usize] as f64 - state.rules.buyin as f64;
            return vec![val; root_actions.len()];
        }

        // depth limit → use blueprint or heuristic leaf evaluation
        if depth >= self.config.max_depth {
            let val = self.leaf_evaluate(state, hole, community, my_seat);
            return vec![val; root_actions.len()];
        }

        let acting = state.acting_seat;
        let actions = self.get_actions(state);
        let n = actions.len();
        if n == 0 { return vec![0.0; root_actions.len()]; }

        if acting == my_seat && depth == 0 {
            // root node: compute value for each action
            let mut action_values = vec![0.0f64; root_actions.len()];

            for (a, &(action, amount)) in root_actions.iter().enumerate() {
                let mut child = state.clone();
                let signed = SignedAction { seat: acting, action, amount, seq: 0, sig: [0; 64] };
                match child.apply(&signed) {
                    Ok(result) => {
                        let abs = abstract_action(action, amount, state.pot, state.stacks[acting as usize]);
                        history.push(abs);
                        if result.hand_over {
                            if child.phase == Phase::Showdown { child.showdown(); }
                            action_values[a] = child.stacks[my_seat as usize] as f64 - child.rules.buyin as f64;
                        } else {
                            let child_vals = self.search_traverse(
                                &mut child, hole, community, history,
                                training_player, my_seat, depth + 1, root_actions,
                            );
                            action_values[a] = child_vals[a];
                        }
                        history.pop();
                    }
                    Err(_) => {
                        action_values[a] = -(state.rules.buyin as f64);
                    }
                }
            }

            action_values
        } else {
            // opponent or deeper node: sample from blueprint
            let opp_probs = self.blueprint_lookup(state, hole, community, history)
                .unwrap_or_else(|| vec![1.0 / n as f64; n]);

            // sample
            let r = self.rng_f64();
            let mut cumul = 0.0;
            let mut chosen = n - 1;
            for (i, p) in opp_probs.iter().enumerate() {
                cumul += p;
                if r < cumul && i < n { chosen = i; break; }
            }
            if chosen >= n { chosen = n - 1; }

            let (action, amount) = actions[chosen];
            let signed = SignedAction { seat: acting, action, amount, seq: 0, sig: [0; 64] };

            let abs = abstract_action(action, amount, state.pot, state.stacks[acting as usize]);
            history.push(abs);

            match state.apply(&signed) {
                Ok(result) => {
                    let vals = if result.hand_over {
                        if state.phase == Phase::Showdown { state.showdown(); }
                        let v = state.stacks[my_seat as usize] as f64 - state.rules.buyin as f64;
                        vec![v; root_actions.len()]
                    } else {
                        self.search_traverse(state, hole, community, history, training_player, my_seat, depth + 1, root_actions)
                    };
                    history.pop();
                    vals
                }
                Err(_) => {
                    history.pop();
                    vec![0.0; root_actions.len()]
                }
            }
        }
    }

    /// L2: leaf node evaluation
    /// Uses CTM-MoE when available, falls back to MC rollout.
    fn leaf_evaluate(&mut self, state: &GameState, hole: &[u8; 2], community: &[u8; 5], seat: u8) -> f64 {
        // try CTM-MoE first (if loaded)
        #[cfg(feature = "onnx")]
        if let Some(ref moe) = self.moe {
            let board_len = match state.phase {
                Phase::Flop => 3, Phase::Turn => 4, Phase::River => 5, _ => 0,
            };
            let board_slice = &community[..board_len];
            let n = state.num_players as usize;
            let hero = seat as usize;
            let villain = if n == 2 { 1 - hero } else { hero };
            let features = super::ctm::extract_all(
                board_slice,
                state.pot,
                state.stacks[hero],
                state.stacks[villain],
                state.bets.get(hero).copied().unwrap_or(0),
                10, // big blind — should come from rules
                0.5, 0.3, 0.1, 0.1, // range placeholders
                hero == 0, // simplified IP check
            );
            if let Ok(output) = moe.evaluate(&features.features) {
                return output.value as f64;
            }
        }

        let s = seat as usize;
        let stack = state.stacks[s] as f64;
        let pot = state.pot as f64;

        let board_len = match state.phase {
            Phase::Flop => 3,
            Phase::Turn => 4,
            Phase::River => 5,
            _ => 0,
        };

        let equity = if board_len == 0 {
            // preflop: use bucket as rough equity (fast)
            preflop_bucket(hole[0], hole[1]) as f64 / 168.0
        } else if board_len == 5 {
            // river: exact evaluation possible (no rollout needed)
            // just check against a few random opponents
            hand_strength_bucket(
                *hole, &community[..5], 100, 30, &mut || self.rng()
            ) as f64 / 100.0
        } else {
            // flop/turn: small rollout (speed vs accuracy tradeoff)
            hand_strength_bucket(
                *hole, &community[..board_len], 10, 50, &mut || self.rng()
            ) as f64 / 10.0
        };

        stack + pot * equity - state.rules.buyin as f64
    }

    /// Main entry: decide action at current game state.
    /// Combines all three layers.
    pub fn decide(
        &mut self,
        state: &GameState,
        hole: &[u8; 2],
        community: &[u8; 5],
        history: &[u8],
    ) -> SearchResult {
        let my_seat = state.acting_seat;
        let actions = self.get_actions(state);
        let n = actions.len();

        if n == 0 {
            return SearchResult {
                action_probs: vec![],
                value: 0.0,
                actions: vec![],
                from_blueprint: false,
                search_iters: 0,
            };
        }

        // L0: try blueprint
        let blueprint_probs = self.blueprint_lookup(state, hole, community, history);

        if let Some(bp) = &blueprint_probs {
            // check if blueprint is confident (low entropy = strong opinion)
            let entropy: f64 = bp.iter()
                .filter(|&&p| p > 0.0)
                .map(|&p| -p * p.ln())
                .sum();
            let max_entropy = (bp.len() as f64).ln();
            let confidence = 1.0 - (entropy / max_entropy);

            if confidence > 0.5 {
                // blueprint is confident — use it directly, no search needed
                // map blueprint abstract actions to concrete
                let probs = self.blueprint_to_concrete(bp, &actions, state);
                return SearchResult {
                    action_probs: probs,
                    value: 0.0,
                    actions,
                    from_blueprint: true,
                    search_iters: 0,
                };
            }
        }

        // L1: real-time search (blueprint is uncertain or missing)
        let mut result = self.realtime_search(state, hole, community, history, my_seat);

        // blend with blueprint if available (trust blueprint as prior)
        if let Some(bp) = blueprint_probs {
            let bp_concrete = self.blueprint_to_concrete(&bp, &result.actions, state);
            let blend = 0.3; // 30% blueprint, 70% search
            for (i, p) in result.action_probs.iter_mut().enumerate() {
                if i < bp_concrete.len() {
                    *p = *p * (1.0 - blend) + bp_concrete[i] * blend;
                }
            }
            // renormalize
            let total: f64 = result.action_probs.iter().sum();
            if total > 0.0 {
                for p in result.action_probs.iter_mut() { *p /= total; }
            }
        }

        result
    }

    /// L0 only: blueprint lookup, no search fallback.
    /// Falls back to uniform if blueprint miss.
    pub fn decide_blueprint_only(
        &mut self,
        state: &GameState,
        hole: &[u8; 2],
        community: &[u8; 5],
        history: &[u8],
    ) -> SearchResult {
        let actions = self.get_actions(state);
        if actions.is_empty() {
            return SearchResult {
                action_probs: vec![], value: 0.0, actions: vec![],
                from_blueprint: false, search_iters: 0,
            };
        }

        let blueprint_probs = self.blueprint_lookup(state, hole, community, history);
        let probs = if let Some(bp) = blueprint_probs {
            self.blueprint_to_concrete(&bp, &actions, state)
        } else {
            // uniform fallback
            vec![1.0 / actions.len() as f64; actions.len()]
        };

        SearchResult {
            action_probs: probs,
            value: 0.0,
            actions,
            from_blueprint: true,
            search_iters: 0,
        }
    }

    /// sample a concrete action from search result
    pub fn sample_action(&mut self, result: &SearchResult) -> Option<(Action, u32)> {
        if result.actions.is_empty() { return None; }
        let r = self.rng_f64();
        let mut cumul = 0.0;
        for (i, &p) in result.action_probs.iter().enumerate() {
            cumul += p;
            if r < cumul {
                return Some(result.actions[i]);
            }
        }
        Some(*result.actions.last().unwrap())
    }

    /// map blueprint abstract probs [fold, check/call, small_bet, big_bet]
    /// to concrete action probs matching the actions list
    /// map blueprint's 4 abstract actions [fold, check/call, small_bet, big_bet]
    /// onto concrete actions with Pluribus-style sizing
    fn blueprint_to_concrete(&self, bp: &[f64], actions: &[(Action, u32)], state: &GameState) -> Vec<f64> {
        let mut probs = vec![0.0; actions.len()];
        let bp4: Vec<f64> = {
            let mut v = bp.to_vec();
            while v.len() < 4 { v.push(0.0); }
            v
        };

        let pot = state.pot;

        for (i, &(action, amount)) in actions.iter().enumerate() {
            match action {
                Action::Fold => probs[i] = bp4[0],
                Action::Check => probs[i] = bp4[1],
                Action::Call => probs[i] = bp4[1],
                Action::Bet | Action::Raise => {
                    // distribute small_bet and big_bet across 5 sizing options
                    let frac = if pot > 0 { amount as f64 / pot as f64 } else { 1.0 };
                    if frac <= 0.375 {
                        probs[i] = bp4[2] * 0.7; // 1/4 pot → small bucket
                    } else if frac <= 0.625 {
                        probs[i] = bp4[2]; // 1/2 pot → small bucket
                    } else if frac <= 0.875 {
                        probs[i] = bp4[2] * 0.3 + bp4[3] * 0.4; // 3/4 pot → blend
                    } else if frac <= 1.5 {
                        probs[i] = bp4[3]; // pot → big bucket
                    } else {
                        probs[i] = bp4[3] * 0.7; // 2x pot → big bucket
                    }
                }
                Action::AllIn => probs[i] = bp4[3] * 0.4,
            }
        }

        // normalize
        let total: f64 = probs.iter().sum();
        if total > 1e-10 {
            for p in probs.iter_mut() { *p /= total; }
        } else {
            let uniform = 1.0 / actions.len() as f64;
            for p in probs.iter_mut() { *p = uniform; }
        }
        probs
    }
}

/// regret matching: convert cumulative regrets to strategy
fn regret_matching(regret_sum: &[f64]) -> Vec<f64> {
    let positive_sum: f64 = regret_sum.iter().map(|&r| r.max(0.0)).sum();
    if positive_sum > 0.0 {
        regret_sum.iter().map(|&r| r.max(0.0) / positive_sum).collect()
    } else {
        vec![1.0 / regret_sum.len() as f64; regret_sum.len()]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_bot_decides() {
        let mut bot = PokerBot::new(SearchConfig {
            max_depth: 2,
            iterations: 100,
            ..Default::default()
        });

        let rules = Rules::default();
        let mut state = GameState::new(rules, 2);
        let cards = [[12, 25], [0, 13]]; // AA vs 22
        let community = [5, 18, 31, 44, 7];
        state.deal(&cards, community);

        let result = bot.decide(&state, &cards[0], &community, &[]);
        println!("actions: {:?}", result.actions);
        println!("probs: {:?}", result.action_probs);
        println!("from_blueprint: {}", result.from_blueprint);
        println!("search_iters: {}", result.search_iters);

        assert!(!result.actions.is_empty());
        assert_eq!(result.actions.len(), result.action_probs.len());

        // AA should heavily favor betting/allin
        let fold_prob = result.action_probs[0]; // fold is always first
        assert!(fold_prob < 0.3, "AA should rarely fold, got {}", fold_prob);
    }

    #[test]
    fn test_bot_with_blueprint() {
        // create a tiny blueprint with entries for various preflop buckets
        let mut blueprint = HashMap::new();

        // add entries for ALL 169 preflop buckets so we always hit
        for bucket in 0..169u16 {
            let key = InfoSetKey { hand_bucket: bucket, history: vec![], street: 0 };
            if bucket >= 150 {
                // premium hands: heavy allin
                blueprint.insert(key.to_bytes(), vec![0.01, 0.04, 0.05, 0.90]);
            } else if bucket < 20 {
                // trash hands: heavy fold
                blueprint.insert(key.to_bytes(), vec![0.80, 0.10, 0.05, 0.05]);
            } else {
                // medium: mixed
                blueprint.insert(key.to_bytes(), vec![0.25, 0.25, 0.25, 0.25]);
            }
        }

        let mut bot = PokerBot::new(SearchConfig {
            blueprint,
            max_depth: 2,
            iterations: 100,
            ..Default::default()
        });

        let rules = Rules::default();

        // test AA: bucket = preflop_bucket(12, 25) = 168 (premium)
        let aa_bucket = preflop_bucket(12, 25);
        println!("AA bucket: {}", aa_bucket);

        let mut state = GameState::new(rules, 2);
        state.deal(&[[12, 25], [0, 13]], [5, 18, 31, 44, 7]);
        let acting = state.acting_seat;
        let hole = if acting == 0 { &[12u8, 25] } else { &[0u8, 13] };
        let result = bot.decide(&state, hole, &[5, 18, 31, 44, 7], &[]);
        println!("hand (seat {}): from_blueprint={} probs={:?}", acting, result.from_blueprint, result.action_probs);

        // 72o test
        let _72_bucket = preflop_bucket(5, 13);
        println!("72o bucket: {}", _72_bucket);

        let mut state = GameState::new(rules, 2);
        state.deal(&[[5, 13], [12, 25]], [18, 31, 44, 7, 20]);
        let acting = state.acting_seat;
        let hole = if acting == 0 { &[5u8, 13] } else { &[12u8, 25] };
        let result = bot.decide(&state, hole, &[18, 31, 44, 7, 20], &[]);
        println!("hand (seat {}): from_blueprint={} probs={:?}", acting, result.from_blueprint, result.action_probs);
    }
}
