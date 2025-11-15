# Zeratul DEX - Based on Penumbra's Battle-Proven Design

## Overview

We're building a **faster, more P2P version of Penumbra** using PolkaVM for superior execution speed.

Penumbra is battle-tested and has solved the hard problems. We copy their design and make it faster.

## Core Design (From Penumbra)

### 1. Batch Auction DEX (MEV-Proof!)

**How it works:**

```
Block N contains swaps:
  Alice: 100 DOT → KSM
  Bob:   400 DOT → KSM
  Charlie: 500 DOT → KSM

Step 1: Aggregate (order doesn't matter!)
  delta_1 = 100 + 400 + 500 = 1000 DOT

Step 2: Execute aggregate against liquidity
  1000 DOT → 1818 KSM = lambda_2

Step 3: Pro-rata distribution
  Clearing price: 1818 / 1000 = 1.818 KSM/DOT

  Alice gets: (100/1000) * 1818 = 181.8 KSM
  Bob gets:   (400/1000) * 1818 = 727.2 KSM
  Charlie gets: (500/1000) * 1818 = 909.0 KSM
```

**Why this eliminates MEV:**

- Leader can reorder swaps any way they want
- Doesn't matter! All aggregate to same `delta_1`
- Everyone gets same clearing price
- No frontrunning possible
- No sandwich attacks possible

**Implementation:** `src/zswap.rs:353` - `execute_batch()`

### 2. Delegation Tokens (delZT per validator)

**From Penumbra's delegation system:**

```rust
// Delegate 10M ZT to validator V
burn(10M ZT)
mint(10M delZT(V)) at rate ψ_v = 1.0

// Exchange rate grows with rewards
epoch 1: ψ_v = 1.1 (10% staking reward)

// Undelegate
burn(10M delZT(V))
mint(11M ZT) at rate ψ_v = 1.1
```

Rewards are **shielded** - hidden in exchange rate until undelegation.

**Implementation:** `src/delegation_tokens.rs`

### 3. Target Staking Ratio (Our Twist)

Penumbra has fixed inflation. We add **target ratio incentive**:

```
Target: 50% staking ratio
Base inflation: 2% per year

APY = inflation / staking_ratio

If 10% staked: APY = 2% / 10% = 20% (encourages staking)
If 50% staked: APY = 2% / 50% = 4%  (equilibrium)
If 100% staked: APY = 2% / 100% = 2% (encourages unstaking)
```

**Implementation:** `src/staking_rewards.rs`

### 4. Superlinear Slashing (From Polkadot)

Penumbra has slashing. We use Polkadot's formula:

```
penalty = min(3 * (k/n)², 1)

1 validator slashed: 0.03% (accident)
30 validators slashed: 27% (coordinated attack!)
60+ validators slashed: 100% (total loss)
```

Slashing affects **exchange rate ψ_v**, not token amount.

**Implementation:** `src/slashing.rs`

### 5. Pooled Staking (sZT)

For users who want simplicity:

```rust
// Delegate to pool (diversified across validators)
burn(1M ZT)
mint(1M sZT) at pool exchange rate

// Pool automatically rebalances
// Shared slashing risk across all validators
```

**Implementation:** `src/staked_pool.rs`

## Our Improvements Over Penumbra

### 1. **PolkaVM Execution (10-100x faster)**

Penumbra uses CosmWasm (slow). We use PolkaVM:

```
Proof generation: 400ms (vs Penumbra's ~seconds)
Proof verification: 512μs (vs Penumbra's ~5ms)
```

This enables **100ms block times** vs Penumbra's 1s+

### 2. **More P2P Architecture**

Penumbra uses Tendermint (centralized validators).

We use **QUIC P2P** with stake-weighted BFT:
- Any node with MIN_STAKE can participate
- No validator set rotation needed
- Pure Byzantine agreement on batch validity

**Implementation:** `src/bft.rs`

### 3. **Simplified Design**

Penumbra has IBC, governance, etc. We focus on:
- Fast DEX execution
- Stake-weighted consensus
- Privacy via ZK proofs

## What We Copy from Penumbra

✅ **Batch auction mechanism** - Battle-proven MEV resistance
✅ **Delegation token design** - Shielded rewards work perfectly
✅ **Burn-mint swap model** - Clean state transitions
✅ **Pro-rata distribution** - Simple and fair
✅ **Concentrated liquidity** - Capital efficient
✅ **Swap proof structure** - ZK privacy that works

## What We Build New

⚡ **PolkaVM execution** - 10-100x faster proofs
⚡ **QUIC P2P networking** - Low latency, built-in encryption
⚡ **Stake-weighted BFT** - No validator set management
⚡ **Target staking ratio** - Better economic incentives
⚡ **Superlinear slashing** - Polkadot's proven approach

## Architecture

```
User Layer
  │
  ├─ Submit SwapIntent (burn X, want Y)
  │
  ▼
P2P Network (QUIC)
  │
  ├─ Gossip swaps to all nodes
  │
  ▼
Consensus Layer (Stake-weighted BFT)
  │
  ├─ Aggregate swaps to delta_1, delta_2
  ├─ Execute batch via PolkaVM
  ├─ Generate proof (400ms)
  ├─ Validators sign if valid
  │
  ▼
Finalization (2/3+ stake)
  │
  ├─ Publish BatchSwapOutputData
  ├─ Users claim pro-rata outputs
  │
  ▼
Settlement
  │
  └─ Mint outputs to user commitments (private!)
```

## File Structure

```
src/
  ├── zswap.rs              # Batch auction DEX (Penumbra model)
  ├── delegation_tokens.rs  # delZT(v) per validator
  ├── staked_pool.rs        # sZT pooled staking
  ├── staking_rewards.rs    # Target ratio inflation
  ├── slashing.rs           # Superlinear penalties
  ├── bft.rs                # Stake-weighted consensus
  ├── privacy.rs            # Pedersen commitments
  ├── zswap_pvm.rs          # PolkaVM swap execution
  ├── gossip.rs             # P2P message broadcasting
  └── consensus.rs          # Block ordering
```

## Performance Targets

| Metric | Target | Penumbra | Improvement |
|--------|--------|----------|-------------|
| Proof generation | 400ms | ~2s | 5x faster |
| Proof verification | 512μs | ~5ms | 10x faster |
| Block time | 100ms | 1-2s | 10-20x faster |
| Network latency | 10-50ms | 50-200ms | 2-4x faster |
| End-to-end swap | <500ms | 2-5s | 4-10x faster |

## Next Steps

1. **Copy more from Penumbra:**
   - Liquidity position management
   - Swap claim proofs
   - Circuit breaker (from their code)
   - Routing logic

2. **Build networking layer:**
   - QUIC transport setup
   - Gossipsub integration
   - Peer discovery
   - Message validation

3. **Integrate PolkaVM:**
   - Swap execution in PVM
   - Proof generation pipeline
   - Batch verification

4. **Testing:**
   - MEV resistance tests
   - Slashing scenarios
   - Network partition tests
   - Performance benchmarks

## References

- Penumbra DEX: `/home/alice/rotko/penumbra/crates/core/component/dex/`
- Penumbra Staking: `/home/alice/rotko/penumbra/crates/core/component/stake/`
- Penumbra Docs: `/home/alice/rotko/penumbra/docs/protocol/`
- Our Implementation: `/home/alice/rotko/zeratul/crates/zeratul-p2p/src/`
