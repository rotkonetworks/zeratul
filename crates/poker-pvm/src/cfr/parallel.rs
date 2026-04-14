//! Parallel MCCFR solver for multi-core CPUs.
//!
//! Uses lock-free concurrent hashmap (DashMap) for the shared node table.
//! Each thread runs independent MCCFR traversals with its own RNG.
//! Node updates use fine-grained locks — no global synchronization.
//!
//! On EPYC 9654 (96 cores): ~48K iterations/sec
//! 100M iterations in ~35 minutes.
//!
//! Architecture:
//!   - N worker threads, each with own RNG seed
//!   - Shared DashMap<InfoSetKey, AtomicNode> for regrets + strategy sums
//!   - Each worker: deal → traverse → update regrets (lock per node, not global)
//!   - Periodic progress reporting from main thread

#[cfg(feature = "parallel")]
pub mod par {
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::sync::Arc;
    use dashmap::DashMap;
    use rayon::prelude::*;

    use crate::{GameState, Rules, Phase, Action, SignedAction};
    use super::super::abstraction::*;

    /// thread-safe node with interior mutability
    #[derive(Debug)]
    pub struct AtomicNode {
        pub regret_sum: parking_lot::Mutex<Vec<f64>>,
        pub strategy_sum: parking_lot::Mutex<Vec<f64>>,
        pub num_actions: usize,
    }

    impl AtomicNode {
        pub fn new(num_actions: usize) -> Self {
            Self {
                regret_sum: parking_lot::Mutex::new(vec![0.0; num_actions]),
                strategy_sum: parking_lot::Mutex::new(vec![0.0; num_actions]),
                num_actions,
            }
        }

        pub fn current_strategy(&self) -> Vec<f64> {
            let regrets = self.regret_sum.lock();
            let positive_sum: f64 = regrets.iter().map(|&r| r.max(0.0)).sum();
            if positive_sum > 0.0 {
                regrets.iter().map(|&r| r.max(0.0) / positive_sum).collect()
            } else {
                vec![1.0 / self.num_actions as f64; self.num_actions]
            }
        }

        pub fn average_strategy(&self) -> Vec<f64> {
            let sums = self.strategy_sum.lock();
            let total: f64 = sums.iter().sum();
            if total > 0.0 {
                sums.iter().map(|&s| s / total).collect()
            } else {
                vec![1.0 / self.num_actions as f64; self.num_actions]
            }
        }
    }

    /// per-thread RNG (xorshift64)
    struct ThreadRng(u64);
    impl ThreadRng {
        fn new(seed: u64) -> Self { Self(seed) }
        fn next(&mut self) -> u32 {
            self.0 ^= self.0 << 13;
            self.0 ^= self.0 >> 7;
            self.0 ^= self.0 << 17;
            (self.0 >> 16) as u32
        }
        fn next_f64(&mut self) -> f64 { self.next() as f64 / u32::MAX as f64 }
    }

    /// parallel MCCFR solver
    pub struct ParallelSolver {
        pub nodes: Arc<DashMap<Vec<u8>, AtomicNode>>,
        pub rules: Rules,
        pub iterations: AtomicU64,
        pub num_threads: usize,
    }

    impl ParallelSolver {
        pub fn new(rules: Rules, num_threads: usize) -> Self {
            println!("[parallel-cfr] {} threads", num_threads);
            Self {
                nodes: Arc::new(DashMap::with_capacity(100_000)),
                rules,
                iterations: AtomicU64::new(0),
                num_threads,
            }
        }

        /// run N total iterations across all threads
        /// saves intermediate strategy every `checkpoint_every` iterations
        pub fn train(&self, total_iterations: u64) {
            self.train_with_saves(total_iterations, None, 0);
        }

