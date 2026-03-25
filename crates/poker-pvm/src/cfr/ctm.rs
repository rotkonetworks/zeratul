//! CTM-MoE: Continuous Thought Machine with Mixture of Experts
//!
//! replaces blueprint lookup at search leaf nodes. instead of a hash table
//! (exact for seen states, garbage for unseen), a learned value function
//! that generalizes across the game tree.
//!
//! # architecture
//!
//! ```text
//! input features
//!   ├── board texture (13 floats: rank counts, suit counts, connectedness)
//!   ├── pot geometry (3 floats: pot/stack ratio, SPR, bet/pot ratio)
//!   ├── stack depths (2 floats: hero/villain effective stacks in BB)
//!   ├── range summary (4 floats: range equity, range polarization, top%, bottom%)
//!   ├── street (4 one-hot: preflop/flop/turn/river)
//!   └── position (1 float: IP=1, OOP=0)
//!       │
//!       ▼
//!   router network (27 → 6 softmax)
//!       │
//!       ├──► expert 0: dry boards    (~1M params, CTM with 2-8 steps)
//!       ├──► expert 1: wet boards    (~1M params)
//!       ├──► expert 2: deep stacks   (~1M params)
//!       ├──► expert 3: short stacks  (~1M params)
//!       ├──► expert 4: multiway      (~1M params)
//!       └──► expert 5: river polar   (~1M params)
//!             │
//!             ▼  (top-2 experts, weighted)
//!       output: (value: f32, action_probs: [f32; NUM_ACTIONS])
//! ```
//!
//! # CTM thinking budget
//!
//! each expert is a continuous thought machine: it can iterate its hidden
//! state for a variable number of steps. easy decisions (clear value bet
//! on river) converge in 2 steps. complex decisions (multi-draw board
//! with balanced ranges) take 8 steps.
//!
//! the halting probability is learned — the model decides when it's
//! "thought enough." this matches poker: some spots are trivial,
//! others require deep reasoning about range interactions.
//!
//! # training
//!
//! 1. generate positions from self-play (blueprint + search)
//! 2. for each position: run deep search (10K iterations) → "true" value
//! 3. train CTM-MoE to predict values from state features
//! 4. loss = MSE(predicted_value, search_value) + KL(predicted_policy, search_policy)
//! 5. auxiliary loss: halting penalty (encourage fewer thinking steps)

/// number of input features to the router and experts
pub const NUM_FEATURES: usize = 27;

/// number of discrete actions the model outputs probabilities for
pub const NUM_ACTIONS: usize = 9; // fold, check, call, bet_25, bet_50, bet_75, bet_100, bet_200, allin

/// number of experts in the MoE
pub const NUM_EXPERTS: usize = 6;

/// max thinking steps for CTM
pub const MAX_THINK_STEPS: usize = 8;

/// top-K experts activated per decision
pub const TOP_K: usize = 2;

// ---------------------------------------------------------------------------
// Feature extraction (exact, no learning)
// ---------------------------------------------------------------------------

/// extracted features from a game state for CTM input
#[derive(Clone, Debug)]
pub struct StateFeatures {
    pub features: [f32; NUM_FEATURES],
}

