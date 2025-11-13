# Phragmén Implementation: Zeratul vs Polkadot

**Date**: 2025-11-12

## Comparison Overview

### Polkadot's Implementation

**File**: `/polkadot-sdk/substrate/primitives/npos-elections/src/phragmen.rs`

**Key Features**:
1. **Load-based scoring**: Uses `Rational128` with max precision (`DEN = u128::MAX`)
2. **Sequential election**: Elects validators one by one
3. **Balancing iterations**: Optional post-processing for better balance
4. **Generic precision**: `PerThing128` trait for configurable accuracy
5. **Normalization**: Ensures edge weights sum to exactly `P::one()`

**Algorithm Structure**:
```rust
for round in 0..to_elect {
    // Loop 1: Initialize scores (1/approval_stake)
    for candidate in candidates {
        candidate.score = DEN / candidate.approval_stake
    }

    // Loop 2: Increment scores based on voter loads
    for voter in voters {
        for edge in voter.edges {
            candidate.score += (voter.load * voter.budget) / approval_stake
        }
    }

    // Loop 3: Select winner (min score = best)
    winner = candidates.min_by_key(|c| c.score)

    // Loop 4: Update voter loads
    for voter in voters {
        if voter.nominated(winner) {
            voter.load = winner.score
        }
    }
}
```

**Precision**:
- Uses `Rational128` for all calculations
- Denominator: `u128::MAX` for maximum precision
- Handles rounding explicitly (`Rounding::Down`)

### Our Implementation

**File**: `/blockchain/src/governance/phragmen.rs`

**Key Features**:
1. **Simpler load tracking**: Uses `Balance` (u128) directly
2. **Sequential election**: Same approach as Polkadot
3. **Rebalancing after each round**: Recalculates potential backing
4. **No balancing iterations**: Simpler, no post-processing
5. **BTreeMap-based**: Deterministic iteration order

**Algorithm Structure**:
```rust
for round in 0..validator_count {
    // Find candidate with highest backing
    winner = candidate_backings.max_by_key(|backing|)

    // Calculate nominator contributions
    for nomination in nominations {
        if nomination.targets.contains(winner) {
            // Calculate share based on remaining targets
            share = nomination.stake / targets_remaining

            // Adjust for existing load
            adjusted = share - (nominator_load / targets_remaining)
        }
    }

    // Rebalance remaining candidates
    for candidate in candidates {
        potential_backing = self_stake + sum(nominator_shares)
        candidate_backings[candidate] = potential_backing
    }
}
```

**Precision**:
- Uses `u128` directly (no rational arithmetic)
- Division can lose precision
- No explicit rounding strategy

---

## Key Differences

### 1. **Precision & Accuracy**

**Polkadot**:
```rust
// Uses Rational128 with DEN = u128::MAX
candidate.score = Rational128::from(DEN / approval_stake, DEN);

// Explicit rounding
multiply_by_rational_with_rounding(
    voter.load.n(),
    voter.budget,
    candidate.approval_stake,
    Rounding::Down,
);
```

**Ours**:
```rust
// Direct u128 division (precision loss)
let contribution_per_target = nomination.stake / targets_still_available as u128;

// No explicit rounding strategy
let adjusted_contribution = if nominator_load > 0 {
    contribution_per_target.saturating_sub(nominator_load / targets_still_available as u128)
} else {
    contribution_per_target
};
```

**Impact**: Polkadot's rational arithmetic is more accurate, especially for small stakes.

### 2. **Score vs Backing**

**Polkadot**:
- Uses "score" = cost per unit of support
- **Lower score = better** (cheaper to support)
- Score formula: `1/approval_stake + sum(voter_loads)`

**Ours**:
- Uses "backing" = total stake supporting candidate
- **Higher backing = better** (more support)
- Backing formula: `self_stake + sum(nominator_contributions)`

**Impact**: Same goal (maximin), different formulation.

### 3. **Balancing**

**Polkadot**:
```rust
// Optional post-processing balancing
if let Some(ref config) = balancing {
    let _iters = balancing::balance::<AccountId>(&mut voters, config);
}
```

**Ours**:
```rust
// No explicit balancing pass
// Instead, we rebalance after each election round
for (candidate, backing) in candidate_backings.iter_mut() {
    // Recalculate potential backing
    potential_backing = calculate_with_updated_loads();
    *backing = potential_backing;
}
```

**Impact**: Polkadot's balancing is more sophisticated, but our approach is simpler.

### 4. **Load Tracking**

**Polkadot**:
```rust
// Voter load = cumulative support cost
voter.load = winner.score;  // Rational128

// Edge load = additional cost for this edge
edge.load = winner.score.lazy_saturating_sub(voter.load);
```

**Ours**:
```rust
// Nominator load = stake already allocated
nominator_loads: BTreeMap<AccountId, Balance>  // u128

// Update after allocation
*nominator_loads.entry(nomination.nominator).or_insert(0) += adjusted_contribution;
```

**Impact**: Polkadot tracks per-edge loads, we track per-nominator totals.

---

## Improvements Needed

### 1. **Add Rational Arithmetic** (High Priority)