        pub fn train_with_saves(&self, total_iterations: u64, output_path: Option<&str>, checkpoint_every: u64) {
            let iters_per_thread = total_iterations / self.num_threads as u64;
            let nodes = self.nodes.clone();
            let rules = self.rules;
            let iterations = &self.iterations;
            let last_checkpoint = std::sync::atomic::AtomicU64::new(0);

            // configure rayon thread pool
            let pool = rayon::ThreadPoolBuilder::new()
                .num_threads(self.num_threads)
                .build()
                .unwrap();

            pool.scope(|s| {
                for thread_id in 0..self.num_threads {
                    let nodes = nodes.clone();
                    let last_checkpoint = &last_checkpoint;
                    s.spawn(move |_| {
                        let mut rng = ThreadRng::new(0xCAFE_BABE + thread_id as u64 * 7919);
                        let mut local_iters = 0u64;

                        for i in 0..iters_per_thread {
                            let training_player = (i % 2) as u8;

                            // deal
                            let mut deck: Vec<u8> = (0..52).collect();
                            for j in (1..52).rev() {
                                let k = (rng.next() as usize) % (j + 1);
                                deck.swap(j, k);
                            }
                            let cards = [[deck[0], deck[1]], [deck[2], deck[3]]];
                            let community = [deck[4], deck[5], deck[6], deck[7], deck[8]];

                            let mut state = GameState::new(rules, 2);
                            state.deal(&cards, community);
                            let mut history = Vec::new();

                            Self::traverse(
                                &nodes, &mut state, &cards, &community,
                                &mut history, training_player, rules, &mut rng,
                            );

                            local_iters += 1;
                            if local_iters % 10000 == 0 {
                                let total = iterations.fetch_add(10000, Ordering::Relaxed) + 10000;
                                if thread_id == 0 {
                                    println!("iteration {}: {} info sets ({} threads)",
                                        total, nodes.len(), rayon::current_num_threads());

                                    // checkpoint save (only thread 0)
                                    if checkpoint_every > 0 {
                                        let last = last_checkpoint.load(Ordering::Relaxed);
                                        if total >= last + checkpoint_every {
                                            last_checkpoint.store(total, Ordering::Relaxed);
                                            if let Some(path) = output_path {
                                                let ckpt_path = format!("{}.{}m.bin",
                                                    path.trim_end_matches(".bin"), total / 1_000_000);
                                                let data = Self::export_strategy_from_nodes(&nodes);
                                                match std::fs::write(&ckpt_path, &data) {
                                                    Ok(_) => println!("checkpoint saved: {} ({} KB)",
                                                        ckpt_path, data.len() / 1024),
                                                    Err(e) => eprintln!("checkpoint failed: {}", e),
                                                }
                                            }
                                        }
                                    }
                                }
                            }
                        }

                        // flush remaining count
                        let rem = local_iters % 10000;
                        if rem > 0 {
                            iterations.fetch_add(rem, Ordering::Relaxed);
                        }
                    });
                }
            });
        }

        fn export_strategy_from_nodes(nodes: &DashMap<Vec<u8>, AtomicNode>) -> Vec<u8> {
            let mut buf = Vec::new();
            let count = nodes.len() as u32;
            buf.extend_from_slice(&count.to_le_bytes());
            for entry in nodes.iter() {
                let key = entry.key();
                let node = entry.value();
                let avg = node.average_strategy();
                buf.push(key.len() as u8);
                buf.extend_from_slice(key);
                buf.push(avg.len() as u8);
                for &p in &avg {
                    let fixed = (p * 65535.0).round().min(65535.0) as u16;
                    buf.extend_from_slice(&fixed.to_le_bytes());
                }
            }
            buf
        }

        fn get_actions(rules: &Rules, state: &GameState) -> Vec<(Action, u32)> {
            let seat = state.acting_seat as usize;
            let max_bet = state.bets.iter().take(2).copied().max().unwrap_or(0);
            let my_bet = state.bets[seat];
            let stack = state.stacks[seat];
            let facing = my_bet < max_bet;
            let bb = rules.big_blind;
            let pot = state.pot;

            let mut actions = vec![(Action::Fold, 0)];
            if !facing { actions.push((Action::Check, 0)); }
            if facing && stack > 0 { actions.push((Action::Call, 0)); }

            // 5 bet sizes matching 9-action abstraction: 25%, 50%, 75%, 100%, 200% of pot
            let min_bet = if facing { (max_bet * 2 - my_bet).max(bb) } else { bb };
            let action_type = if facing { Action::Raise } else { Action::Bet };
            let mut seen = Vec::new();
            for &frac_num in &[1u32, 2, 3, 4, 8] {
                let amount = (pot * frac_num / 4).max(min_bet).min(stack);
                if amount >= min_bet && amount < stack && !seen.contains(&amount) {
                    actions.push((action_type, amount));
                    seen.push(amount);
                }
            }

            if stack > 0 { actions.push((Action::AllIn, 0)); }
            actions
        }

        fn get_info_key(
            state: &GameState, cards: &[[u8; 2]; 2], community: &[u8; 5],
            history: &[u8], rng: &mut ThreadRng,
        ) -> Vec<u8> {
            let seat = state.acting_seat as usize;
            let opp = 1 - seat;
            let hole = cards[seat];
            let street = match state.phase {
                Phase::Preflop => 0u8, Phase::Flop => 1, Phase::Turn => 2, Phase::River => 3, _ => 0,
            };
            let hand_bucket = if street == 0 {
                preflop_bucket(hole[0], hole[1])
            } else {
                let board_len = match street { 1 => 3, 2 => 4, _ => 5 };
                hand_strength_bucket(hole, &community[..board_len], FLOP_BUCKETS, 200, &mut || rng.next())
            };

            // Position: IP (button) vs OOP
            let position = if state.acting_seat == state.button { 1 } else { 0 };

            // Effective stack depth
            let stack_bucket = stack_depth_bucket(
                state.stacks[seat], state.stacks[opp], state.rules.big_blind as u32,
            );

            // Board texture
            let board_texture = if state.community_count > 0 {
                board_texture_bucket(&community[..state.community_count as usize])
            } else { 0 };

            let key = InfoSetKey {
                hand_bucket, history: history.to_vec(), street,
                position, stack_bucket, board_texture,
            };
            key.to_bytes()
        }

