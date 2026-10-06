//! Tournament runner: two brains play N hands, track net worth.
//!
//! Used for:
//!   1. Autoresearch: compare configs over 10K+ hands
//!   2. Regression testing: verify new code doesn't make bot worse
//!   3. Measuring Hebbian plasticity improvement over time
//!
//! Outputs: bb/100 (big blinds won per 100 hands), win rate, action stats,
//!          plasticity progression, stack trajectory

use crate::{GameState, Phase, Action, SeatState, Rules, MAX_SEATS};
use super::brain::Brain;

/// Tournament result
#[derive(Debug)]
pub struct TournamentResult {
    pub hands: u32,
    pub p0_bb_won: f64,        // total big blinds won by player 0
    pub p0_bb_per_100: f64,    // bb/100 (the standard measure)
    pub p0_wins: u32,
    pub p1_wins: u32,
    pub action_counts: [u32; 9], // per action type
    pub errors: u32,
    /// Win rate in 10-hand blocks (for tracking improvement over time)
    pub p0_block_winrates: Vec<f32>,
}

/// Run a heads-up tournament between two brains.
///
/// `brain_a` plays seat 0, `brain_b` plays seat 1.
/// Returns results from brain_a's perspective.
/// If `log_path` is Some, writes every hand to JSONL for retraining.
pub fn run_tournament(
    brain_a: &mut Brain,
    brain_b: &mut Brain,
    num_hands: u32,
    big_blind: u64,
    starting_stack: u64,
) -> TournamentResult {
    run_tournament_with_log(brain_a, brain_b, num_hands, big_blind, starting_stack, None)
}

/// Run tournament with a custom seed (for fair multi-matchup comparison)
pub fn run_tournament_seeded(
    brain_a: &mut Brain,
    brain_b: &mut Brain,
    num_hands: u32,
    big_blind: u64,
    starting_stack: u64,
    seed: u64,
) -> TournamentResult {
    run_tournament_inner(brain_a, brain_b, num_hands, big_blind, starting_stack, None, seed)
}

/// Tournament with optional data logging for retraining
pub fn run_tournament_with_log(
    brain_a: &mut Brain,
    brain_b: &mut Brain,
    num_hands: u32,
    big_blind: u64,
    starting_stack: u64,
    log_path: Option<&str>,
) -> TournamentResult {
    run_tournament_inner(brain_a, brain_b, num_hands, big_blind, starting_stack, log_path,
        0xDEADBEEF_CAFEBABE ^ (num_hands as u64 * 31))
}