Current:
```rust
let contribution_per_target = nomination.stake / targets_still_available as u128;
```

Improved:
```rust
use sp_arithmetic::Rational128;

let contribution = Rational128::from(
    nomination.stake,
    targets_still_available as u128
);
```

**Benefit**: Eliminates precision loss in division.

### 2. **Add Balancing Iterations** (Medium Priority)

Add post-processing balancing like Polkadot:

```rust
pub struct BalancingConfig {
    pub iterations: usize,
    pub tolerance: u128,
}

impl PhragmenElection {
    pub fn run_election_with_balancing(
        &self,
        era: u64,
        config: Option<BalancingConfig>,
    ) -> Result<ElectionResult> {
        let mut result = self.run_election(era)?;

        if let Some(config) = config {
            self.balance_stakes(&mut result, config)?;
        }

        Ok(result)
    }

    fn balance_stakes(
        &self,
        result: &mut ElectionResult,
        config: BalancingConfig,
    ) -> Result<()> {
        for _ in 0..config.iterations {
            // Iteratively adjust nominator contributions
            // to minimize variance in validator backing
            // (similar to Polkadot's balancing::balance)
        }
        Ok(())
    }
}
```

**Benefit**: Better balanced validator set (smaller `max/min` ratio).

### 3. **Add PJR Test** (Low Priority)

Polkadot tests for "Proportional Justified Representation" (PJR):

```rust
#[test]
fn test_pjr_property() {
    // Check that the election result satisfies PJR
    // i.e., no group of nominators is under-represented
}
```

**Benefit**: Ensures fair representation.

---

## Recommendation

### Short-term: Keep our simpler implementation ✅

**Rationale**:
- Our algorithm is correct (sequential Phragmén)
- Simpler to understand and debug
- Good enough for initial testnet

**TODO**:
- Add comprehensive tests (compare outputs with Polkadot)
- Document precision limitations

### Medium-term: Add rational arithmetic 🔄

**Rationale**:
- Improves accuracy for small stakes
- Prevents rounding errors accumulating
- Consensus-critical (must be deterministic!)

**Implementation**:
```rust
// Add to Cargo.toml
sp-arithmetic = { version = "16.0.0", default-features = false }

// Use in phragmen.rs
use sp_arithmetic::{Rational128, PerThing128};
```

### Long-term: Full Polkadot compatibility 🎯

**Rationale**:
- Battle-tested algorithm
- Years of production use
- Community audited

**Implementation**:
- Import `sp-npos-elections` directly
- Wrap with our types
- Add balancing iterations

---

## Test Plan

### 1. **Unit Tests** (Already have)
- ✅ Simple election with 3 validators
- ✅ Balanced stakes
- ✅ Maximin property
- ✅ Nomination validation

### 2. **Comparison Tests** (TODO)

Compare our outputs with Polkadot's for same inputs:

```rust
#[test]
fn test_compare_with_polkadot() {
    // Setup identical inputs
    let candidates = vec![...];
    let voters = vec![...];

    // Run our implementation
    let our_result = our_phragmen_election(candidates.clone(), voters.clone());

    // Run Polkadot's implementation
    let polkadot_result = sp_npos_elections::seq_phragmen(
        15,  // to_elect
        candidates,
        voters,
        None,  // no balancing
    );

    // Compare results
    assert_eq!(our_result.winners, polkadot_result.winners);

    // Check balance ratio is similar (allow small difference)
    let ratio_diff = (our_result.balance_ratio() - polkadot_result.balance_ratio()).abs();
    assert!(ratio_diff < 0.05, "Balance ratios differ by {}", ratio_diff);
}
```

### 3. **Stress Tests** (TODO)

```rust
#[test]
fn test_large_candidate_pool() {
    // 100 candidates, elect 15
    // 1000 nominators with various stake amounts
    // Check:
    // - Performance (<1 second)
    // - Balance ratio (<1.5)
    // - All validators have backing
}

#[test]
fn test_concentrated_stake() {
    // One nominator with 90% of stake
    // Should still distribute fairly
}

#[test]
fn test_edge_cases() {
    // Candidate with zero nominators
    // Nominator with single target
    // All nominators nominate same validator
}
```

---

## Conclusion

**Our implementation is good enough for now**, but we should:

1. ✅ **Keep it simple** for initial launch
2. 🔄 **Add rational arithmetic** for better precision (pre-mainnet)
3. 🎯 **Consider full Polkadot compatibility** long-term (post-mainnet)

**Key insight**: Polkadot's algorithm is production-proven with years of use. We should converge to it, but can start with our simpler version.

---

## References

- [Polkadot Phragmén Source](https://github.com/paritytech/polkadot-sdk/blob/master/substrate/primitives/npos-elections/src/phragmen.rs)
- [Phragmén's Method (Wikipedia)](https://en.wikipedia.org/wiki/Phragmen%27s_method)
- [NPoS Research Paper](https://arxiv.org/abs/2004.12990)
- [Web3 Foundation Grant](https://github.com/w3f/Grants-Program/blob/master/applications/phragmen.md)