/// board texture features (13 floats)
/// - rank histogram (13 values, 0-4 each, normalized to 0-1)
/// nah — too sparse. better decomposition:
///
/// board texture (13 floats):
///   [0]  high card rank (0-12 → 0-1)
///   [1]  num pairs on board (0-2 → 0-1)
///   [2]  num trips on board (0-1)
///   [3]  board paired? (0/1)
///   [4]  monotone? (0/1)
///   [5]  two-tone? (0/1)
///   [6]  rainbow? (0/1)
///   [7]  straight possible? (0/1)
///   [8]  flush possible? (0/1)
///   [9]  num connected cards (within 2 ranks) / board_size
///   [10] num overcards to top pair / 12
///   [11] board wetness score (0-1, composite)
///   [12] num community cards / 5
pub fn extract_board_texture(community: &[u8]) -> [f32; 13] {
    let mut f = [0.0f32; 13];
    if community.is_empty() { return f; }

    let mut ranks = [0u8; 13];
    let mut suits = [0u8; 4];
    let mut max_rank = 0u8;

    for &card in community {
        let rank = card % 13;
        let suit = card / 13;
        ranks[rank as usize] += 1;
        suits[suit as usize] += 1;
        if rank > max_rank { max_rank = rank; }
    }

    f[0] = max_rank as f32 / 12.0;

    let pairs = ranks.iter().filter(|&&c| c == 2).count();
    let trips = ranks.iter().filter(|&&c| c >= 3).count();
    f[1] = pairs as f32 / 2.0;
    f[2] = trips as f32;
    f[3] = if pairs > 0 || trips > 0 { 1.0 } else { 0.0 };

    let max_suit = *suits.iter().max().unwrap_or(&0);
    f[4] = if max_suit >= community.len() as u8 { 1.0 } else { 0.0 }; // monotone
    f[5] = if max_suit == community.len() as u8 - 1 { 1.0 } else { 0.0 }; // two-tone
    f[6] = if max_suit <= 1 && community.len() >= 3 { 1.0 } else { 0.0 }; // rainbow

    // straight possible: any window of 5 consecutive ranks with >= 3 present
    let mut straight_possible = false;
    for start in 0..9 {
        let count: u8 = (start..start+5).map(|r| if ranks[r] > 0 { 1 } else { 0 }).sum();
        if count >= 3 { straight_possible = true; break; }
    }
    // wheel check (A-2-3-4-5)
    if ranks[12] > 0 {
        let low: u8 = (0..4).map(|r| if ranks[r] > 0 { 1 } else { 0 }).sum();
        if low >= 2 { straight_possible = true; }
    }
    f[7] = if straight_possible { 1.0 } else { 0.0 };

    // flush possible: any suit with >= 3 cards
    f[8] = if suits.iter().any(|&s| s >= 3) { 1.0 } else { 0.0 };

    // connectedness: count pairs of cards within 2 ranks
    let mut connected = 0;
    for i in 0..community.len() {
        for j in i+1..community.len() {
            let r1 = community[i] % 13;
            let r2 = community[j] % 13;
            let diff = if r1 > r2 { r1 - r2 } else { r2 - r1 };
            if diff <= 2 { connected += 1; }
        }
    }
    f[9] = connected as f32 / community.len().max(1) as f32;

    // overcards to board
    f[10] = (12 - max_rank) as f32 / 12.0;

    // composite wetness
    f[11] = (f[7] * 0.3 + f[8] * 0.3 + f[9] * 0.2 + (1.0 - f[3]) * 0.2).min(1.0);

    f[12] = community.len() as f32 / 5.0;

    f
}

/// pot geometry features (3 floats)
pub fn extract_pot_geometry(pot: u32, hero_stack: u32, villain_stack: u32, current_bet: u32, big_blind: u32) -> [f32; 3] {
    let effective = hero_stack.min(villain_stack);
    let spr = if pot > 0 { effective as f32 / pot as f32 } else { 20.0 };
    let pot_stack_ratio = if effective > 0 { pot as f32 / effective as f32 } else { 0.0 };
    let bet_pot_ratio = if pot > 0 { current_bet as f32 / pot as f32 } else { 0.0 };

    [
        pot_stack_ratio.min(2.0) / 2.0,     // normalized to 0-1
        (spr / 20.0).min(1.0),              // SPR capped at 20
        bet_pot_ratio.min(3.0) / 3.0,       // bet/pot capped at 3x
    ]
}

/// stack depth features (2 floats)
pub fn extract_stacks(hero_stack: u32, villain_stack: u32, big_blind: u32) -> [f32; 2] {
    let bb = big_blind.max(1);
    [
        ((hero_stack / bb) as f32 / 200.0).min(1.0),     // hero BB, capped at 200
        ((villain_stack / bb) as f32 / 200.0).min(1.0),  // villain BB
    ]
}

/// range summary features (4 floats)
/// these come from the Bayesian range tracker (L2)
pub fn extract_range_summary(
    range_equity: f32,
    range_polarization: f32,
    top_pct: f32,
    bottom_pct: f32,
) -> [f32; 4] {
    [range_equity, range_polarization, top_pct, bottom_pct]
}

/// street one-hot (4 floats)
pub fn extract_street(community_count: u8) -> [f32; 4] {
    match community_count {
        0 => [1.0, 0.0, 0.0, 0.0],
        3 => [0.0, 1.0, 0.0, 0.0],
        4 => [0.0, 0.0, 1.0, 0.0],
        5 => [0.0, 0.0, 0.0, 1.0],
        _ => [0.0, 0.0, 0.0, 0.0],
    }
}

/// position (1 float)
pub fn extract_position(is_in_position: bool) -> f32 {
    if is_in_position { 1.0 } else { 0.0 }
}