        fn traverse(
            nodes: &DashMap<Vec<u8>, AtomicNode>,
            state: &mut GameState,
            cards: &[[u8; 2]; 2],
            community: &[u8; 5],
            history: &mut Vec<u8>,
            training_player: u8,
            rules: Rules,
            rng: &mut ThreadRng,
        ) -> f64 {
            if state.phase == Phase::Settled || state.phase == Phase::Showdown {
                if state.phase == Phase::Showdown { state.showdown(); }
                return state.stacks[training_player as usize] as f64 - rules.buyin as f64;
            }

            let acting = state.acting_seat;
            let is_training = acting == training_player;
            let actions = Self::get_actions(&rules, state);
            let num_actions = actions.len();
            if num_actions == 0 { return 0.0; }

            let key = Self::get_info_key(state, cards, community, history, rng);

            // ensure node exists
            if !nodes.contains_key(&key) {
                nodes.insert(key.clone(), AtomicNode::new(num_actions));
            }

            let strategy = {
                let node = nodes.get(&key).unwrap();
                if node.num_actions != num_actions {
                    drop(node);
                    nodes.insert(key.clone(), AtomicNode::new(num_actions));
                    vec![1.0 / num_actions as f64; num_actions]
                } else {
                    node.current_strategy()
                }
            };

            if is_training {
                let mut action_values = vec![0.0; num_actions];
                let mut node_value = 0.0;

                for (a, &(action, amount)) in actions.iter().enumerate() {
                    let mut child = state.clone();
                    let signed = SignedAction { seat: acting, action, amount, seq: 0, sig: [0; 64] };
                    match child.apply(&signed) {
                        Ok(_) => {
                            history.push(abstract_action(action, amount, state.pot, state.stacks[acting as usize]));
                            action_values[a] = Self::traverse(nodes, &mut child, cards, community, history, training_player, rules, rng);
                            history.pop();
                        }
                        Err(_) => { action_values[a] = -(rules.buyin as f64); }
                    }
                    if a < strategy.len() {
                        node_value += strategy[a] * action_values[a];
                    }
                }

                // update regrets (CFR+)
                if let Some(node) = nodes.get(&key) {
                    let mut regrets = node.regret_sum.lock();
                    let n = regrets.len().min(num_actions);
                    for a in 0..n {
                        regrets[a] = (regrets[a] + action_values[a] - node_value).max(0.0);
                    }
                }

                node_value
            } else {
                // sample from strategy
                let r = rng.next_f64();
                let mut cumul = 0.0;
                let mut chosen = strategy.len() - 1;
                for (a, &s) in strategy.iter().enumerate() {
                    cumul += s;
                    if r < cumul { chosen = a; break; }
                }
                // clamp to valid action range
                if chosen >= num_actions { chosen = num_actions - 1; }

                // accumulate strategy sums
                if let Some(node) = nodes.get(&key) {
                    let mut sums = node.strategy_sum.lock();
                    let n = sums.len().min(strategy.len());
                    for a in 0..n {
                        sums[a] += strategy[a];
                    }
                }

                let (action, amount) = actions[chosen];
                let signed = SignedAction { seat: acting, action, amount, seq: 0, sig: [0; 64] };
                match state.apply(&signed) {
                    Ok(_) => {
                        history.push(abstract_action(action, amount, state.pot, state.stacks[acting as usize]));
                        let v = Self::traverse(nodes, state, cards, community, history, training_player, rules, rng);
                        history.pop();
                        v
                    }
                    Err(_) => -(rules.buyin as f64),
                }
            }
        }

        /// export strategy (compatible with single-threaded format)
        pub fn export_strategy(&self) -> Vec<u8> {
            let mut buf = Vec::new();
            let count = self.nodes.len() as u32;
            buf.extend_from_slice(&count.to_le_bytes());

            for entry in self.nodes.iter() {
                let key = entry.key();
                let node = entry.value();
                let avg = node.average_strategy();

                buf.push(key.len() as u8);
                buf.extend_from_slice(key);
                buf.push(avg.len() as u8);
                for &p in &avg {
                    let fixed = (p * 65535.0).round().min(65535.0) as u16;
                    buf.extend_from_slice(&fixed.to_le_bytes());
                }
            }
            buf
        }
    }
}

#[cfg(not(feature = "parallel"))]
pub mod par {
    // stub when parallel feature is not enabled
}
