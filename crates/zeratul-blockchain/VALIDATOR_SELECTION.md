# Validator Selection via Token-Weighted Voting

## Overview

Zeratul uses **Nominated Proof-of-Stake (NPoS)** with **Phragmén election** to select 15 validators from a permissionless candidate pool. Any ZT token holder can participate in validator selection.

**Date**: 2025-11-12
**Status**: Design phase

---

## Architecture

### Three-Tier Network Participation

```
┌─────────────────────────────────────────────────────────────────┐
│                        Network Participants                      │
├─────────────────────────────────────────────────────────────────┤
│                                                                   │
│  Tier 1: Light Clients (Mobile, Browser)                        │
│  ├─> Verify Ligerito proofs (22ms, ~10 KB)                     │
│  ├─> Trust FROST threshold (11/15 validators)                   │
│  └─> No stake required                                           │
│                                                                   │
│  Tier 2: Full Nodes (Anyone with bandwidth)                     │
│  ├─> Download ZODA encodings (4 MB/block)                      │
│  ├─> Re-execute state transitions in PolkaVM                    │
│  ├─> Generate own Ligerito proofs                               │
│  ├─> Can challenge invalid blocks (fraud proofs)                │
│  └─> No stake required                                           │
│                                                                   │
│  Tier 3: Nominators (ZT token holders)                          │
│  ├─> Vote for validator candidates                              │
│  ├─> Nominate up to 16 candidates                               │
│  ├─> Share in validator rewards                                 │
│  └─> Risk slashing if validator misbehaves                       │
│                                                                   │
│  Tier 4: Validators (15 selected via Phragmén)                 │
│  ├─> Generate state transitions                                 │
│  ├─> Sign with FROST (Byzantine threshold 11/15)                │
│  ├─> Earn block rewards + fees                                  │
│  ├─> Share rewards with nominators                              │
│  └─> Risk slashing for misbehavior                              │
│                                                                   │
└─────────────────────────────────────────────────────────────────┘
```

---

## Nominated Proof-of-Stake (NPoS)

### Roles

#### **Nominators** (Passive stakers)
- Hold ZT tokens
- Nominate up to 16 validator candidates
- Share rewards proportionally to backing
- Share slashing risk

**Minimum stake**: 100 ZT
**Max nominations**: 16 candidates

#### **Validators** (Active operators)
- Run blockchain infrastructure
- Generate and verify state transitions
- Sign blocks with FROST threshold signatures
- Earn commission on rewards (e.g., 10%)

**Minimum stake**: 10,000 ZT
**Validator set size**: 15 (fixed)

---

## Phragmén Election Algorithm

### What is Phragmén?

**Phragmén's method** is a Swedish proportional representation algorithm that selects validators to:
1. **Maximize security**: Ensure balanced backing across validators
2. **Ensure fairness**: Proportional representation of nominator preferences
3. **Optimize decentralization**: No single validator dominates

### Properties

**Maximin Support Optimization**:
```
Maximize: min(backing_validator_1, backing_validator_2, ..., backing_validator_15)

Goal: Maximize the minimum validator backing (strongest weak link)
```

**Balanced Distribution**:
- Nominator votes split across their nominated validators
- Prevents concentration of stake on single validator
- Ensures economic security is evenly distributed

### Example Election

**Candidates** (20 validators want to be selected):
```
Candidate Pool:
Val_A: 50,000 ZT self-stake
Val_B: 30,000 ZT self-stake
Val_C: 20,000 ZT self-stake
... (17 more candidates)
```

**Nominators** (vote with ZT tokens):
```
Nominator_1: 100,000 ZT → [Val_A, Val_B, Val_C]
Nominator_2: 50,000 ZT → [Val_B, Val_D, Val_E]
Nominator_3: 30,000 ZT → [Val_A, Val_F]
... (more nominators)
```

**Phragmén Election** (select 15):
1. **Round 1**: Select validator with most backing → Val_A (150K ZT)
2. **Round 2**: Rebalance nominator votes (reduce Val_A backing)
3. **Round 3**: Select next validator with most remaining votes
4. **Repeat** until 15 validators selected

**Result**:
```
Selected Validators (epoch N):
Val_A: 85,000 ZT backing (balanced down)
Val_B: 80,000 ZT backing
Val_C: 78,000 ZT backing
...
Val_O: 75,000 ZT backing

Average backing: ~80,000 ZT per validator
Minimum backing: 75,000 ZT (maximin property)
```

---

## Election Cycle

### Era Structure

**Era**: 24 hours (43,200 blocks at 2-second block time)

```
Era N (24 hours)
├─> Epoch 1 (4 hours): Validator set from Era N-1 election
├─> Epoch 2 (4 hours): Same validator set
├─> Epoch 3 (4 hours): Same validator set
├─> Epoch 4 (4 hours): Same validator set
├─> Epoch 5 (4 hours): Same validator set
└─> Epoch 6 (4 hours): Same validator set

Era N+1 (24 hours)
└─> New validator set elected based on Era N nominations
```

