//! Information set abstraction for MCCFR.
//!
//! Reduces the game tree to a tractable size by bucketing:
//!   - Hole cards: 169 canonical preflop hands (suited/offsuit/pair)
//!   - Postflop: hand strength percentile buckets via Monte Carlo rollout
//!   - Action sequences: compressed action history per street
//!
//! The key insight from Pluribus: you don't need fine-grained abstraction
//! everywhere. Coarse buckets + depth-limited search at play time.

use crate::Action;

/// number of hand strength buckets per street
pub const PREFLOP_BUCKETS: usize = 169;  // canonical hole card combos
pub const FLOP_BUCKETS: usize = 10;      // hand strength deciles
pub const TURN_BUCKETS: usize = 10;
pub const RIVER_BUCKETS: usize = 10;

// GPU FFI for hand equity (ROCm HIP)
#[cfg(feature = "gpu")]
extern "C" {
    fn gpu_hand_equity(hole0: u8, hole1: u8, board: *const u8, num_board: i32, num_rollouts: i32) -> i32;
}

/// compute hand strength bucket using GPU (10K rollouts in ~2ms)
#[cfg(feature = "gpu")]
pub fn hand_strength_bucket_gpu(hole: [u8; 2], board: &[u8], num_buckets: usize) -> u16 {
    let equity = unsafe {
        gpu_hand_equity(hole[0], hole[1], board.as_ptr(), board.len() as i32, 10000)
    };
    // equity is 0-10000 (basis points). map to bucket.
    let bucket = (equity as f64 / 10000.0 * num_buckets as f64).min(num_buckets as f64 - 1.0) as u16;
    bucket
}

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

/// information set key: hand bucket + action history (heads-up)
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

    pub fn from_bytes(data: &[u8]) -> Result<Self, String> {
        if data.len() < 4 { return Err("info set key too short".into()); }
        let hand_bucket = u16::from_le_bytes(data[0..2].try_into().unwrap());
        let street = data[2];
        let hist_len = data[3] as usize;
        if data.len() < 4 + hist_len { return Err("truncated history".into()); }
        let history = data[4..4 + hist_len].to_vec();
        Ok(Self { hand_bucket, history, street })
    }
}

// ── Multiplayer abstraction (Pluribus-style) ────────────────────────
//
// For N>2 players, the full action history is intractable.
// Pluribus compresses the info set to:
//   1. Hand bucket (same as heads-up)
//   2. Position relative to button (0 = BTN, 1 = SB, ... N-1)
//   3. Per-street action summary (not full history):
//      - num_raises this street (0-3+)
//      - num_callers (0-N)
//      - last aggressor position relative to hero
//      - pot size bucket (fraction of starting stacks)
//   4. Street (0-3)
//
// This keeps info sets ~100K even at 6-max (vs billions with full history).

/// compressed per-street action summary
#[derive(Clone, Hash, Eq, PartialEq, Debug)]
pub struct StreetSummary {
    /// raises this street (capped at 3)
    pub num_raises: u8,
    /// players who called/are still active
    pub num_active: u8,
    /// pot size bucket (0-7: fraction of total starting stacks)
    pub pot_bucket: u8,
}

impl StreetSummary {
    pub fn to_byte(&self) -> u8 {
        // pack into single byte: 2 bits raises + 3 bits active + 3 bits pot
        (self.num_raises.min(3) << 6) | (self.num_active.min(7) << 3) | self.pot_bucket.min(7)
    }

    pub fn from_byte(b: u8) -> Self {
        Self {
            num_raises: (b >> 6) & 0x3,
            num_active: (b >> 3) & 0x7,
            pot_bucket: b & 0x7,
        }
    }
}

/// multiplayer info set key (Pluribus-style compressed)
#[derive(Clone, Hash, Eq, PartialEq, Debug)]
pub struct MultiInfoSetKey {
    /// hand bucket (preflop: 0-168, postflop: 0-9)
    pub hand_bucket: u16,
    /// position relative to button (0=BTN, 1=SB, 2=BB, 3=UTG, etc.)
    pub position: u8,
    /// current street (0-3)
    pub street: u8,
    /// per-street summaries for streets 0..=current
    pub street_summaries: Vec<StreetSummary>,
}

