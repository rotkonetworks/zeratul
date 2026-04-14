//! Range tracking: Bayesian inference over opponent holdings.
//!
//! Given a Nash blueprint and an action sequence, compute
//! the probability distribution over opponent's possible hands.
//!
//! This is pure game-tree reasoning, not opponent modeling.
//! It works against ANY opponent who plays close to Nash.
//! No profiling, no manipulation lever, no adaptation.
//!
//! Algorithm:
//!   1. Start with uniform distribution over all possible combos
//!      (minus cards we can see: our hole cards + board)
//!   2. For each opponent action, compute P(action | hand) from blueprint
//!   3. Multiply range by likelihood, renormalize (Bayes rule)
//!   4. Result: probability distribution over opponent's possible hands
//!
//! The search uses this range to weight opponent action sampling:
//!   instead of "what would a random hand do?" →
//!   "what would THIS range of hands do?"

use super::abstraction::*;
use super::strategy::import_strategy;
use std::collections::HashMap;

/// total possible 2-card combos from a 52-card deck
pub const NUM_COMBOS: usize = 1326; // C(52,2)

/// a hand range: probability weight for each possible 2-card combo
#[derive(Clone)]
pub struct Range {
    /// weights[combo_index] = probability this combo is in range
    /// combo_index maps to (card_a, card_b) where a < b
    pub weights: [f32; NUM_COMBOS],
}

impl Range {
    /// start with uniform range (all combos equally likely)
    pub fn uniform() -> Self {
        Self { weights: [1.0; NUM_COMBOS] }
    }

    /// remove combos containing any of the given cards (dead cards)
    pub fn remove_cards(&mut self, dead: &[u8]) {
        for i in 0..NUM_COMBOS {
            let (a, b) = combo_to_cards(i);
            if dead.contains(&a) || dead.contains(&b) {
                self.weights[i] = 0.0;
            }
        }
    }

    /// Bayesian update: multiply each combo's weight by P(action | hand)
    /// then renormalize
    pub fn update(&mut self, action_likelihoods: &[f32; NUM_COMBOS]) {
        let mut total = 0.0f32;
        for i in 0..NUM_COMBOS {
            self.weights[i] *= action_likelihoods[i];
            total += self.weights[i];
        }
        // renormalize
        if total > 1e-10 {
            for w in self.weights.iter_mut() {
                *w /= total;
            }
        }
    }

    /// fraction of range that's still alive (non-zero weight)
    /// 1.0 = full range, 0.0 = nothing left
    pub fn alive_fraction(&self) -> f32 {
        let alive: usize = self.weights.iter().filter(|&&w| w > 1e-10).count();
        alive as f32 / NUM_COMBOS as f32
    }

    /// how many effective combos remain (entropy-based)
    pub fn effective_combos(&self) -> f32 {
        let total: f32 = self.weights.iter().sum();
        if total < 1e-10 { return 0.0; }
        let mut entropy = 0.0f32;
        for &w in &self.weights {
            if w > 1e-10 {
                let p = w / total;
                entropy -= p * p.ln();
            }
        }
        entropy.exp()
    }

    /// what fraction of the range is likely strong hands?
    /// (top N% by hand strength)
    pub fn strength_distribution(&self, community: &[u8]) -> RangeStrength {
        let total: f32 = self.weights.iter().sum();
        if total < 1e-10 {
            return RangeStrength { strong: 0.0, medium: 0.0, weak: 0.0 };
        }

        let mut strong = 0.0f32; // top 30%
        let mut medium = 0.0f32; // middle 40%
        let mut weak = 0.0f32;   // bottom 30%

        // evaluate each combo's hand strength
        if community.len() >= 3 {
            let mut scored: Vec<(f32, f32)> = Vec::new(); // (score, weight)
            for i in 0..NUM_COMBOS {
                if self.weights[i] < 1e-10 { continue; }
                let (a, b) = combo_to_cards(i);
                // skip if uses community cards
                if community.contains(&a) || community.contains(&b) { continue; }
                let score = quick_hand_strength(a, b, community);
                scored.push((score, self.weights[i]));
            }
            scored.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap());

            let scored_total: f32 = scored.iter().map(|&(_, w)| w).sum();
            let mut cumulative = 0.0f32;
            for &(_, w) in &scored {
                let pct = cumulative / scored_total;
                if pct < 0.3 { strong += w; }
                else if pct < 0.7 { medium += w; }
                else { weak += w; }
                cumulative += w;
            }

            let all = strong + medium + weak;
            if all > 1e-10 {
                strong /= all;
                medium /= all;
                weak /= all;
            }
        } else {
            // preflop: use preflop bucket as proxy
            strong = 0.33;
            medium = 0.34;
            weak = 0.33;
        }

        RangeStrength { strong, medium, weak }
    }

    /// equity of our hand against this range (via enumeration, river only)
    pub fn equity_vs(&self, our_hand: [u8; 2], community: &[u8; 5]) -> f32 {
        let mut wins = 0.0f32;
        let mut total = 0.0f32;

        let our_score = crate::best_hand_7(our_hand, community);

        for i in 0..NUM_COMBOS {
            if self.weights[i] < 1e-10 { continue; }
            let (a, b) = combo_to_cards(i);
            if a == our_hand[0] || a == our_hand[1] || b == our_hand[0] || b == our_hand[1] {
                continue;
            }
            if community.contains(&a) || community.contains(&b) { continue; }

            let w = self.weights[i];
            let their_score = crate::best_hand_7([a, b], community);

            if our_score > their_score { wins += w; }
            else if our_score == their_score { wins += w * 0.5; }
            total += w;
        }

        if total > 1e-10 { wins / total } else { 0.5 }
    }
}