/// combine all features into one vector
pub fn extract_all(
    community: &[u8],
    pot: u32,
    hero_stack: u32,
    villain_stack: u32,
    current_bet: u32,
    big_blind: u32,
    range_equity: f32,
    range_polarization: f32,
    top_pct: f32,
    bottom_pct: f32,
    is_in_position: bool,
) -> StateFeatures {
    let mut features = [0.0f32; NUM_FEATURES];

    let board = extract_board_texture(community);
    features[0..13].copy_from_slice(&board);

    let pot_geo = extract_pot_geometry(pot, hero_stack, villain_stack, current_bet, big_blind);
    features[13..16].copy_from_slice(&pot_geo);

    let stacks = extract_stacks(hero_stack, villain_stack, big_blind);
    features[16..18].copy_from_slice(&stacks);

    let range = extract_range_summary(range_equity, range_polarization, top_pct, bottom_pct);
    features[18..22].copy_from_slice(&range);

    let street = extract_street(community.len() as u8);
    features[22..26].copy_from_slice(&street);

    features[26] = extract_position(is_in_position);

    StateFeatures { features }
}

// ---------------------------------------------------------------------------
// Expert routing (which experts handle this state?)
// ---------------------------------------------------------------------------

/// determine which expert(s) should evaluate this state.
/// returns (expert_index, weight) pairs, top-K.
///
/// this is the "hard" router — rule-based, no learning needed.
/// the feature extraction already tells us the situation type.
pub fn route(features: &StateFeatures) -> [(usize, f32); TOP_K] {
    let f = &features.features;

    let board_wetness = f[11];
    let spr = f[14] * 20.0; // denormalize
    let is_river = f[25] > 0.5;
    let community_count = (f[12] * 5.0) as u8;
    // TODO: multiway detection (need player count in features)

    let mut scores = [0.0f32; NUM_EXPERTS];

    // expert IDs aligned with Python train_experts_v2.py:
    // 0=headsup, 1=preflop_multi, 2=postflop_wet, 3=postflop_dry, 4=shortstack, 5=river_polar

    // expert 0: headsup (always active in HU, low weight in multiway)
    scores[0] = 0.3; // baseline for HU

    // expert 1: preflop_multi (preflop, no community cards)
    scores[1] = if community_count == 0 { 0.7 } else { 0.0 };

    // expert 2: postflop wet boards (high wetness, draws)
    scores[2] = if community_count > 0 { board_wetness * 0.4 + f[7] * 0.3 + f[8] * 0.3 } else { 0.0 };

    // expert 3: postflop dry boards (low wetness, paired, rainbow)
    scores[3] = if community_count > 0 { (1.0 - board_wetness) * 0.5 + f[3] * 0.3 + f[6] * 0.2 } else { 0.0 };

    // expert 4: short stacks (SPR < 8, i.e. effective stack < 8x pot)
    // covers push/fold territory through medium stack-to-pot situations
    scores[4] = if spr < 8.0 { (8.0 - spr) / 8.0 } else { 0.0 };

    // expert 5: river polarization
    scores[5] = if is_river { 0.8 } else { 0.0 };

    // pick top-K
    let mut indices: Vec<(usize, f32)> = scores.iter().copied().enumerate().collect();
    indices.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));

    // normalize top-K weights
    let total = indices[0].1 + indices[1].1;
    let w0 = if total > 0.0 { indices[0].1 / total } else { 0.5 };
    let w1 = 1.0 - w0;

    [(indices[0].0, w0), (indices[1].0, w1)]
}

// ---------------------------------------------------------------------------
// CTM expert (placeholder for learned model)
// ---------------------------------------------------------------------------

/// output from one expert
#[derive(Clone, Debug)]
pub struct ExpertOutput {
    pub value: f32,
    pub action_probs: [f32; NUM_ACTIONS],
    pub think_steps: u8,
}

/// a single CTM expert.
/// in production: ONNX model loaded via tract or candle.
/// for now: returns blueprint-derived heuristic.
pub struct Expert {
    pub id: usize,
    pub name: &'static str,
}

impl Expert {
    /// placeholder: evaluate using feature heuristics.
    /// replaced by neural network after training.
    pub fn evaluate(&self, features: &StateFeatures) -> ExpertOutput {
        // placeholder: uniform action distribution, neutral value
        ExpertOutput {
            value: 0.0,
            action_probs: [1.0 / NUM_ACTIONS as f32; NUM_ACTIONS],
            think_steps: 2,
        }
    }
}

/// the full MoE ensemble
pub struct MoEEnsemble {
    pub experts: Vec<Expert>,
}

impl Default for MoEEnsemble {
    fn default() -> Self {
        Self {
            experts: vec![
                Expert { id: 0, name: "headsup" },
                Expert { id: 1, name: "preflop_multi" },
                Expert { id: 2, name: "postflop_wet" },
                Expert { id: 3, name: "postflop_dry" },
                Expert { id: 4, name: "shortstack" },
                Expert { id: 5, name: "river_polar" },
            ],
        }
    }
}

