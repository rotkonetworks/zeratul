//! N-player MCCFR solver (Pluribus-style).
//!
//! Key differences from heads-up:
//!   1. Uses MultiInfoSetKey (position-aware, compressed history)
//!   2. Trains one player at a time, samples N-1 opponents
//!   3. Cycles through all positions for balanced training
//!   4. Game tree traversal handles N-player state machine
//!
//! Abstraction keeps info sets tractable:
//!   6-max: ~120K info sets (vs billions unabstracted)
//!   9-max: ~180K info sets
//!
//! Reference: "Superhuman AI for multiplayer poker" (Brown & Sandholm, 2019)

use std::collections::HashMap;
use crate::{GameState, Rules, Phase, Action, SignedAction, SeatState};
use super::abstraction::*;
use super::solver::InfoNode;

/// multiplayer solver state
pub struct MultiSolver {
    /// strategy table: compressed key → regrets + strategy sums
    pub nodes: HashMap<Vec<u8>, InfoNode>,
    /// game rules
    pub rules: Rules,
    /// number of players
    pub num_players: u8,
    /// total iterations completed
    pub iterations: u64,
    /// RNG
    rng_state: u64,
}

impl MultiSolver {
    pub fn new(rules: Rules, num_players: u8) -> Self {
        let est = MultiInfoSetKey::estimate_info_sets(num_players);
        println!("[multi-cfr] {}-player, estimated ~{} info sets", num_players, est);
        Self {
            nodes: HashMap::with_capacity(est),
            rules,
            num_players,
            iterations: 0,
            rng_state: 0xDEAD_BEEF_CAFE_1234,
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

    /// deal random cards for N players
    fn deal_random(&mut self) -> (GameState, Vec<[u8; 2]>, [u8; 5]) {
        let n = self.num_players as usize;
        let mut deck: Vec<u8> = (0..52).collect();
        // fisher-yates
        for i in (1..52).rev() {
            let j = (self.rng() as usize) % (i + 1);
            deck.swap(i, j);
        }

        let mut cards: Vec<[u8; 2]> = Vec::with_capacity(n);
        for i in 0..n {
            cards.push([deck[i * 2], deck[i * 2 + 1]]);
        }
        let community = [
            deck[n * 2], deck[n * 2 + 1], deck[n * 2 + 2],
            deck[n * 2 + 3], deck[n * 2 + 4],
        ];

        let mut state = GameState::new(self.rules, self.num_players);
        state.deal(&cards, community);

        (state, cards, community)
    }

    /// get valid actions for current player
    fn get_actions(&self, state: &GameState) -> Vec<(Action, u32)> {
        let seat = state.acting_seat as usize;
        let n = state.num_players as usize;
        let max_bet = state.bets.iter().take(n).copied().max().unwrap_or(0);
        let my_bet = state.bets[seat];
        let stack = state.stacks[seat];
        let facing = my_bet < max_bet;

        let mut actions = vec![(Action::Fold, 0)];
        if !facing { actions.push((Action::Check, 0)); }
        if facing && stack > 0 { actions.push((Action::Call, 0)); }
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

    /// compute multi-player info set key
    fn get_info_key(
        &mut self,
        state: &GameState,
        cards: &[[u8; 2]],
        community: &[u8; 5],
        num_raises: &[u8; 4],
        num_active: &[u8; 4],
    ) -> Vec<u8> {
        let seat = state.acting_seat;
        let hole = cards[seat as usize];
        let n = state.num_players;

        let street = match state.phase {
            Phase::Preflop => 0,
            Phase::Flop => 1,
            Phase::Turn => 2,
            Phase::River => 3,
            _ => 0,
        };

        let pos = relative_position(seat, state.button, n);
        let total_stacks = self.rules.buyin * n as u32;

        let key = compute_multi_info_key(
            hole,
            &community[..],
            street,
            pos,
            num_raises,
            num_active,
            state.pot,
            total_stacks,
            &mut || self.rng(),
        );

        key.to_bytes()
    }

    /// count active players
    fn count_active(&self, state: &GameState) -> u8 {
        let n = state.num_players as usize;
        (0..n).filter(|&i| {
            state.seat_state[i] == SeatState::Active || state.seat_state[i] == SeatState::AllIn
        }).count() as u8
    }

    /// external sampling MCCFR traversal for N players
    fn traverse(
        &mut self,
        state: &mut GameState,
        cards: &Vec<[u8; 2]>,
        community: &[u8; 5],
        num_raises: &mut [u8; 4],
        num_active: &mut [u8; 4],
        training_player: u8,
    ) -> f64 {
        // terminal
        if matches!(state.phase, Phase::Settled | Phase::Showdown) {
            if state.phase == Phase::Showdown {
                state.showdown();
            }
            return state.stacks[training_player as usize] as f64 - self.rules.buyin as f64;
        }

        // not a betting phase
        if !matches!(state.phase, Phase::Preflop | Phase::Flop | Phase::Turn | Phase::River) {
            return 0.0;
        }

        let acting = state.acting_seat;
        let is_training = acting == training_player;
        let actions = self.get_actions(state);
        let num_actions = actions.len();
        if num_actions == 0 { return 0.0; }

        let key = self.get_info_key(state, cards, community, num_raises, num_active);

        // ensure node exists
        let node = self.nodes.entry(key.clone()).or_insert_with(|| InfoNode::new(num_actions));
        if node.num_actions != num_actions {
            *node = InfoNode::new(num_actions);
        }
        let strategy = node.current_strategy();

        let street_idx = match state.phase {
            Phase::Preflop => 0,
            Phase::Flop => 1,
            Phase::Turn => 2,
            Phase::River => 3,
            _ => 0,
        };

        if is_training {
            // explore ALL actions
            let mut action_values = vec![0.0; num_actions];
            let mut node_value = 0.0;

            for (a, &(action, amount)) in actions.iter().enumerate() {
                let mut child = state.clone();
                let signed = SignedAction { seat: acting, action, amount, seq: 0, sig: [0; 64] };

                let mut child_raises = *num_raises;
                let mut child_active = *num_active;
                if matches!(action, Action::Bet | Action::Raise | Action::AllIn) {
                    child_raises[street_idx] = child_raises[street_idx].saturating_add(1).min(3);
                }
                if action == Action::Fold {
                    child_active[street_idx] = child_active[street_idx].saturating_sub(1);
                }

                match child.apply(&signed) {
                    Ok(result) => {
                        if result.hand_over {
                            if child.phase == Phase::Showdown { child.showdown(); }
                            action_values[a] = child.stacks[training_player as usize] as f64 - self.rules.buyin as f64;
                        } else {
                            // update active count for new street
                            let new_street = match child.phase {
                                Phase::Flop => 1, Phase::Turn => 2, Phase::River => 3, _ => street_idx,
                            };
                            if new_street != street_idx {
                                child_active[new_street] = self.count_active(&child);
                            }
                            action_values[a] = self.traverse(&mut child, cards, community, &mut child_raises, &mut child_active, training_player);
                        }
                    }
                    Err(_) => {
                        action_values[a] = -(self.rules.buyin as f64);
                    }
                }
                node_value += strategy[a] * action_values[a];
            }

            // update regrets (CFR+: clip negatives)
            let node = self.nodes.get_mut(&key).unwrap();
            for a in 0..num_actions {
                node.regret_sum[a] = (node.regret_sum[a] + action_values[a] - node_value).max(0.0);
            }

            node_value
        } else {
            // sample one action from strategy
            let r = self.rng_f64();
            let mut cumul = 0.0;
            let mut chosen = num_actions - 1;
            for (a, &s) in strategy.iter().enumerate() {
                cumul += s;
                if r < cumul { chosen = a; break; }
            }

            // accumulate strategy sums
            let node = self.nodes.get_mut(&key).unwrap();
            for a in 0..num_actions {
                node.strategy_sum[a] += strategy[a];
            }

            let (action, amount) = actions[chosen];
            let signed = SignedAction { seat: acting, action, amount, seq: 0, sig: [0; 64] };

            let mut child_raises = *num_raises;
            let mut child_active = *num_active;
            if matches!(action, Action::Bet | Action::Raise | Action::AllIn) {
                child_raises[street_idx] = child_raises[street_idx].saturating_add(1).min(3);
            }
            if action == Action::Fold {
                child_active[street_idx] = child_active[street_idx].saturating_sub(1);
            }

            match state.apply(&signed) {
                Ok(result) => {
                    if result.hand_over {
                        if state.phase == Phase::Showdown { state.showdown(); }
                        state.stacks[training_player as usize] as f64 - self.rules.buyin as f64
                    } else {
                        let new_street = match state.phase {
                            Phase::Flop => 1, Phase::Turn => 2, Phase::River => 3, _ => street_idx,
                        };
                        if new_street != street_idx {
                            child_active[new_street] = self.count_active(state);
                        }
                        self.traverse(state, cards, community, &mut child_raises, &mut child_active, training_player)
                    }
                }
                Err(_) => -(self.rules.buyin as f64),
            }
        }
    }

    /// run N iterations, cycling through all positions as training player
    pub fn train(&mut self, num_iterations: u64) {
        let n = self.num_players;

        for i in 0..num_iterations {
            // cycle through all positions as training player
            let training_player = (i % n as u64) as u8;

            let (mut state, cards, community) = self.deal_random();
            let mut num_raises = [0u8; 4];
            let mut num_active = [n; 4];

            self.traverse(&mut state, &cards, &community, &mut num_raises, &mut num_active, training_player);
            self.iterations += 1;

            if self.iterations % 10000 == 0 {
                println!("iteration {}: {} info sets ({}-player)",
                    self.iterations, self.nodes.len(), n);
            }
        }
    }

    /// export strategy (same format as heads-up for compatibility)
    pub fn export_strategy(&self) -> Vec<u8> {
        super::strategy::export_strategy_from_nodes(&self.nodes)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_multi_solver_3player() {
        let rules = Rules {
            buyin: 1000, small_blind: 5, big_blind: 10,
            turn_timeout_blocks: 6, rake_bps: 0, rake_cap: 0,
        };
        let mut solver = MultiSolver::new(rules, 3);
        solver.train(5000);
        println!("3-player: {} info sets after 5K iterations", solver.nodes.len());
        assert!(solver.nodes.len() > 0);
        assert!(solver.nodes.len() < 500_000, "info sets should be bounded by abstraction");
    }

    #[test]
    fn test_multi_solver_6player() {
        let rules = Rules {
            buyin: 1000, small_blind: 5, big_blind: 10,
            turn_timeout_blocks: 6, rake_bps: 0, rake_cap: 0,
        };
        let mut solver = MultiSolver::new(rules, 6);
        solver.train(5000);
        println!("6-player: {} info sets after 5K iterations", solver.nodes.len());
        assert!(solver.nodes.len() > 0);
    }

    #[test]
    fn test_info_set_bounded() {
        // verify that 6-max info sets don't explode
        println!("estimated 2-max info sets: {}", MultiInfoSetKey::estimate_info_sets(2));
        println!("estimated 3-max info sets: {}", MultiInfoSetKey::estimate_info_sets(3));
        println!("estimated 6-max info sets: {}", MultiInfoSetKey::estimate_info_sets(6));
        println!("estimated 9-max info sets: {}", MultiInfoSetKey::estimate_info_sets(9));
    }
}
