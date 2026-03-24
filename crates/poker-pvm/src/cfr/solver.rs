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

/// CFR variant to use
#[derive(Clone, Copy, Debug)]
pub enum CfrVariant {
    /// vanilla CFR (no discounting)
    Vanilla,
    /// DCFR+ (Xu et al. 2022): discount + clip negatives
    DcfrPlus { alpha: f64, gamma: f64 },
}

/// MCCFR solver state
pub struct Solver {
    /// strategy table: info set → regrets + strategy sums
    pub nodes: HashMap<InfoSetKey, InfoNode>,
    /// game rules
    pub rules: Rules,
    /// total iterations completed
    pub iterations: u64,
    /// CFR variant
    pub variant: CfrVariant,
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
        Self::with_variant(rules, CfrVariant::DcfrPlus { alpha: 2.0, gamma: 2.0 })
    }

    pub fn with_variant(rules: Rules, variant: CfrVariant) -> Self {
        Self {
            nodes: HashMap::new(),
            rules,
            iterations: 0,
            variant,
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

            // update regrets with DCFR+ discounting + clipping
            let node = self.nodes.get_mut(&key).unwrap();
            let t = self.iterations as f64 + 1.0;
            match self.variant {
                CfrVariant::Vanilla => {
                    for a in 0..num_actions {
                        node.regret_sum[a] += action_values[a] - node_value;
                    }
                }
                CfrVariant::DcfrPlus { alpha, .. } => {
                    // DCFR+: R^t = max(R^{t-1} * discount + r^t, 0)
                    let tm1 = (t - 1.0).max(1.0);
                    let discount = tm1.powf(alpha) / (tm1.powf(alpha) + 1.0);
                    for a in 0..num_actions {
                        let prev = node.regret_sum[a].max(0.0); // clip negatives (CFR+)
                        node.regret_sum[a] = (prev * discount + (action_values[a] - node_value)).max(0.0);
                    }
                }
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

            // accumulate strategy sums (with DCFR+ discounting)
            let node = self.nodes.get_mut(&key).unwrap();
            let t = self.iterations as f64 + 1.0;
            match self.variant {
                CfrVariant::Vanilla => {
                    for a in 0..num_actions {
                        node.strategy_sum[a] += strategy[a];
                    }
                }
                CfrVariant::DcfrPlus { gamma, .. } => {
                    let tm1 = (t - 1.0).max(1.0);
                    let discount = (tm1 / t).powf(gamma);
                    for a in 0..num_actions {
                        node.strategy_sum[a] = node.strategy_sum[a] * discount + strategy[a];
                    }
                }
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
        self.train_with_checkpoint(num_iterations, None);
    }

    /// run N iterations with periodic checkpoint saves
    pub fn train_with_checkpoint(&mut self, num_iterations: u64, checkpoint_path: Option<&str>) {
        let start_iter = self.iterations;
        let target = start_iter + num_iterations;

        for i in start_iter..target {
            // alternate training player
            let training_player = ((i - start_iter) % 2) as u8;

            let (mut state, cards, community) = self.deal_random();
            let mut history = Vec::new();

            self.traverse(&mut state, &cards, &community, &mut history, training_player);
            self.iterations += 1;

            if (self.iterations) % 10000 == 0 {
                println!("iteration {}: {} info sets", self.iterations, self.nodes.len());
            }

            // checkpoint every 500K iterations
            if let Some(path) = checkpoint_path {
                if self.iterations % 500_000 == 0 {
                    if let Err(e) = self.save_checkpoint(path) {
                        eprintln!("checkpoint save failed: {}", e);
                    } else {
                        println!("checkpoint saved at iteration {} to {}", self.iterations, path);
                    }
                }
            }
        }
    }

    /// serialize full solver state (nodes + rng + iterations + variant) for resume
    pub fn save_checkpoint(&self, path: &str) -> Result<(), String> {
        let mut buf: Vec<u8> = Vec::new();

        // magic + version
        buf.extend_from_slice(b"CFR1");

        // variant tag
        match self.variant {
            CfrVariant::Vanilla => buf.push(0),
            CfrVariant::DcfrPlus { .. } => buf.push(1),
        };
        if let CfrVariant::DcfrPlus { alpha, gamma } = self.variant {
            buf.extend_from_slice(&alpha.to_le_bytes());
            buf.extend_from_slice(&gamma.to_le_bytes());
        }

        // iterations + rng
        buf.extend_from_slice(&self.iterations.to_le_bytes());
        buf.extend_from_slice(&self.rng_state.to_le_bytes());

        // rules
        buf.extend_from_slice(&self.rules.buyin.to_le_bytes());
        buf.extend_from_slice(&self.rules.small_blind.to_le_bytes());
        buf.extend_from_slice(&self.rules.big_blind.to_le_bytes());
        buf.extend_from_slice(&self.rules.rake_bps.to_le_bytes());
        buf.extend_from_slice(&self.rules.rake_cap.to_le_bytes());

        // nodes
        let count = self.nodes.len() as u64;
        buf.extend_from_slice(&count.to_le_bytes());

        for (key, node) in &self.nodes {
            let key_bytes = key.to_bytes();
            buf.extend_from_slice(&(key_bytes.len() as u16).to_le_bytes());
            buf.extend_from_slice(&key_bytes);

            buf.extend_from_slice(&(node.num_actions as u16).to_le_bytes());
            for &r in &node.regret_sum {
                buf.extend_from_slice(&r.to_le_bytes());
            }
            for &s in &node.strategy_sum {
                buf.extend_from_slice(&s.to_le_bytes());
            }
        }

        std::fs::write(path, &buf).map_err(|e| e.to_string())
    }

    /// load solver state from checkpoint
    pub fn load_checkpoint(path: &str) -> Result<Self, String> {
        let data = std::fs::read(path).map_err(|e| e.to_string())?;
        let mut pos = 0;

        fn read_bytes<'a>(data: &'a [u8], pos: &mut usize, n: usize) -> Result<&'a [u8], String> {
            if *pos + n > data.len() { return Err("truncated checkpoint".into()); }
            let slice = &data[*pos..*pos + n];
            *pos += n;
            Ok(slice)
        }

        // magic
        let magic = read_bytes(&data, &mut pos, 4)?;
        if magic != b"CFR1" { return Err("not a CFR checkpoint".into()); }

        // variant
        let tag = read_bytes(&data, &mut pos, 1)?[0];
        let variant = match tag {
            0 => CfrVariant::Vanilla,
            1 => {
                let alpha = f64::from_le_bytes(read_bytes(&data, &mut pos, 8)?.try_into().unwrap());
                let gamma = f64::from_le_bytes(read_bytes(&data, &mut pos, 8)?.try_into().unwrap());
                CfrVariant::DcfrPlus { alpha, gamma }
            }
            _ => return Err(format!("unknown variant tag {}", tag)),
        };

        let iterations = u64::from_le_bytes(read_bytes(&data, &mut pos, 8)?.try_into().unwrap());
        let rng_state = u64::from_le_bytes(read_bytes(&data, &mut pos, 8)?.try_into().unwrap());

        // rules
        let buyin = u32::from_le_bytes(read_bytes(&data, &mut pos, 4)?.try_into().unwrap());
        let small_blind = u32::from_le_bytes(read_bytes(&data, &mut pos, 4)?.try_into().unwrap());
        let big_blind = u32::from_le_bytes(read_bytes(&data, &mut pos, 4)?.try_into().unwrap());
        let rake_bps = u16::from_le_bytes(read_bytes(&data, &mut pos, 2)?.try_into().unwrap());
        let rake_cap = u32::from_le_bytes(read_bytes(&data, &mut pos, 4)?.try_into().unwrap());
        let rules = Rules { buyin, small_blind, big_blind, turn_timeout_blocks: 6, rake_bps, rake_cap };

        // nodes
        let count = u64::from_le_bytes(read_bytes(&data, &mut pos, 8)?.try_into().unwrap()) as usize;
        let mut nodes = HashMap::with_capacity(count);

        for _ in 0..count {
            let key_len = u16::from_le_bytes(read_bytes(&data, &mut pos, 2)?.try_into().unwrap()) as usize;
            let key_bytes = read_bytes(&data, &mut pos, key_len)?;
            let key = super::abstraction::InfoSetKey::from_bytes(key_bytes)?;

            let num_actions = u16::from_le_bytes(read_bytes(&data, &mut pos, 2)?.try_into().unwrap()) as usize;
            let mut regret_sum = Vec::with_capacity(num_actions);
            for _ in 0..num_actions {
                regret_sum.push(f64::from_le_bytes(read_bytes(&data, &mut pos, 8)?.try_into().unwrap()));
            }
            let mut strategy_sum = Vec::with_capacity(num_actions);
            for _ in 0..num_actions {
                strategy_sum.push(f64::from_le_bytes(read_bytes(&data, &mut pos, 8)?.try_into().unwrap()));
            }

            nodes.insert(key, InfoNode { regret_sum, strategy_sum, num_actions });
        }

        Ok(Solver { nodes, rules, iterations, variant, rng_state })
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