fn run_tournament_inner(
    brain_a: &mut Brain,
    brain_b: &mut Brain,
    num_hands: u32,
    big_blind: u64,
    starting_stack: u64,
    log_path: Option<&str>,
    seed: u64,
) -> TournamentResult {
    use std::io::Write;
    let mut log_file = log_path.and_then(|p| {
        std::fs::OpenOptions::new().create(true).append(true).open(p).ok()
    });
    let mut result = TournamentResult {
        hands: 0, p0_bb_won: 0.0, p0_bb_per_100: 0.0,
        p0_wins: 0, p1_wins: 0,
        action_counts: [0; 9],
        errors: 0,
        p0_block_winrates: Vec::new(),
    };

    let small_blind = big_blind / 2;
    let mut rng_state: u64 = seed;
    let mut button = 0u8;

    // Persistent stacks — carry over between hands (net worth tournament)
    let mut net_stack_a = starting_stack as i64;
    let mut net_stack_b = starting_stack as i64;

    let mut block_wins = 0u32;
    let mut block_hands = 0u32;

    for hand_num in 0..num_hands {
        // Fisher-Yates shuffle
        let mut deck: Vec<u8> = (0..52).collect();
        for i in (1..52).rev() {
            let j = (splitmix64(&mut rng_state) as usize) % (i + 1);
            deck.swap(i, j);
        }

        let cards_a = [deck[0], deck[1]];
        let cards_b = [deck[2], deck[3]];
        let community = [deck[4], deck[5], deck[6], deck[7], deck[8]];
        let mut community_revealed = [0u8; 5];

        // Cash game style: reset to starting stack each hand
        // Net worth tracked separately via p0_bb_won
        let stack_a = starting_stack;
        let stack_b = starting_stack;

        button = 1 - button; // alternate

        // Build initial game state
        let mut pot = small_blind + big_blind;
        let mut bets = [0u64; 2];
        let mut stacks = [stack_a, stack_b];

        // Post blinds
        let sb_seat = button;
        let bb_seat = 1 - button;
        bets[sb_seat as usize] = small_blind;
        bets[bb_seat as usize] = big_blind;
        stacks[sb_seat as usize] -= small_blind;
        stacks[bb_seat as usize] -= big_blind;

        // Notify brains
        let gs = build_gs(&stacks, &bets, pot, &community_revealed, 0, Phase::Preflop, sb_seat, hand_num, button);
        brain_a.new_hand(&gs);
        brain_b.new_hand(&gs);
        brain_a.set_hero_cards(0, cards_a, &gs);
        brain_b.set_hero_cards(1, cards_b, &gs);

        // Play the hand
        let mut acting = sb_seat; // SB acts first preflop
        let mut phase = Phase::Preflop;
        let mut cc = 0u8;
        let mut folded = [false; 2];
        let mut actions_this_street = 0u32;
        let mut last_raiser: Option<u8> = None;

        'hand: loop {
            if folded[0] || folded[1] { break; }
            if stacks[0] == 0 && stacks[1] == 0 { break; } // all-in

            let gs = build_gs(&stacks, &bets, pot, &community_revealed, cc, phase, acting, hand_num, button);
            let cards = if acting == 0 { &cards_a } else { &cards_b };
            let brain = if acting == 0 { &mut *brain_a } else { &mut *brain_b };

            let decision = brain.decide(&gs, cards, &community_revealed);
            let (action, amount) = match decision.sample(rng_f64(&mut rng_state)) {
                Some(a) => a,
                None => (Action::Check, 0),
            };

            // Count action
            let action_idx = super::abstraction::abstract_action(action, amount, pot as u32, stacks[acting as usize] as u32);
            if (action_idx as usize) < result.action_counts.len() {
                result.action_counts[action_idx as usize] += 1;
            }

            // Apply action
            match action {
                Action::Fold => {
                    folded[acting as usize] = true;
                }
                Action::Check => {
                    actions_this_street += 1;
                }
                Action::Call => {
                    let to_call = bets[1 - acting as usize].saturating_sub(bets[acting as usize]);
                    let actual = to_call.min(stacks[acting as usize]);
                    stacks[acting as usize] -= actual;
                    bets[acting as usize] += actual;
                    pot += actual;
                    actions_this_street += 1;
                }
                Action::Bet | Action::Raise => {
                    let amt = (amount as u64).max(big_blind).min(stacks[acting as usize]);
                    stacks[acting as usize] -= amt;
                    bets[acting as usize] += amt;
                    pot += amt;
                    last_raiser = Some(acting);
                    actions_this_street += 1;
                }
                Action::AllIn => {
                    let amt = stacks[acting as usize];
                    stacks[acting as usize] = 0;
                    bets[acting as usize] += amt;
                    pot += amt;
                    last_raiser = Some(acting);
                    actions_this_street += 1;
                }
            }

            // Observe action for opponent's brain
            let opp_brain = if acting == 0 { &mut *brain_b } else { &mut *brain_a };
            opp_brain.observe_action(acting, action, amount, &gs);

            // Check if street is done (both acted, bets equal or someone folded)
            let street_done = folded[0] || folded[1]
                || (actions_this_street >= 2 && bets[0] == bets[1])
                || (stacks[0] == 0 && stacks[1] == 0);

            if street_done {
                if folded[0] || folded[1] { break 'hand; }

                // Next street
                match phase {
                    Phase::Preflop => {
                        phase = Phase::Flop;
                        community_revealed[0] = community[0];
                        community_revealed[1] = community[1];
                        community_revealed[2] = community[2];
                        cc = 3;
                    }
                    Phase::Flop => {
                        phase = Phase::Turn;
                        community_revealed[3] = community[3];
                        cc = 4;
                    }
                    Phase::Turn => {
                        phase = Phase::River;
                        community_revealed[4] = community[4];
                        cc = 5;
                    }
                    Phase::River => break 'hand, // showdown
                    _ => break 'hand,
                }

                // Reset for new street
                bets = [0; 2];
                actions_this_street = 0;
                last_raiser = None;
                acting = 1 - button; // OOP acts first postflop
                continue;
            }

            acting = 1 - acting;
        }

        // Determine winner
        let p0_profit: i64 = if folded[1] {
            // P1 folded, P0 wins pot
            (pot as i64) - (starting_stack as i64 - stacks[0] as i64)
        } else if folded[0] {
            // P0 folded
            -((starting_stack as i64) - (stacks[0] as i64))
        } else {
            // Showdown — compare hands
            let p0_hand = eval_hand(&cards_a, &community);
            let p1_hand = eval_hand(&cards_b, &community);
            if p0_hand > p1_hand {
                (pot as i64) - (starting_stack as i64 - stacks[0] as i64)
            } else if p1_hand > p0_hand {
                -((starting_stack as i64) - (stacks[0] as i64))
            } else {
                0 // split pot
            }
        };

        // Update persistent net stacks
        net_stack_a += p0_profit;
        net_stack_b -= p0_profit;

        // Log hand data for retraining
        if let Some(ref mut f) = log_file {
            let _ = writeln!(f, "{{\"hand\":{},\"cards_a\":[{},{}],\"cards_b\":[{},{}],\"community\":[{},{},{},{},{}],\"profit\":{},\"stacks\":[{},{}],\"phase\":{}}}",
                hand_num, cards_a[0], cards_a[1], cards_b[0], cards_b[1],
                community[0], community[1], community[2], community[3], community[4],
                p0_profit, net_stack_a, net_stack_b, cc);
        }

        let bb_won = p0_profit as f64 / big_blind as f64;
        result.p0_bb_won += bb_won;
        result.hands += 1;

        if p0_profit > 0 { result.p0_wins += 1; }
        else if p0_profit < 0 { result.p1_wins += 1; }

        // Feed outcome to Hebbian
        let outcome = if p0_profit > 0 { 1.0 } else if p0_profit < 0 { -1.0 } else { 0.0 };
        brain_a.hand_complete(0, outcome, 0, cc.min(3));
        brain_b.hand_complete(1, -outcome, 0, cc.min(3));

        // Track 10-hand block win rates
        block_hands += 1;
        if p0_profit > 0 { block_wins += 1; }
        if block_hands >= 10 {
            result.p0_block_winrates.push(block_wins as f32 / block_hands as f32);
            block_wins = 0;
            block_hands = 0;
        }
    }

    result.p0_bb_per_100 = if result.hands > 0 {
        result.p0_bb_won / result.hands as f64 * 100.0
    } else { 0.0 };

    result
}