#[derive(Debug, Clone, Copy)]
pub struct RangeStrength {
    pub strong: f32, // fraction of range that's strong
    pub medium: f32,
    pub weak: f32,
}

/// compute P(action | hand) for all combos given a blueprint strategy
/// this is the likelihood function for Bayesian updating
pub fn action_likelihoods(
    blueprint: &HashMap<Vec<u8>, Vec<f64>>,
    community: &[u8],
    street: u8,
    history: &[u8],
    action_bucket: usize, // which abstract action was taken (0-3)
    rng: &mut impl FnMut() -> u32,
) -> [f32; NUM_COMBOS] {
    let mut likelihoods = [0.0f32; NUM_COMBOS];

    for i in 0..NUM_COMBOS {
        let (a, b) = combo_to_cards(i);
        // skip combos that conflict with community
        if community.contains(&a) || community.contains(&b) {
            likelihoods[i] = 0.0;
            continue;
        }

        // compute info set key for this hand
        let hand_bucket = if street == 0 {
            preflop_bucket(a, b)
        } else {
            let board_len = match street { 1 => 3, 2 => 4, _ => 5 };
            let board = &community[..board_len.min(community.len())];
            hand_strength_bucket([a, b], board, 10, 50, rng)
        };

        let key = InfoSetKey {
            hand_bucket,
            history: history.to_vec(),
            street,
            position: 0,
            stack_bucket: 3,
            board_texture: 0,
        };

        // look up blueprint probability for this action
        if let Some(probs) = blueprint.get(&key.to_bytes()) {
            if action_bucket < probs.len() {
                likelihoods[i] = probs[action_bucket] as f32;
            } else {
                likelihoods[i] = 0.1; // unknown action → small probability
            }
        } else {
            // no blueprint entry → assume uniform
            likelihoods[i] = 0.25;
        }
    }

    likelihoods
}

// ── helpers ──────────────────────────────────────────────────

/// convert combo index to two cards (a < b)
pub fn combo_to_cards(idx: usize) -> (u8, u8) {
    // combo index = position in upper triangle of 52×52 matrix
    let mut i = 0u8;
    let mut j = 1u8;
    let mut count = 0;
    for a in 0..52u8 {
        for b in (a + 1)..52u8 {
            if count == idx {
                return (a, b);
            }
            count += 1;
        }
    }
    (i, j) // shouldn't reach here
}

/// convert two cards to combo index
pub fn cards_to_combo(a: u8, b: u8) -> usize {
    let (lo, hi) = if a < b { (a, b) } else { (b, a) };
    let lo = lo as usize;
    let hi = hi as usize;
    // index = sum of (52-1-k) for k=0..lo-1, plus (hi - lo - 1)
    lo * 52 - (lo * (lo + 1)) / 2 + (hi - lo - 1)
}

/// quick hand strength estimate (0-1) without full MC rollout
fn quick_hand_strength(c0: u8, c1: u8, community: &[u8]) -> f32 {
    if community.len() >= 5 {
        // river: exact evaluation
        let mut comm = [0u8; 5];
        for (i, &c) in community.iter().take(5).enumerate() { comm[i] = c; }
        let score = crate::best_hand_7([c0, c1], &comm);
        // normalize score to 0-1 range (rough)
        (score as f32) / (8 << 20) as f32
    } else {
        // preflop/flop/turn: use bucket as proxy
        let bucket = preflop_bucket(c0, c1);
        bucket as f32 / 168.0
    }
}

/// best 5-card hand from 2 hole + community (re-exported for range equity)
fn best_hand_7_from(hole: [u8; 2], community: &[u8]) -> u32 {
    if community.len() < 5 { return 0; }
    let mut comm = [0u8; 5];
    for (i, &c) in community.iter().take(5).enumerate() { comm[i] = c; }
    crate::best_hand_7(hole, &comm)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_combo_roundtrip() {
        for i in 0..100 {
            let (a, b) = combo_to_cards(i);
            assert!(a < b);
            assert_eq!(cards_to_combo(a, b), i);
        }
        // last combo
        let (a, b) = combo_to_cards(NUM_COMBOS - 1);
        assert_eq!(a, 50);
        assert_eq!(b, 51);
    }

    #[test]
    fn test_range_basics() {
        let mut range = Range::uniform();
        assert_eq!(range.weights.iter().filter(|&&w| w > 0.0).count(), NUM_COMBOS);

        // remove our hole cards (Ah, Kh = cards 12, 24)
        range.remove_cards(&[12, 24]);
        let alive = range.weights.iter().filter(|&&w| w > 0.0).count();
        // should remove all combos containing card 12 or 24
        assert!(alive < NUM_COMBOS);
        assert!(alive > 1000); // most combos survive

        let eff = range.effective_combos();
        println!("effective combos after removing 2 cards: {:.0}", eff);
        assert!(eff > 900.0);
    }

    #[test]
    fn test_range_update() {
        let mut range = Range::uniform();
        range.remove_cards(&[12, 24, 5, 18, 31]); // hole + 3 board cards

        // simulate: opponent raised preflop → strong hands more likely
        let mut likelihoods = [0.1f32; NUM_COMBOS]; // weak hands: 10% chance of raising
        // premium hands: high chance of raising
        for i in 0..NUM_COMBOS {
            let (a, b) = combo_to_cards(i);
            let r0 = a % 13;
            let r1 = b % 13;
            if r0 >= 9 && r1 >= 9 { likelihoods[i] = 0.9; } // broadways
            if r0 == r1 { likelihoods[i] = 0.8; } // pairs
        }

        let before = range.effective_combos();
        range.update(&likelihoods);
        let after = range.effective_combos();

        println!("effective combos: {:.0} → {:.0} after raise", before, after);
        assert!(after < before, "range should narrow after update");
    }
}
