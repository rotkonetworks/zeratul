//! self-play arena: two Brain instances play each other
//!
//! records every decision point for CTM-MoE training data.
//! runs at engine speed (~62K hands/sec) on a single core.
//!
//! usage:
//!   let arena = Arena::new(strategy_data);
//!   let samples = arena.play(100_000);
//!   // samples: Vec<(features, action_probs, value)>

use std::collections::HashMap;
use crate::*;
use super::brain::{Brain, FilterStack, SearchFilter, RangeFilter, ExploitFilter};
use super::strategy::import_strategy;
use super::abstraction::*;
use super::ctm::{self, NUM_FEATURES, NUM_ACTIONS};

/// one training sample from self-play
#[derive(Clone, Debug)]
pub struct SelfPlaySample {
    pub features: [f32; NUM_FEATURES],
    pub action_probs: [f32; NUM_ACTIONS],
    pub outcome_value: f32, // +1 won, -1 lost, 0 tie (normalized to pot)
    pub pot: u32,
    pub street: u8,
    pub expert_id: u8,
}

/// self-play arena
pub struct Arena {
    strategy: Vec<u8>,
    #[cfg(feature = "onnx")]
    moe_dir: Option<String>,
    #[cfg(feature = "onnx")]
    moe_weight: f32,
}

/// result of a self-play session
pub struct SelfPlayResult {
    pub samples: Vec<SelfPlaySample>,
    pub hands_played: u64,
    pub player_0_winnings: i64,
    pub player_1_winnings: i64,
}

impl Arena {
    pub fn new(strategy_data: &[u8]) -> Self {
        Self {
            strategy: strategy_data.to_vec(),
            #[cfg(feature = "onnx")]
            moe_dir: None,
            #[cfg(feature = "onnx")]
            moe_weight: 0.0,
        }
    }

    #[cfg(feature = "onnx")]
    pub fn with_moe(mut self, dir: &str, weight: f32) -> Self {
        self.moe_dir = Some(dir.to_string());
        self.moe_weight = weight;
        self
    }

    /// play N hands across multiple threads, merge results
    pub fn play_parallel(&self, num_hands: u64, num_threads: usize, fast: bool) -> SelfPlayResult {
        let hands_per_thread = num_hands / num_threads as u64;
        let remainder = num_hands % num_threads as u64;

        let strategy = &self.strategy;
        let handles: Vec<_> = (0..num_threads).map(|t| {
            let strat = strategy.clone();
            let n = hands_per_thread + if (t as u64) < remainder { 1 } else { 0 };
            std::thread::spawn(move || {
                let arena = Arena::new(&strat);
                if fast { arena.play_fast(n) } else { arena.play(n) }
            })
        }).collect();

        let mut all_samples = Vec::new();
        let mut total_hands = 0u64;
        let mut p0 = 0i64;
        let mut p1 = 0i64;

        for h in handles {
            let result = h.join().unwrap();
            all_samples.extend(result.samples);
            total_hands += result.hands_played;
            p0 += result.player_0_winnings;
            p1 += result.player_1_winnings;
        }

        SelfPlayResult {
            samples: all_samples,
            hands_played: total_hands,
            player_0_winnings: p0,
            player_1_winnings: p1,
        }
    }

