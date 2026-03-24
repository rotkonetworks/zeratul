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
use super::brain::{Brain, ExploitMode};
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
        }
    }

    /// play N hands, return training samples from both players' perspectives
    pub fn play(&self, num_hands: u64) -> SelfPlayResult {
        let mut brain0 = Brain::new(&self.strategy).with_mode(ExploitMode::Exploit);
        let mut brain1 = Brain::new(&self.strategy).with_mode(ExploitMode::Exploit);

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
                stacks: [1000u32, 1000, 0, 0, 0, 0, 0, 0, 0, 0],
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

                // map decision to fixed action space
                let mut action_probs = [0.0f32; NUM_ACTIONS];
                for (i, &p) in decision.action_probs.iter().enumerate().take(NUM_ACTIONS) {
                    action_probs[i] = p as f32;
                }

                // route to expert
                let expert_id = ctm::route(&features)[0].0 as u8;

                hand_samples.push((acting, features.features, action_probs));

                // sample action
                let (action, amount) = match decision.sample(rng() as f64 / u32::MAX as f64) {
                    Some(a) => a,
                    None => (Action::Check, 0),
                };

                // apply action to state
                match action {
                    Action::Fold => { folded = true; break; }
                    Action::Check => {
                        state.round_actions += 1;
                        // advance
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

                // observe action in opponent brain
                let opp_brain = if acting == 0 { &mut brain1 } else { &mut brain0 };
                opp_brain.observe_action(acting, action, amount, &state);

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