fn build_gs(stacks: &[u64; 2], bets: &[u64; 2], pot: u64, community: &[u8; 5],
            cc: u8, phase: Phase, acting: u8, hand_num: u32, button: u8) -> GameState {
    let mut gs_stacks = [0u32; MAX_SEATS];
    let mut gs_bets = [0u32; MAX_SEATS];
    let mut seat_state = [SeatState::Empty; MAX_SEATS];
    gs_stacks[0] = stacks[0] as u32;
    gs_stacks[1] = stacks[1] as u32;
    gs_bets[0] = bets[0] as u32;
    gs_bets[1] = bets[1] as u32;
    seat_state[0] = SeatState::Active;
    seat_state[1] = SeatState::Active;
    GameState {
        stacks: gs_stacks, bets: gs_bets, pot: pot as u32,
        community: *community, community_count: cc, phase,
        acting_seat: acting, num_players: 2, hand_number: hand_num,
        button, seat_state,
        cards: [[0; 2]; MAX_SEATS],
        round_actions: 0, last_aggressor: 0, action_count: 0,
        last_action_hash: [0; 32], rake: 0,
        rules: Rules { buyin: 1000, small_blind: 5, big_blind: 10,
                       turn_timeout_blocks: 6, rake_bps: 0, rake_cap: 0,
                       level_hands: 0, blind_growth_pct: 0 },
    }
}

/// SplitMix64 — better statistical properties than xorshift for card dealing
fn splitmix64(state: &mut u64) -> u64 {
    *state = state.wrapping_add(0x9E3779B97F4A7C15);
    let mut z = *state;
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58476D1CE4E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D049BB133111EB);
    z ^ (z >> 31)
}

fn rng_f64(state: &mut u64) -> f64 {
    (splitmix64(state) >> 11) as f64 / (1u64 << 53) as f64
}

/// Simple hand evaluation (for tournament — doesn't need to be fast)
fn eval_hand(hole: &[u8; 2], community: &[u8; 5]) -> u32 {
    // Reuse the existing eval from abstraction
    let mut best = 0u32;
    let all7 = [hole[0], hole[1], community[0], community[1], community[2], community[3], community[4]];
    // Try all 5-card combos from 7 cards
    for i in 0..7 {
        for j in (i+1)..7 {
            // Skip these two cards, use the other 5
            let hand: Vec<u8> = (0..7).filter(|&k| k != i && k != j).map(|k| all7[k]).collect();
            let arr: [u8; 5] = [hand[0], hand[1], hand[2], hand[3], hand[4]];
            let val = crate::eval_5(arr);
            if val > best { best = val; }
        }
    }
    best
}
