//! Information set abstraction for MCCFR.
//!
//! Reduces the game tree to a tractable size by bucketing:
//!   - Hole cards: 169 canonical preflop hands (suited/offsuit/pair)
//!   - Postflop: hand strength percentile buckets via Monte Carlo rollout
//!   - Action sequences: compressed action history per street
//!
//! The key insight from Pluribus: you don't need fine-grained abstraction
//! everywhere. Coarse buckets + depth-limited search at play time.

use crate::{GameState, Rules, Phase, Action, SignedAction, MAX_SEATS};

/// number of hand strength buckets per street
pub const PREFLOP_BUCKETS: usize = 169;  // canonical hole card combos
pub const FLOP_BUCKETS: usize = 10;      // hand strength deciles
pub const TURN_BUCKETS: usize = 10;
pub const RIVER_BUCKETS: usize = 10;

/// number of action abstraction buckets
pub const ACTION_BUCKETS: usize = 4; // fold, check/call, bet_small, bet_big/allin

/// canonical preflop hand index (0..168)
/// encodes 13×13 grid: pairs on diagonal, suited above, offsuit below
pub fn preflop_bucket(c0: u8, c1: u8) -> u16 {
    let r0 = c0 % 13;
    let r1 = c1 % 13;
    let s0 = c0 / 13;
    let s1 = c1 / 13;
    let suited = s0 == s1;

    let high = r0.max(r1) as u16;
    let low = r0.min(r1) as u16;

    if high == low {
        // pair: on the diagonal
        high * 13 + low
    } else if suited {
        // suited: above diagonal (high row, low col)
        high * 13 + low
    } else {
        // offsuit: below diagonal (low row, high col)
        low * 13 + high
    }
}

/// postflop hand strength via Monte Carlo rollout.
/// deals random opponent hands and remaining board, counts wins.
/// returns a percentile bucket (0 = worst, buckets-1 = best).
pub fn hand_strength_bucket(
    hole: [u8; 2],
    board: &[u8],
    num_buckets: usize,
    num_rollouts: usize,
    rng: &mut impl FnMut() -> u32,
) -> u16 {
    let mut wins = 0u32;
    let mut ties = 0u32;
    let total = num_rollouts as u32;

    let mut used = [false; 52];
    used[hole[0] as usize] = true;
    used[hole[1] as usize] = true;
    for &c in board { used[c as usize] = true; }

    let available: Vec<u8> = (0..52u8).filter(|&c| !used[c as usize]).collect();

    for _ in 0..num_rollouts {
        // pick 2 random opponent cards + fill remaining board
        let remaining_board = 5 - board.len();
        let needed = 2 + remaining_board;
        let mut picks = Vec::with_capacity(needed);
        let mut pick_used = [false; 52];

        while picks.len() < needed {
            let idx = (rng() as usize) % available.len();
            let card = available[idx];
            if !pick_used[card as usize] {
                pick_used[card as usize] = true;
                picks.push(card);
            }
        }

        let opp_hole = [picks[0], picks[1]];
        let mut full_board = [0u8; 5];
        for (i, &c) in board.iter().enumerate() { full_board[i] = c; }
        for i in 0..remaining_board { full_board[board.len() + i] = picks[2 + i]; }

        // evaluate both hands
        let my_score = best_hand_7(hole, &full_board);
        let opp_score = best_hand_7(opp_hole, &full_board);

        if my_score > opp_score { wins += 1; }
        else if my_score == opp_score { ties += 1; }
    }

    // equity as percentile
    let equity = (wins as f64 + ties as f64 * 0.5) / total as f64;
    let bucket = (equity * num_buckets as f64).min(num_buckets as f64 - 1.0) as u16;
    bucket
}

/// evaluate best 5-card hand from 2 hole + 5 community
fn best_hand_7(hole: [u8; 2], community: &[u8; 5]) -> u32 {
    let mut all7 = [0u8; 7];
    all7[0] = hole[0];
    all7[1] = hole[1];
    for i in 0..5 { all7[2 + i] = community[i]; }

    let mut best = 0u32;
    for i in 0..7 {
        for j in (i + 1)..7 {
            let mut hand = [0u8; 5];
            let mut idx = 0;
            for k in 0..7 {
                if k != i && k != j { hand[idx] = all7[k]; idx += 1; }
            }
            let score = crate::eval_5(hand);
            if score > best { best = score; }
        }
    }
    best
}

/// abstract action: maps a concrete action to a bucket
pub fn abstract_action(action: Action, amount: u32, pot: u32, stack: u32) -> u8 {
    match action {
        Action::Fold => 0,
        Action::Check | Action::Call => 1,
        Action::Bet | Action::Raise => {
            if amount as f64 > pot as f64 * 0.75 { 3 } // big bet
            else { 2 } // small bet
        }
        Action::AllIn => 3,
    }
}

/// information set key: hand bucket + action history
#[derive(Clone, Hash, Eq, PartialEq, Debug)]
pub struct InfoSetKey {
    /// hand bucket (preflop: 0-168, postflop: 0-9)
    pub hand_bucket: u16,
    /// compressed action history per street
    pub history: Vec<u8>,
    /// current betting round
    pub street: u8,
}

impl InfoSetKey {
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut bytes = Vec::new();
        bytes.extend_from_slice(&self.hand_bucket.to_le_bytes());
        bytes.push(self.street);
        bytes.push(self.history.len() as u8);
        bytes.extend_from_slice(&self.history);
        bytes
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_preflop_buckets() {
        // AA = pair of aces = bucket 12*13+12 = 168
        assert_eq!(preflop_bucket(12, 25), preflop_bucket(25, 12)); // A♠ A♥
        // AKs (suited)
        let aks = preflop_bucket(12, 11); // A♠ K♠ (same suit 0)
        // AKo (offsuit)
        let ako = preflop_bucket(12, 24); // A♠ K♥ (diff suits)
        assert_ne!(aks, ako, "suited != offsuit");
        // 22 = pair of twos
        let _22 = preflop_bucket(0, 13); // 2♠ 2♥
        assert_eq!(_22, 0); // lowest pair
    }

    #[test]
    fn test_hand_strength_rollout() {
        let mut rng_state: u32 = 12345;
        let mut rng = || -> u32 {
            rng_state = rng_state.wrapping_mul(1103515245).wrapping_add(12345);
            rng_state >> 16
        };

        // AA on empty board should be high bucket
        let aa_bucket = hand_strength_bucket([12, 25], &[], 10, 1000, &mut rng);
        // 72o on empty board should be low bucket
        let _72o_bucket = hand_strength_bucket([5, 13], &[], 10, 1000, &mut rng);

        println!("AA bucket: {}, 72o bucket: {}", aa_bucket, _72o_bucket);
        assert!(aa_bucket > _72o_bucket, "AA should be stronger than 72o");
    }
}