    /// play N hands with L0 blueprint only — no search, no range tracking.
    /// ~62K hands/sec per thread. good for training data generation.
    pub fn play_fast(&self, num_hands: u64) -> SelfPlayResult {
        let blueprint = import_strategy(&self.strategy);
        let mut samples = Vec::new();
        let mut p0_winnings: i64 = 0;
        let mut p1_winnings: i64 = 0;

        let mut rng_state: u64 = 0xDEAD_BEEF_CAFE_1234 ^ (std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().subsec_nanos() as u64);
        let mut rng = || -> u32 {
            rng_state ^= rng_state << 13;
            rng_state ^= rng_state >> 7;
            rng_state ^= rng_state << 17;
            (rng_state >> 16) as u32
        };

        for hand_num in 0..num_hands {
            let button = (hand_num % 2) as u8;
            let mut deck: Vec<u8> = (0..52).collect();
            for i in (1..52).rev() { let j = (rng() as usize) % (i+1); deck.swap(i, j); }

            let h0 = [deck[0], deck[1]];
            let h1 = [deck[2], deck[3]];
            let community = [deck[4], deck[5], deck[6], deck[7], deck[8]];

            // randomize stacks: 40% shortstack (10-30bb), 60% normal (50-100bb)
            let stack_roll = rng() % 100;
            let chips = if stack_roll < 40 {
                100 + (rng() % 201) // 100-300 chips (10-30bb)
            } else {
                500 + (rng() % 501) // 500-1000 chips (50-100bb)
            };
            let mut stacks = [chips, chips];
            let mut bets = [0u32, 0];
            let mut pot = 0u32;
            let mut acting = button;
            let mut round_actions = 0u8;
            let mut streets = 0u8;
            let mut history: Vec<u8> = Vec::new();
            let mut hand_samples: Vec<(u8, [f32; NUM_FEATURES], [f32; NUM_ACTIONS])> = Vec::new();
            let mut folded = false;

            // post blinds
            let sb = 5u32.min(stacks[acting as usize]);
            stacks[acting as usize] -= sb;
            bets[acting as usize] = sb;
            let bb_seat = 1 - acting;
            let bb = 10u32.min(stacks[bb_seat as usize]);
            stacks[bb_seat as usize] -= bb;
            bets[bb_seat as usize] = bb;
            pot = sb + bb;

            for _ in 0..50 {
                let cards = if acting == 0 { &h0 } else { &h1 };
                let community_count = match streets { 0 => 0usize, 1 => 3, 2 => 4, 3 => 5, _ => 5 };
                let bucket = Self::hand_bucket_fast(cards, &community[..community_count]);
                let community_count = match streets { 0 => 0, 1 => 3, 2 => 4, 3 => 5, _ => 5 };

                // blueprint lookup
                let key_bytes = InfoSetKey { hand_bucket: bucket, history: history.clone(), street: streets }.to_bytes();
                let probs = blueprint.get(&key_bytes).cloned().unwrap_or_else(|| {
                    vec![0.15, 0.35, 0.35, 0.15] // fallback: slight fold bias
                });

                // extract features
                let opp = 1 - acting;
                let board = &community[..community_count];
                let features = ctm::extract_all(
                    board, pot, stacks[acting as usize], stacks[opp as usize],
                    bets[acting as usize], 10, 0.5, 0.3, 0.1, 0.1, acting == button,
                );
                // map blueprint 4-bucket [fold, check/call, small, big] → 9-action
                // [fold, check, call, bet_25, bet_50, bet_75, bet_100, bet_200, allin]
                let bp = &probs;
                let p_fold = bp.get(0).copied().unwrap_or(0.0) as f32;
                let p_passive = bp.get(1).copied().unwrap_or(0.0) as f32;
                let p_small = bp.get(2).copied().unwrap_or(0.0) as f32;
                let p_big = bp.get(3).copied().unwrap_or(0.0) as f32;
                let facing = bets[acting as usize] < bets[0].max(bets[1]);
                let mut action_probs = [0.0f32; NUM_ACTIONS];
                action_probs[0] = p_fold;                           // fold
                action_probs[1] = if !facing { p_passive } else { 0.0 }; // check
                action_probs[2] = if facing { p_passive } else { 0.0 };  // call
                action_probs[3] = p_small * 0.3;                    // bet_25
                action_probs[4] = p_small * 0.4;                    // bet_50
                action_probs[5] = p_small * 0.3;                    // bet_75
                action_probs[6] = p_big * 0.4;                      // bet_100
                action_probs[7] = p_big * 0.3;                      // bet_200
                action_probs[8] = p_big * 0.3;                      // allin
                let total: f32 = action_probs.iter().sum();
                if total > 0.0 { for p in &mut action_probs { *p /= total; } }

                hand_samples.push((acting, features.features, action_probs));

                // sample action
                let r = rng() as f32 / u32::MAX as f32;
                let mut cumul = 0.0f32;
                let mut action_idx = 0usize;
                for (i, &p) in action_probs.iter().enumerate() {
                    cumul += p;
                    if r < cumul { action_idx = i; break; }
                }

                let to_call = bets[0].max(bets[1]) - bets[acting as usize];
                match action_idx {
                    0 => { folded = true; history.push(0); break; } // fold
                    1 => { history.push(1); } // check
                    2 => { // call
                        let actual = to_call.min(stacks[acting as usize]);
                        stacks[acting as usize] -= actual;
                        bets[acting as usize] += actual;
                        pot += actual;
                        history.push(2);
                    }
                    3..=7 => { // bet sizes: 25%, 50%, 75%, 100%, 200% pot
                        let frac = [1u32, 2, 3, 4, 8][action_idx - 3]; // /4
                        let raise = (pot * frac / 4).max(10);
                        let total_bet = bets[0].max(bets[1]) + raise;
                        let needed = total_bet - bets[acting as usize];
                        let actual = needed.min(stacks[acting as usize]);
                        stacks[acting as usize] -= actual;
                        bets[acting as usize] += actual;
                        pot += actual;
                        history.push(3);
                    }
                    _ => { // allin
                        let all = stacks[acting as usize];
                        bets[acting as usize] += all;
                        stacks[acting as usize] = 0;
                        pot += all;
                        history.push(4);
                    }
                }

                round_actions += 1;
                let both_acted = round_actions >= 2;
                let bets_eq = bets[0] == bets[1] || stacks[0] == 0 || stacks[1] == 0;

                if both_acted && bets_eq {
                    round_actions = 0;
                    bets = [0, 0];
                    streets += 1;
                    if streets > 3 { break; } // showdown
                    acting = 1 - button; // OOP first postflop
                } else {
                    acting = 1 - acting;
                }
            }

            let winner = if folded { 1 - acting }
                else {
                    let s0 = eval_best_5(h0, &community);
                    let s1 = eval_best_5(h1, &community);
                    if s0 > s1 { 0 } else if s1 > s0 { 1 } else { 255 }
                };

            let value_0 = if winner == 0 { 1.0 } else if winner == 1 { -1.0 } else { 0.0 };
            for (seat, features, aprobs) in &hand_samples {
                let v = if *seat == 0 { value_0 } else { -value_0 };
                samples.push(SelfPlaySample {
                    features: *features, action_probs: *aprobs,
                    outcome_value: v, pot, street: streets,
                    expert_id: ctm::route(&ctm::StateFeatures { features: *features })[0].0 as u8,
                });
            }

            let half = (pot / 2) as i64;
            if winner == 0 { p0_winnings += half; p1_winnings -= half; }
            else if winner == 1 { p1_winnings += half; p0_winnings -= half; }
        }

        SelfPlayResult { samples, hands_played: num_hands, player_0_winnings: p0_winnings, player_1_winnings: p1_winnings }
    }

