//! External Sampling MCCFR solver.
//!
//! On each iteration:
//!   - Deal random cards
//!   - Traverse the game tree for the training player
//!   - At the training player's nodes: explore all actions, update regrets
//!   - At opponent's nodes: sample one action from current strategy
//!   - At chance nodes (deal community cards): sample
//!
//! After many iterations, cumulative regrets converge to Nash equilibrium.
//! The average strategy (from strategy sums) IS the equilibrium.

use std::collections::HashMap;
use crate::{GameState, Rules, Phase, Action, SignedAction, eval_5};
use super::abstraction::*;
use super::strategy::*;

/// MCCFR solver state
pub struct Solver {
    /// strategy table: info set → regrets + strategy sums
    pub nodes: HashMap<InfoSetKey, InfoNode>,
    /// game rules
    pub rules: Rules,
    /// total iterations completed
    pub iterations: u64,
    /// RNG state (xorshift)
    rng_state: u64,
}

/// node in the strategy table
#[derive(Clone, Debug)]
pub struct InfoNode {
    /// cumulative regret per action
    pub regret_sum: Vec<f64>,
    /// cumulative strategy per action (for averaging)
    pub strategy_sum: Vec<f64>,
    /// number of actions available
    pub num_actions: usize,
}

impl InfoNode {
    pub fn new(num_actions: usize) -> Self {
        Self {
            regret_sum: vec![0.0; num_actions],
            strategy_sum: vec![0.0; num_actions],
            num_actions,
        }
    }

    /// compute current strategy from regrets (regret matching)
    pub fn current_strategy(&self) -> Vec<f64> {
        let mut strategy = vec![0.0; self.num_actions];
        let positive_sum: f64 = self.regret_sum.iter().map(|&r| r.max(0.0)).sum();

        if positive_sum > 0.0 {
            for i in 0..self.num_actions {
                strategy[i] = self.regret_sum[i].max(0.0) / positive_sum;
            }
        } else {
            // uniform if no positive regrets
            let uniform = 1.0 / self.num_actions as f64;
            for s in strategy.iter_mut() { *s = uniform; }
        }
        strategy
    }

    /// compute average strategy (the actual Nash equilibrium output)
    pub fn average_strategy(&self) -> Vec<f64> {
        let total: f64 = self.strategy_sum.iter().sum();
        if total > 0.0 {
            self.strategy_sum.iter().map(|&s| s / total).collect()
        } else {
            vec![1.0 / self.num_actions as f64; self.num_actions]
        }
    }
}