/// pot size → bucket (0-7)
pub fn pot_bucket(pot: u32, total_stacks: u32) -> u8 {
    if total_stacks == 0 { return 0; }
    let ratio = pot as f64 / total_stacks as f64;
    // 0: <5%, 1: 5-10%, 2: 10-20%, 3: 20-35%, 4: 35-50%, 5: 50-70%, 6: 70-90%, 7: >90%
    if ratio < 0.05 { 0 }
    else if ratio < 0.10 { 1 }
    else if ratio < 0.20 { 2 }
    else if ratio < 0.35 { 3 }
    else if ratio < 0.50 { 4 }
    else if ratio < 0.70 { 5 }
    else if ratio < 0.90 { 6 }
    else { 7 }
}

/// position relative to button
pub fn relative_position(seat: u8, button: u8, num_players: u8) -> u8 {
    // 0 = button, 1 = SB, 2 = BB, 3+ = early/middle positions
    (seat + num_players - button) % num_players
}

impl MultiInfoSetKey {
    /// serialize to bytes for hashmap lookup
    pub fn to_bytes(&self) -> Vec<u8> {
        // magic byte 0xFF to distinguish from heads-up keys
        let mut bytes = vec![0xFF];
        bytes.extend_from_slice(&self.hand_bucket.to_le_bytes());
        bytes.push(self.position);
        bytes.push(self.street);
        bytes.push(self.street_summaries.len() as u8);
        for ss in &self.street_summaries {
            bytes.push(ss.to_byte());
        }
        bytes
    }

    pub fn from_bytes(data: &[u8]) -> Result<Self, String> {
        if data.len() < 6 || data[0] != 0xFF {
            return Err("not a multi info set key".into());
        }
        let hand_bucket = u16::from_le_bytes(data[1..3].try_into().unwrap());
        let position = data[3];
        let street = data[4];
        let n = data[5] as usize;
        if data.len() < 6 + n { return Err("truncated summaries".into()); }
        let summaries = data[6..6+n].iter().map(|&b| StreetSummary::from_byte(b)).collect();
        Ok(Self { hand_bucket, position, street, street_summaries: summaries })
    }

    /// estimate the number of unique info sets for N players
    /// = hand_buckets × positions × street_combos
    pub fn estimate_info_sets(num_players: u8) -> usize {
        let hand_buckets = 169 + 10 + 10 + 10; // preflop + flop + turn + river
        let positions = num_players as usize;
        // per-street: 4 raise levels × 8 active × 8 pot = 256 combos
        // but most are unreachable, ~50 realistic per street × 4 streets
        let street_combos = 50 * 4;
        hand_buckets * positions * street_combos
    }
}

/// compute multi-player info set from game state
pub fn compute_multi_info_key(
    hole: [u8; 2],
    board: &[u8],
    street: u8,
    position: u8,
    num_raises_per_street: &[u8],
    active_per_street: &[u8],
    pot: u32,
    total_stacks: u32,
    rng: &mut impl FnMut() -> u32,
) -> MultiInfoSetKey {
    let hand_bucket = if street == 0 {
        preflop_bucket(hole[0], hole[1])
    } else {
        let board_len = match street { 1 => 3, 2 => 4, _ => 5 };
        let actual_board = if board.len() >= board_len { &board[..board_len] } else { board };
        hand_strength_bucket(hole, actual_board, 10, 200, rng)
    };

    let pb = pot_bucket(pot, total_stacks);
    let mut summaries = Vec::new();
    for s in 0..=street as usize {
        summaries.push(StreetSummary {
            num_raises: *num_raises_per_street.get(s).unwrap_or(&0),
            num_active: *active_per_street.get(s).unwrap_or(&2),
            pot_bucket: pb, // use current pot bucket for all (approximation)
        });
    }

    MultiInfoSetKey {
        hand_bucket,
        position,
        street,
        street_summaries: summaries,
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