    fn hand_bucket_fast(cards: &[u8; 2], board: &[u8]) -> u16 {
        // fast bucket from card ranks only (no rollout)
        let r0 = cards[0] % 13;
        let r1 = cards[1] % 13;
        let suited = cards[0] / 13 == cards[1] / 13;
        let high = r0.max(r1);
        let low = r0.min(r1);
        let gap = high - low;
        // simple hand strength heuristic: high cards + pair bonus + suited bonus
        let mut score = high as u16 * 2 + low as u16;
        if r0 == r1 { score += 20; } // pair
        if suited { score += 3; }
        if gap <= 2 { score += 2; } // connected
        // map to 0-9 bucket
        (score * 10 / 50).min(9)
    }

    pub fn play(&self, num_hands: u64) -> SelfPlayResult {
        self.play_inner(num_hands, true)
    }

    fn play_inner(&self, num_hands: u64, use_search: bool) -> SelfPlayResult {
        // load MoE if available
        #[cfg(feature = "onnx")]
        let moe = self.moe_dir.as_ref().and_then(|dir| {
            match super::inference::OnnxMoE::load(dir) {
                Ok(m) => {
                    println!("[moe] loaded from {}", dir);
                    Some(std::sync::Arc::new(m))
                }
                Err(e) => { eprintln!("[moe] failed to load: {}", e); None }
            }
        });

        let make_brain = |strategy: &[u8]| -> Brain {
            let mut filters = FilterStack::new();
            if use_search {
                filters = filters
                    .and_then(SearchFilter)
                    .and_then(RangeFilter)
                    .and_then(ExploitFilter);
            }
            let mut brain = Brain::new(strategy).with_filters(filters);
            #[cfg(feature = "onnx")]
            if let Some(ref m) = moe {
                brain = brain.with_moe(m.clone(), self.moe_weight);
            }
            brain
        };

        let mut brain0 = make_brain(&self.strategy);
        let mut brain1 = make_brain(&self.strategy);

        let mut samples = Vec::new();
        let mut p0_winnings: i64 = 0;
        let mut p1_winnings: i64 = 0;

        let rules = Rules {
            buyin: 1000, small_blind: 5, big_blind: 10,
            turn_timeout_blocks: 6, rake_bps: 0, rake_cap: 0,
        };

        let mut rng_state: u64 = 0xDEAD_BEEF_CAFE_1234;
        let mut rng = || -> u32 {
            rng_state ^= rng_state << 13;
            rng_state ^= rng_state >> 7;
            rng_state ^= rng_state << 17;
            (rng_state >> 16) as u32
        };

        for hand_num in 0..num_hands {
            let button = (hand_num % 2) as u8;

            // deal random cards
            let mut deck: Vec<u8> = (0..52).collect();
            for i in (1..52).rev() {
                let j = (rng() as usize) % (i + 1);
                deck.swap(i, j);
            }

            let hero0_cards = [deck[0], deck[1]];
            let hero1_cards = [deck[2], deck[3]];
            let community = [deck[4], deck[5], deck[6], deck[7], deck[8]];

            // init game state
            let mut state = GameState {
                stacks: {
                    // randomize: 40% shortstack (10-30bb), 60% normal (50-100bb)
                    let roll = rng() % 100;
                    let chips = if roll < 40 {
                        100 + (rng() % 201)
                    } else {
                        500 + (rng() % 501)
                    };
                    let mut s = [0u32; 10];
                    s[0] = chips;
                    s[1] = chips;
                    s
                },
                bets: [0; MAX_SEATS],
                pot: 0,
                community: [0; MAX_COMMUNITY],
                community_count: 0,
                phase: Phase::Preflop,
                acting_seat: button, // SB acts first preflop in HU
                num_players: 2,
                hand_number: hand_num as u32,
                button,
                seat_state: {
                    let mut s = [SeatState::Empty; MAX_SEATS];
                    s[0] = SeatState::Active;
                    s[1] = SeatState::Active;
                    s
                },
                cards: {
                    let mut c = [[0u8; 2]; MAX_SEATS];
                    c[0] = hero0_cards;
                    c[1] = hero1_cards;
                    c
                },
                round_actions: 0,
                last_aggressor: 0,
                action_count: 0,
                last_action_hash: [0; 32],
                rake: 0,
                rules,
            };

            // post blinds
            let sb_seat = button;
            let bb_seat = 1 - button;
            state.bets[sb_seat as usize] = 5;
            state.stacks[sb_seat as usize] -= 5;
            state.bets[bb_seat as usize] = 10;
            state.stacks[bb_seat as usize] -= 10;
            state.pot = 15;

            // reset brains for new hand
            brain0.new_hand(&state);
            brain1.new_hand(&state);
            brain0.set_hero_cards(0, hero0_cards, &state);
            brain1.set_hero_cards(1, hero1_cards, &state);

            let mut hand_samples: Vec<(u8, [f32; NUM_FEATURES], [f32; NUM_ACTIONS])> = Vec::new();
            let mut folded = false;
            let mut streets_dealt = 0u8;

            // play the hand
            for _action_num in 0..100 { // safety limit
                let acting = state.acting_seat;
                let brain = if acting == 0 { &mut brain0 } else { &mut brain1 };
                let cards = if acting == 0 { &hero0_cards } else { &hero1_cards };

                let decision = brain.decide(&state, cards, &community);

                // extract features for this decision point
                let board_slice = &community[..state.community_count as usize];
                let opp = 1 - acting;
                let features = ctm::extract_all(
                    board_slice,
                    state.pot,
                    state.stacks[acting as usize],
                    state.stacks[opp as usize],
                    state.bets[acting as usize],
                    10,
                    0.5, 0.3, 0.1, 0.1,
                    acting == button,
                );

                // map decision's variable actions → fixed 8-action training format
                // [fold, check, call, bet_25, bet_50, bet_100, bet_200, allin]
                let mut action_probs = [0.0f32; NUM_ACTIONS];
                for (j, (&(act, amt), &prob)) in decision.actions.iter()
                    .zip(decision.action_probs.iter()).enumerate()
                {
                    let idx = match act {
                        Action::Fold => 0,
                        Action::Check => 1,
                        Action::Call => 2,
                        Action::Bet | Action::Raise => {
                            let frac = if state.pot > 0 { amt as f64 / state.pot as f64 } else { 1.0 };
                            if frac <= 0.375 { 3 }       // bet_25
                            else if frac <= 0.625 { 4 }   // bet_50
                            else if frac <= 0.875 { 5 }   // bet_75
                            else if frac <= 1.5 { 6 }     // bet_100
                            else { 7 }                     // bet_200
                        }
                        Action::AllIn => 8,
                    };
                    action_probs[idx] += prob as f32;
                }

                // route to expert
                let expert_id = ctm::route(&features)[0].0 as u8;

                hand_samples.push((acting, features.features, action_probs));

                // sample action
                let (action, amount) = match decision.sample(rng() as f64 / u32::MAX as f64) {
                    Some(a) => a,
                    None => (Action::Check, 0),
                };

                // observe action in opponent brain BEFORE state update
                let opp_brain = if acting == 0 { &mut brain1 } else { &mut brain0 };
                opp_brain.observe_action(acting, action, amount, &state);

                // apply action to state
                match action {
                    Action::Fold => { folded = true; break; }
                    Action::Check => {
                        state.round_actions += 1;
                    }
                    Action::Call => {
                        let to_call = state.bets.iter().take(2).max().copied().unwrap_or(0)
                            - state.bets[acting as usize];
                        let actual = to_call.min(state.stacks[acting as usize]);
                        state.stacks[acting as usize] -= actual;
                        state.bets[acting as usize] += actual;
                        state.pot += actual;
                        state.round_actions += 1;
                    }
                    Action::Bet | Action::Raise => {
                        let bet_total = amount.max(state.bets.iter().take(2).max().copied().unwrap_or(0) + 10);
                        let needed = bet_total - state.bets[acting as usize];
                        let actual = needed.min(state.stacks[acting as usize]);
                        state.stacks[acting as usize] -= actual;
                        state.bets[acting as usize] += actual;
                        state.pot += actual;
                        state.round_actions += 1;
                    }
                    Action::AllIn => {
                        let all = state.stacks[acting as usize];
                        state.bets[acting as usize] += all;
                        state.stacks[acting as usize] = 0;
                        state.pot += all;
                        state.round_actions += 1;
                    }
                }

                // check if betting round complete (simplified)
                let both_acted = state.round_actions >= 2;
                let bets_equal = state.bets[0] == state.bets[1] ||
                    state.stacks[0] == 0 || state.stacks[1] == 0;

                if both_acted && bets_equal {
                    // advance street
                    state.round_actions = 0;
                    state.bets = [0; MAX_SEATS];
                    streets_dealt += 1;

                    match streets_dealt {
                        1 => {
                            state.phase = Phase::Flop;
                            state.community[0..3].copy_from_slice(&community[0..3]);
                            state.community_count = 3;
                            brain0.reveal_community(&community[0..3], &state);
                            brain1.reveal_community(&community[0..3], &state);
                        }
                        2 => {
                            state.phase = Phase::Turn;
                            state.community[3] = community[3];
                            state.community_count = 4;
                            brain0.reveal_community(&[community[3]], &state);
                            brain1.reveal_community(&[community[3]], &state);
                        }
                        3 => {
                            state.phase = Phase::River;
                            state.community[4] = community[4];
                            state.community_count = 5;
                            brain0.reveal_community(&[community[4]], &state);
                            brain1.reveal_community(&[community[4]], &state);
                        }
                        _ => {
                            // showdown
                            break;
                        }
                    }
                    // flip action
                    state.acting_seat = 1 - button; // OOP acts first postflop
                } else {
                    state.acting_seat = 1 - acting;
                }
            }

            // determine winner
            let winner = if folded {
                1 - state.acting_seat // other player wins
            } else {
                // showdown — use hand evaluation
                let h0 = eval_best_5(hero0_cards, &community);
                let h1 = eval_best_5(hero1_cards, &community);
                if h0 > h1 { 0 } else if h1 > h0 { 1 } else { 255 } // 255 = tie
            };

            // track showdown stats (only when hand went to showdown, not fold)
            if !folded {
                brain0.profiles.profiles[0].observe_showdown(winner == 0);
                brain0.profiles.profiles[1].observe_showdown(winner == 1);
                brain1.profiles.profiles[0].observe_showdown(winner == 0);
                brain1.profiles.profiles[1].observe_showdown(winner == 1);
            }

            let pot = state.pot;
            let value_0 = if winner == 0 { 1.0 } else if winner == 1 { -1.0 } else { 0.0 };

            // assign outcome values to samples
            for (acting, features, action_probs) in &hand_samples {
                let value = if *acting == 0 { value_0 } else { -value_0 };
                samples.push(SelfPlaySample {
                    features: *features,
                    action_probs: *action_probs,
                    outcome_value: value,
                    pot,
                    street: 0, // could track per-sample
                    expert_id: ctm::route(&ctm::StateFeatures { features: *features })[0].0 as u8,
                });
            }

            let half_pot = (pot / 2) as i64;
            if winner == 0 { p0_winnings += half_pot; p1_winnings -= half_pot; }
            else if winner == 1 { p1_winnings += half_pot; p0_winnings -= half_pot; }
        }

        SelfPlayResult {
            samples,
            hands_played: num_hands,
            player_0_winnings: p0_winnings,
            player_1_winnings: p1_winnings,
        }
    }
}