impl Solver {
    pub fn new(rules: Rules) -> Self {
        Self {
            nodes: HashMap::new(),
            rules,
            iterations: 0,
            rng_state: 0xdeadbeefcafe1234,
        }
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

    /// deal a random hand: shuffle deck, assign cards
    fn deal_random(&mut self) -> (GameState, [[u8; 2]; 2], [u8; 5]) {
        let mut deck: Vec<u8> = (0..52).collect();
        // fisher-yates
        for i in (1..52).rev() {
            let j = (self.rng() as usize) % (i + 1);
            deck.swap(i, j);
        }

        let cards = [[deck[0], deck[1]], [deck[2], deck[3]]];
        let community = [deck[4], deck[5], deck[6], deck[7], deck[8]];

        let mut state = GameState::new(self.rules, 2);
        state.deal(&cards, community);

        (state, cards, community)
    }

    /// get valid actions for current player
    fn get_actions(&self, state: &GameState) -> Vec<(Action, u32)> {
        let seat = state.acting_seat as usize;
        let max_bet = state.bets.iter().take(2).copied().max().unwrap_or(0);
        let my_bet = state.bets[seat];
        let stack = state.stacks[seat];
        let facing_bet = my_bet < max_bet;

        let mut actions = vec![(Action::Fold, 0)];

        if !facing_bet {
            actions.push((Action::Check, 0));
        }
        if facing_bet && stack > 0 {
            actions.push((Action::Call, 0));
        }
        // pot-sized bet
        let pot_bet = state.pot.min(stack);
        if pot_bet >= self.rules.big_blind && stack >= pot_bet {
            actions.push((Action::Bet, pot_bet));
        }
        // all-in
        if stack > 0 && (actions.len() < 3 || stack != pot_bet) {
            actions.push((Action::AllIn, 0));
        }

        actions
    }

    /// get information set key for current player
    fn get_info_key(&mut self, state: &GameState, cards: &[[u8; 2]; 2], community: &[u8; 5], history: &[u8]) -> InfoSetKey {
        let seat = state.acting_seat as usize;
        let hole = cards[seat];

        let hand_bucket = match state.phase {
            Phase::Preflop => preflop_bucket(hole[0], hole[1]),
            #[cfg(feature = "gpu")]
            Phase::Flop => hand_strength_bucket_gpu(hole, &community[..3], FLOP_BUCKETS),
            #[cfg(feature = "gpu")]
            Phase::Turn => hand_strength_bucket_gpu(hole, &community[..4], TURN_BUCKETS),
            #[cfg(feature = "gpu")]
            Phase::River => hand_strength_bucket_gpu(hole, &community[..5], RIVER_BUCKETS),
            #[cfg(not(feature = "gpu"))]
            Phase::Flop => {
                let board = &community[..3];
                hand_strength_bucket(hole, board, FLOP_BUCKETS, 200, &mut || self.rng())
            }
            #[cfg(not(feature = "gpu"))]
            Phase::Turn => {
                let board = &community[..4];
                hand_strength_bucket(hole, board, TURN_BUCKETS, 200, &mut || self.rng())
            }
            #[cfg(not(feature = "gpu"))]
            Phase::River => {
                let board = &community[..5];
                hand_strength_bucket(hole, board, RIVER_BUCKETS, 300, &mut || self.rng())
            }
            _ => 0,
        };

        let street = match state.phase {
            Phase::Preflop => 0,
            Phase::Flop => 1,
            Phase::Turn => 2,
            Phase::River => 3,
            _ => 4,
        };

        InfoSetKey {
            hand_bucket,
            history: history.to_vec(),
            street,
        }
    }

    /// external sampling MCCFR traversal
    fn traverse(
        &mut self,
        state: &mut GameState,
        cards: &[[u8; 2]; 2],
        community: &[u8; 5],
        history: &mut Vec<u8>,
        training_player: u8,
    ) -> f64 {
        // terminal node
        if state.phase == Phase::Settled || state.phase == Phase::Showdown {
            if state.phase == Phase::Showdown {
                state.showdown();
            }
            // utility for training player
            let initial_stack = self.rules.buyin as f64;
            return state.stacks[training_player as usize] as f64 - initial_stack;
        }

        // check if need phase advance (both checked/called)
        let acting = state.acting_seat;
        let is_training = acting == training_player;
        let actions = self.get_actions(state);
        let num_actions = actions.len();

        if num_actions == 0 { return 0.0; }

        let key = self.get_info_key(state, cards, community, history);

        // ensure node exists with correct action count
        let node = self.nodes.entry(key.clone()).or_insert_with(|| InfoNode::new(num_actions));
        // if action count changed (different game state mapped to same key), resize
        if node.num_actions != num_actions {
            *node = InfoNode::new(num_actions);
        }

        let strategy = node.current_strategy();

        if is_training {
            // explore ALL actions, compute counterfactual values
            let mut action_values = vec![0.0; num_actions];
            let mut node_value = 0.0;

            for (a, &(action, amount)) in actions.iter().enumerate() {
                let mut child_state = state.clone();
                let signed = SignedAction { seat: acting, action, amount, seq: 0, sig: [0; 64] };

                match child_state.apply(&signed) {
                    Ok(_) => {
                        history.push(abstract_action(action, amount, state.pot, state.stacks[acting as usize]));
                        action_values[a] = self.traverse(&mut child_state, cards, community, history, training_player);
                        history.pop();
                    }
                    Err(_) => {
                        action_values[a] = -(self.rules.buyin as f64); // invalid = worst outcome
                    }
                }

                node_value += strategy[a] * action_values[a];
            }

            // update regrets
            let node = self.nodes.get_mut(&key).unwrap();
            for a in 0..num_actions {
                node.regret_sum[a] += action_values[a] - node_value;
            }

            node_value
        } else {
            // sample one action from strategy
            let r = self.rng_f64();
            let mut cumulative = 0.0;
            let mut chosen = num_actions - 1;
            for (a, &s) in strategy.iter().enumerate() {
                cumulative += s;
                if r < cumulative {
                    chosen = a;
                    break;
                }
            }

            // accumulate strategy sums for average strategy
            let node = self.nodes.get_mut(&key).unwrap();
            for a in 0..num_actions {
                node.strategy_sum[a] += strategy[a];
            }

            let (action, amount) = actions[chosen];
            let signed = SignedAction { seat: acting, action, amount, seq: 0, sig: [0; 64] };

            match state.apply(&signed) {
                Ok(_) => {
                    history.push(abstract_action(action, amount, state.pot, state.stacks[acting as usize]));
                    let v = self.traverse(state, cards, community, history, training_player);
                    history.pop();
                    v
                }
                Err(_) => -(self.rules.buyin as f64),
            }
        }
    }

    /// run N iterations of MCCFR
    pub fn train(&mut self, num_iterations: u64) {
        for i in 0..num_iterations {
            // alternate training player
            let training_player = (i % 2) as u8;

            let (mut state, cards, community) = self.deal_random();
            let mut history = Vec::new();

            self.traverse(&mut state, &cards, &community, &mut history, training_player);
            self.iterations += 1;

            if (i + 1) % 10000 == 0 {
                println!("iteration {}: {} info sets", i + 1, self.nodes.len());
            }
        }
    }

    /// exploitability estimate (how far from Nash)
    pub fn exploitability_estimate(&self) -> f64 {
        // rough estimate: average max regret across all nodes
        if self.nodes.is_empty() { return f64::MAX; }
        let total_regret: f64 = self.nodes.values()
            .map(|n| n.regret_sum.iter().cloned().fold(0.0f64, f64::max).max(0.0))
            .sum();
        total_regret / self.nodes.len() as f64
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_solver_runs() {
        let mut solver = Solver::new(Rules::default());
        solver.train(1000);
        println!("info sets: {}", solver.nodes.len());
        println!("exploitability: {:.4}", solver.exploitability_estimate());
        assert!(solver.nodes.len() > 0);
    }

    #[test]
    fn test_solver_converges() {
        let mut solver = Solver::new(Rules::default());
        solver.train(50_000);
        let exploit = solver.exploitability_estimate();
        println!("50K iterations: {} info sets, exploitability: {:.4}", solver.nodes.len(), exploit);
        // after 50K iterations, exploitability should decrease
        assert!(solver.nodes.len() > 100);
    }
}