### Nomination Period

**Continuous nominations**:
- Nominators can change nominations anytime
- Changes take effect in next era (24 hours)
- Prevents gaming/front-running

### Election Timeline

```
Era N (Current)
├─> Block 0-43,199: Validators produce blocks
├─> Block 38,000: Snapshot nominator votes
├─> Block 38,001-42,999: Compute Phragmén election (off-chain)
└─> Block 43,200: Publish Era N+1 validator set (on-chain)

Era N+1 (Next)
└─> Block 43,200: New validator set takes over
```

---

## Economic Parameters

### Staking Requirements

**Validators**:
- Minimum self-stake: 10,000 ZT
- Recommended: 50,000 ZT (competitive backing)

**Nominators**:
- Minimum stake: 100 ZT
- Maximum nominators per validator: Unlimited
- Maximum nominations per nominator: 16

### Reward Distribution

**Block rewards**: 10 ZT per block

**Distribution**:
```
Block reward: 10 ZT
├─> Validator commission: 1 ZT (10%)
└─> Distributed to nominators: 9 ZT (90%)
    ├─> Proportional to backing
    └─> Shared among all nominators of that validator
```

**Example** (Validator with 100K ZT backing):
```
Validator:
├─> Self-stake: 50,000 ZT (50%)
├─> Nominator_A: 30,000 ZT (30%)
└─> Nominator_B: 20,000 ZT (20%)

Block reward: 10 ZT
├─> Validator commission: 1 ZT
├─> Validator share: 9 ZT × 50% = 4.5 ZT
├─> Nominator_A share: 9 ZT × 30% = 2.7 ZT
└─> Nominator_B share: 9 ZT × 20% = 1.8 ZT

Total validator earnings: 1 + 4.5 = 5.5 ZT per block
```

### Slashing Conditions

**Equivocation** (double-signing):
- Penalty: 10% of validator + nominator stake
- Slashed amount burned

**Unavailability** (offline):
- Penalty: 0.1% per missed block
- Max slash: 7% per era

**Oracle manipulation** (price deviation):
- Penalty: 5% of stake
- Detected by reputation system

**Malicious liquidation**:
- Penalty: 20% of stake
- Requires fraud proof from full node

---

## FROST Integration

### Validator Set Changes

When validator set changes (every era):

**Step 1: Distributed Key Generation (DKG)**
```
New validator set (15 validators)
├─> Run ZODA-enhanced DKG protocol
├─> Each validator contributes polynomial
├─> Generate collective FROST keypair
└─> Threshold: 11/15 for Byzantine security
```

**Step 2: Key Rotation**
```
Era N → Era N+1 transition:
├─> Old validators sign handoff message
├─> New validators receive FROST keys
└─> Gradual transition (1 epoch overlap)
```

### Threshold Operations

**Oracle consensus** (8/15 Simple Majority):
```
8 out of 15 validators must sign oracle prices
├─> ZODA VSSS for nonce generation
├─> FROST signature on median price
└─> Published every block
```

**Liquidation execution** (11/15 Byzantine):
```
11 out of 15 validators must approve liquidations
├─> ZODA VSSS for batch commitment
├─> FROST signature on liquidation batch
└─> Executed once per block (if needed)
```

**Governance changes** (13/15 Supermajority):
```
13 out of 15 validators must approve governance
├─> Protocol parameter changes
├─> Validator set size changes
└─> Emergency actions
```

---

## Candidate Registration

### Becoming a Validator Candidate

**Requirements**:
1. Minimum self-stake: 10,000 ZT
2. Public endpoint (for P2P networking)
3. Hardware: 8 cores, 32 GB RAM, 1 TB SSD
4. Uptime: 99%+ availability

**Registration**:
```rust
pub struct ValidatorCandidate {
    /// Public key for consensus (Ed25519)
    pub consensus_key: [u8; 32],

    /// FROST public key (Decaf377)
    pub frost_key: [u8; 32],

    /// Network endpoint
    pub endpoint: SocketAddr,

    /// Self-stake amount
    pub self_stake: u128,

    /// Commission rate (e.g., 10%)
    pub commission: u8,

    /// On-chain identity (optional)
    pub identity: Option<Identity>,
}
```

### Candidate Pool

**Permissionless entry**:
- Any account with 10K ZT can register
- No approval needed
- Candidates compete for nominator votes

**Election eligibility**:
- Must have self-stake ≥ 10,000 ZT
- Must have at least 1 nominator (beyond self)
- Node must be reachable and synced

---

## Nominator Actions

### Nominating Validators

```rust
pub struct Nomination {
    /// Nominator account
    pub nominator: AccountId,

    /// Amount to stake
    pub stake: u128,

    /// Validator candidates (up to 16)
    pub targets: Vec<AccountId>,
}
```