fn eval_best_5(hole: [u8; 2], community: &[u8; 5]) -> u32 {
    let cards = [hole[0], hole[1], community[0], community[1],
        community[2], community[3], community[4]];
    let mut best = 0u32;
    for i in 0..7 {
        for j in (i+1)..7 {
            let hand: Vec<u8> = (0..7).filter(|&k| k != i && k != j).map(|k| cards[k]).collect();
            let score = eval_5([hand[0], hand[1], hand[2], hand[3], hand[4]]);
            best = best.max(score);
        }
    }
    best
}

/// export samples as binary for Python training
pub fn export_samples(samples: &[SelfPlaySample]) -> Vec<u8> {
    let mut buf = Vec::new();
    // header
    buf.extend_from_slice(&(samples.len() as u32).to_le_bytes());
    buf.extend_from_slice(&(NUM_FEATURES as u32).to_le_bytes());
    buf.extend_from_slice(&(NUM_ACTIONS as u32).to_le_bytes());

    for s in samples {
        // features (27 * f32)
        for f in &s.features { buf.extend_from_slice(&f.to_le_bytes()); }
        // action probs (6 * f32)
        for p in &s.action_probs { buf.extend_from_slice(&p.to_le_bytes()); }
        // outcome value (f32)
        buf.extend_from_slice(&s.outcome_value.to_le_bytes());
        // pot (u32)
        buf.extend_from_slice(&s.pot.to_le_bytes());
        // expert_id (u8)
        buf.push(s.expert_id);
    }
    buf
}