impl MoEEnsemble {
    /// evaluate a state using top-K experts
    pub fn evaluate(&self, features: &StateFeatures) -> ExpertOutput {
        let routing = route(features);

        let mut value = 0.0f32;
        let mut action_probs = [0.0f32; NUM_ACTIONS];
        let mut max_steps = 0u8;

        for &(expert_idx, weight) in &routing {
            let output = self.experts[expert_idx].evaluate(features);
            value += weight * output.value;
            for i in 0..NUM_ACTIONS {
                action_probs[i] += weight * output.action_probs[i];
            }
            max_steps = max_steps.max(output.think_steps);
        }

        ExpertOutput { value, action_probs, think_steps: max_steps }
    }
}

// ---------------------------------------------------------------------------
// Training data generation
// ---------------------------------------------------------------------------

/// a training sample: state features → (value, policy) from deep search
#[derive(Clone, Debug)]
pub struct TrainingSample {
    pub features: StateFeatures,
    /// ground truth value from deep CFR search
    pub target_value: f32,
    /// ground truth action distribution from deep search
    pub target_policy: [f32; NUM_ACTIONS],
    /// which expert(s) should handle this (from router)
    pub expert_routing: [(usize, f32); TOP_K],
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_extract_board_texture_flop() {
        // Ah Ks 7d = cards 12, 38+12=50..  actually card encoding: rank + suit*13
        // Ah = 12 (ace of clubs in 0-indexed), but let's use: rank*4+suit or rank+suit*13
        // the crate uses card % 13 = rank, card / 13 = suit
        // so Ah = 12 + 3*13 = 51, Ks = 11 + 2*13 = 37, 7d = 5 + 1*13 = 18
        let community = [51, 37, 18]; // Ah Ks 7d
        let f = extract_board_texture(&community);

        assert!(f[0] > 0.9, "high card should be ace (12/12)");
        assert_eq!(f[3], 0.0, "no pairs on board");
        assert_eq!(f[6], 1.0, "rainbow board");
        assert_eq!(f[4], 0.0, "not monotone");
        assert!(f[12] - 0.6 < 0.01, "3/5 community cards");
    }

    #[test]
    fn test_extract_board_texture_wet() {
        // Jh Th 9h = flush draw + straight draw
        let community = [10 + 3*13, 9 + 3*13, 8 + 3*13]; // J T 9 all hearts
        let f = extract_board_texture(&community);

        assert_eq!(f[4], 1.0, "monotone");
        assert_eq!(f[7], 1.0, "straight possible");
        assert!(f[11] > 0.5, "wet board");
    }

    #[test]
    fn test_routing_dry_vs_wet() {
        // dry board: Kh 7d 2c
        let dry = extract_all(
            &[11 + 3*13, 5 + 1*13, 0 + 0*13],
            100, 1000, 1000, 0, 10,
            0.5, 0.3, 0.1, 0.1, true,
        );
        let dry_route = route(&dry);

        // wet board: Jh Th 9h
        let wet = extract_all(
            &[10 + 3*13, 9 + 3*13, 8 + 3*13],
            100, 1000, 1000, 0, 10,
            0.5, 0.3, 0.1, 0.1, true,
        );
        let wet_route = route(&wet);

        // dry board should prefer expert 3 (postflop_dry)
        assert_eq!(dry_route[0].0, 3, "dry board should route to expert 3");
        // wet board should prefer expert 2 (postflop_wet)
        assert_eq!(wet_route[0].0, 2, "wet board should route to expert 2");
    }

    #[test]
    fn test_routing_short_stack() {
        let features = extract_all(
            &[11 + 3*13, 5 + 1*13, 0 + 0*13],
            200, 50, 50, 0, 10, // SPR = 50/200 = 0.25
            0.5, 0.3, 0.1, 0.1, true,
        );
        let routing = route(&features);

        // should prefer expert 4 (shortstack)
        let has_short = routing.iter().any(|&(idx, _)| idx == 4);
        assert!(has_short, "short stack should route to expert 4");
    }

    #[test]
    fn test_moe_ensemble() {
        let ensemble = MoEEnsemble::default();
        let features = extract_all(
            &[51, 37, 18], // Ah Ks 7d
            100, 1000, 1000, 0, 10,
            0.5, 0.3, 0.1, 0.1, true,
        );
        let output = ensemble.evaluate(&features);

        // placeholder returns uniform distribution
        assert!(output.action_probs.iter().all(|&p| (p - 1.0/6.0).abs() < 0.01));
        assert_eq!(output.think_steps, 2);
    }
}