**Phragmén distributes your stake**:
```
Nominator: 10,000 ZT staked
Nominated: [Val_A, Val_B, Val_C]

After Phragmén:
├─> Val_A gets 4,000 ZT of your backing
├─> Val_B gets 3,500 ZT of your backing
└─> Val_C gets 2,500 ZT of your backing

Note: Distribution optimizes for balanced validator set
```

### Reward Claims

**Automatic compounding**:
- Rewards accrue per block
- Auto-staked by default

**Manual withdrawal**:
```rust
pub fn withdraw_rewards(nominator: AccountId) -> u128 {
    // Calculate pending rewards
    let rewards = calculate_nominator_rewards(nominator);

    // Transfer to nominator
    transfer(nominator, rewards);

    rewards
}
```

### Unbonding

**Unbonding period**: 7 days (604,800 blocks)

```
Nominator initiates unbond
├─> Day 0: Funds locked, no longer earn rewards
├─> Day 1-6: Waiting period (protection against slashing)
└─> Day 7: Funds unlocked, can withdraw
```

**Rationale**: 7-day unbonding allows time to:
- Detect and slash malicious validators
- Prevent "rage quit" attacks
- Maintain economic security

---

## Security Considerations

### Sybil Resistance

**Token-weighted voting**:
- Each token = 1 vote
- Sybil accounts don't help (need tokens anyway)

**Minimum stake requirements**:
- Validators: 10,000 ZT (expensive to spam)
- Nominators: 100 ZT (accessible but not free)

### Cartel Resistance

**Phragmén properties**:
- Balanced backing discourages cartels
- No single validator dominates
- Nominators incentivized to diversify

**Slashing**:
- Validators penalized for collusion
- Nominators share slashing risk
- Encourages due diligence

### Long-Range Attacks

**Weak subjectivity**:
- New nodes must sync from trusted checkpoint
- Checkpoint every era (24 hours)
- Social consensus on canonical chain

**Stake-based finality**:
- FROST signatures from 11/15 validators
- Represents majority of economic security
- Irreversible after finalization

---

## Governance Integration

### Parameter Changes

**Adjustable parameters** (13/15 supermajority):
- Validator set size (currently 15)
- Minimum validator stake (currently 10K ZT)
- Minimum nominator stake (currently 100 ZT)
- Commission limits (currently 0-50%)
- Unbonding period (currently 7 days)

**Governance process**:
```
Proposal submitted → Voting period (7 days) → Execution
├─> Any ZT holder can propose (1000 ZT deposit)
├─> Token-weighted voting (1 ZT = 1 vote)
├─> Requires 13/15 validator signatures for execution
└─> Deposit returned if proposal passes
```

### Emergency Actions

**Validator ejection** (11/15 threshold):
- Remove misbehaving validator mid-era
- Slashing applied
- Backup validator promoted

**Chain halt** (13/15 threshold):
- Stop block production in emergency
- Requires off-chain coordination to restart
- Nuclear option (extremely rare)

---

## Implementation Roadmap

### Phase 1: Basic NPoS ✅ (This document)
- [x] Design token-weighted voting system
- [x] Integrate Phragmén election algorithm
- [ ] Implement validator candidate registration
- [ ] Implement nominator staking

### Phase 2: FROST Integration (Next)
- [ ] DKG for validator set changes
- [ ] Key rotation protocol
- [ ] ZODA VSSS for threshold signatures

### Phase 3: Governance (Later)
- [ ] On-chain parameter governance
- [ ] Proposal and voting system
- [ ] Emergency validator ejection

### Phase 4: Optimizations (Future)
- [ ] Fast unbonding for good actors
- [ ] Dynamic validator set size
- [ ] Validator reputation system

---

## Comparison to Other Chains

### Polkadot
- ✅ Uses Phragmén (same as us)
- ✅ NPoS with 16 nominations
- ❌ ~300 validators (we use 15 for efficiency)
- ❌ Parachain architecture (we're monolithic)

### Ethereum 2.0
- ❌ No proportional representation
- ✅ ~1M validators (high decentralization)
- ❌ 32 ETH minimum (high barrier)
- ❌ Simple threshold signatures (we use FROST)

### Cosmos
- ❌ No Phragmén (simple voting)
- ✅ ~150 validators
- ✅ Delegated staking
- ❌ No threshold signatures

**Zeratul's advantages**:
1. **Phragmén election** → Fair, balanced validator selection
2. **FROST threshold sigs** → Byzantine fault tolerance (11/15)
3. **ZODA VSSS** → Malicious security for free
4. **Three-tier participation** → Light/Full/Validator nodes

---

## References

- [Polkadot NPoS](https://wiki.polkadot.network/docs/learn-phragmen)
- [Phragmén's Method](https://en.wikipedia.org/wiki/Phragmen%27s_method)
- [FROST Paper](https://eprint.iacr.org/2020/852.pdf)
- [ZODA Paper](https://angeris.github.io/papers/zoda.pdf)

---

**Status**: Design complete, ready for implementation
**Next**: Implement `blockchain/src/governance/validator_selection.rs`
