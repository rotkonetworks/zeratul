# Zidecar v2 Design Notes

## Current Architecture

```
zidecar v1:
├── Header Chain Proofs  → ligerito (gigaproof, tip proof)
├── Nullifier Set        → NOMT (sparse merkle trie)
├── Note Storage         → NOMT
├── Proof Cache          → sled
└── Bridge               → zanchor parachain (Polkadot)
```

## Problem: Do We Need Zanchor?

Zanchor provides:
1. Polkadot-secured checkpoints for Zcash state
2. Finality guarantees from Polkadot validators
3. Cross-chain bridge anchor point

Questions:
- Can ligerito proofs alone provide sufficient trust?
- Is zanchor adding value or just complexity?
- Could we have a simpler trust model?

## Alternative Trust Models

### Option A: Pure Ligerito (No External Anchor)

```
Trust model: "verify the math, not the validators"

Light client receives:
1. Ligerito gigaproof (header chain validity)
2. Ligerito tip proof (recent blocks)
3. Nullifier proofs (NOMT or polynomial commitment)

No zanchor needed if:
- Client trusts ligerito proof system
- Client has a known checkpoint (hardcoded or from trusted source)
```

Pros:
- Simpler architecture
- No Polkadot dependency
- Pure cryptographic trust

Cons:
- Need initial checkpoint from somewhere
- No "consensus" on which chain is canonical (just proof of validity)

### Option B: Decentralized Checkpoint Network

```
Replace zanchor with:
- P2P checkpoint gossip
- Multiple independent attesters
- Social consensus on checkpoints
```

Could use FROST threshold signatures without a parachain.

### Option C: Bitcoin Timestamping

```
Like Babylon Protocol:
- Timestamp Zcash checkpoints to Bitcoin
- Bitcoin provides ordering/finality
- No Polkadot needed
```

### Option D: Keep Zanchor, Simplify Role

```
Zanchor only stores:
- Epoch boundary hashes
- Aggregated ligerito proofs

Light clients:
- Fetch checkpoints from zanchor OR hardcoded
- Verify everything else with ligerito
```

## Storage Layer Improvements

### Current: NOMT for Nullifiers

```rust
// O(log N) writes per nullifier
batch_insert_nullifiers(&nullifiers, block_height)
```

### Future: QMDB (When Available)

```rust
// O(1) writes per nullifier
// Historical proofs for free
// "When was this nullifier revealed?" built-in
```

Why QMDB fits:
- Nullifiers are append-only (once spent, always spent)
- Need historical provability for bridges
- High write volume during sync

### Alternative: Polynomial Commitment for Nullifiers

```
Instead of Merkle (NOMT/QMDB):
- Commit to nullifier set as polynomial
- Prove membership/non-membership in ZK
- Single proof system (ligerito) for everything
```

Benefits:
- Unified proof system
- Potentially smaller proofs
- ZK-native (can hide which nullifier being checked)

Tradeoff:
- More complex
- Polynomial commitment setup
- May be overkill for Zcash throughput

## Proof System Consolidation

### Current: Mixed System

```
Header chain  → ligerito proof
Nullifier     → NOMT merkle proof
Bridge        → two separate proofs to verify
```

### Proposed: Unified Ligerito

```
Everything in one proof:
1. Header chain valid (N blocks)
2. Nullifier set root at block N
3. Specific nullifier membership

zanchor (or any verifier) checks ONE proof
```

Benefits:
- Single verification
- Proof aggregation
- Simpler bridge logic

## MMR vs MMB Considerations

If using MMR/MMB for any accumulator (epochs, headers):

| Structure | Proof size (recent) | Append time |
|-----------|---------------------|-------------|
| MMR       | O(log n) - grows forever | O(log n) |
| MMB       | O(log k) - based on recency | O(1) |

For bridges where you mostly prove recent state, MMB is better.

## Concrete Proposal: Zidecar v2

### Real-Time Relay Model (Recommended)

The key insight: **post every Zcash block to zanchor using on-demand coretime**.

On Kusama with on-demand coretime:
- Zcash block time: ~75 seconds
- Cost per block: few cents
- Monthly cost: ~$30-50 for full relay
- Attack window: ~75 seconds (vs 7 days with epoch-only)

#### Cumulative Chainwork in Proofs

**Why chainwork matters:**

Ligerito proves "this chain follows the rules" but NOT "this is the canonical chain".
A malicious zidecar could create a valid proof for a minority fork.

Solution: Include cumulative chainwork in every proof. Fork choice = most work.

```rust
/// Posted to zanchor for every Zcash block
struct BlockCheckpoint {
    height: u32,
    header_hash: [u8; 32],
    prev_hash: [u8; 32],
    cumulative_work: U256,       // total PoW accumulated (proven!)
    nullifier_root: [u8; 32],    // NOMT/QMDB root at this height
    timestamp: u64,
}

/// Ligerito tip proof covers:
/// 1. Header validity (PoW, merkle root, etc)
/// 2. Link to previous block (prev_hash check)
/// 3. Cumulative work calculation (prev_work + block_work)
/// 4. Nullifier set transition (if nullifiers in block)
struct TipProof {
    checkpoint: BlockCheckpoint,
    proof: Vec<u8>,              // ligerito proof of all above
}
```

#### Zanchor Runtime Logic

```rust
#[pallet::call]
impl<T: Config> Pallet<T> {
    /// Submit a new Zcash block checkpoint
    /// Anyone can call - proof is the authority, not the caller
    pub fn submit_block(
        origin: OriginFor<T>,
        checkpoint: BlockCheckpoint,
        tip_proof: Vec<u8>,
    ) -> DispatchResult {
        // 1. Verify ligerito proof (covers validity + chainwork calc)
        ensure!(
            verify_tip_proof(&tip_proof, &checkpoint),
            Error::<T>::InvalidProof
        );

        // 2. Check it extends current chain
        let prev = Blocks::<T>::get(checkpoint.height - 1)
            .ok_or(Error::<T>::MissingParent)?;
        ensure!(
            checkpoint.prev_hash == prev.header_hash,
            Error::<T>::InvalidPrevHash
        );

        // 3. Fork choice: only accept if more cumulative work
        ensure!(
            checkpoint.cumulative_work > prev.cumulative_work,
            Error::<T>::InsufficientWork
        );

        // 4. Store checkpoint
        Blocks::<T>::insert(checkpoint.height, checkpoint);
        LatestHeight::<T>::put(checkpoint.height);

        Ok(())
    }

    /// Handle reorgs: submit competing chain with more work
    pub fn submit_reorg(
        origin: OriginFor<T>,
        checkpoints: Vec<BlockCheckpoint>,
        proofs: Vec<Vec<u8>>,
        fork_height: u32,
    ) -> DispatchResult {
        // Verify all proofs in competing chain
        // Accept if total cumulative_work > current chain at same height
        // Replace blocks from fork_height onward
        todo!()
    }
}
```

#### Security Properties

```
Attack: Malicious zidecar submits fake chain
Defense: cumulative_work must exceed real chain
Result: Attacker needs 51% of Zcash hashpower (same as attacking Zcash)

Attack: Zidecar withholds blocks
Defense: Anyone can submit blocks (permissionless)
Result: Run your own zidecar, or use multiple providers

Attack: Zidecar lies about nullifiers
Defense: nullifier_root is committed in proof
Result: Merkle proof against on-chain root exposes lie
```

#### Architecture Diagram

```
┌─────────────────────────────────────────────────────────────┐
│                      zidecar v2                              │
├─────────────────────────────────────────────────────────────┤
│  zebrad ──► Header Sync ──► Tip Prover ──► zanchor          │
│               │                  │            (Kusama)       │
│               │                  │               │           │
│               ▼                  ▼               ▼           │
│         NOMT/QMDB          TipProof with    BlockCheckpoint │
│        (nullifiers)        cumulative_work   (on-chain)     │
│               │                                  │           │
│               └──────────────┬───────────────────┘           │
│                              ▼                               │
│                    Light Client Verifies:                    │
│                    1. Fetch checkpoint from zanchor          │
│                    2. Verify nullifier proof vs root         │
│                    3. Trust = Zcash PoW + ligerito math      │
└─────────────────────────────────────────────────────────────┘
```

### Minimal Version (Drop Zanchor)

For use cases where zanchor is overkill:

```
┌─────────────────────────────────────────┐
│              zidecar v2 minimal          │
├─────────────────────────────────────────┤
│  Header Chain    │ ligerito gigaproof   │
│  Tip Blocks      │ ligerito tip proof   │
│  Nullifier Set   │ NOMT (now) / QMDB    │
│  Trust Anchor    │ hardcoded checkpoint │
│                  │ + ligerito proofs    │
└─────────────────────────────────────────┘
```

No zanchor. Light clients trust:
1. Hardcoded checkpoint (in app/wallet)
2. Ligerito proofs from that checkpoint forward
3. Query multiple zidecars and compare cumulative_work

### Epoch-Only Zanchor (Cheaper, Less Secure)

If per-block posting is too expensive:

```
┌─────────────────────────────────────────┐
│          zidecar v2 epoch-only          │
├─────────────────────────────────────────┤
│  Header Chain    │ ligerito gigaproof   │
│  Tip Blocks      │ ligerito tip proof   │
│  Nullifier Set   │ QMDB (when ready)    │
│  Trust Anchor    │ zanchor (epoch only) │
│  Post Frequency  │ ~weekly (8192 blocks)│
└─────────────────────────────────────────┘
```

Zanchor stores epoch boundaries only:
- Lower cost (~4 posts/month)
- Larger attack window (~7 days)
- Still has cumulative_work fork choice

## Implementation Notes: Cumulative Chainwork

### Zcash Chainwork Calculation

```rust
/// Zcash uses equihash PoW, chainwork is sum of difficulties
/// difficulty = 2^256 / target
fn calculate_block_work(header: &BlockHeader) -> U256 {
    let target = header.bits.to_target();
    U256::MAX / target
}

fn calculate_cumulative_work(
    prev_cumulative: U256,
    header: &BlockHeader,
) -> U256 {
    prev_cumulative + calculate_block_work(header)
}
```

### Ligerito Circuit for Chainwork

The tip proof circuit must verify:

```
Inputs (public):
  - prev_header_hash
  - prev_cumulative_work
  - new_header_hash
  - new_cumulative_work
  - new_nullifier_root

Witness (private):
  - full block header
  - nullifiers in block
  - previous nullifier root

Constraints:
  1. hash(header) == new_header_hash
  2. header.prev_hash == prev_header_hash
  3. verify_equihash(header)  // PoW valid
  4. block_work = 2^256 / header.target
  5. new_cumulative_work == prev_cumulative_work + block_work
  6. nullifier_transition(prev_root, nullifiers) == new_nullifier_root
```

### Genesis Bootstrapping

Zanchor needs a starting point. Options:

1. **Hardcode Orchard activation** (height 1687104)
   - Known checkpoint, no trust assumptions
   - Cumulative work at that point is public knowledge

2. **Genesis sync**
   - Start from Zcash genesis, prove entire chain
   - More work but fully trustless

```rust
// In zanchor genesis config
GenesisCheckpoint {
    height: 1687104,  // Orchard activation
    header_hash: ORCHARD_ACTIVATION_HASH,
    cumulative_work: ORCHARD_ACTIVATION_WORK,  // precomputed
    nullifier_root: EMPTY_ROOT,  // no Orchard nullifiers yet
}
```

## Open Questions

1. **Per-block vs epoch posting?**
   - Per-block: ~$30-50/month on Kusama, 75s attack window
   - Epoch-only: ~$2/month, 7 day attack window
   - Recommendation: Start per-block, can relax later if costs matter

2. **Reorg handling depth?**
   - How many blocks of history to keep on zanchor?
   - Zcash reorgs are rare (< 10 blocks typically)
   - Could prune anything older than 100 blocks

3. **QMDB timeline?**
   - When will it be available as Rust crate?
   - NOMT works fine, QMDB is optimization for append-only nullifiers

4. **Proof size vs verification time?**
   - Tip proofs are small (~few KB)
   - On-chain verification cost on Kusama?
   - May need to benchmark ligerito verifier in WASM

5. **Multiple zidecar coordination?**
   - If multiple zidecars submit same block, first wins
   - No coordination needed - proof is authority
   - Could add small reward for first valid submission?

## Sovereign Zanchor with Commonware Consensus

### Motivation

Instead of relying on Polkadot validators (who don't care about Zcash),
make zanchor validators = ZEC bridgers. Skin in the game.

### Commonware-Style Consensus

Use Commonware's simplex consensus with:
- Threshold BLS signatures
- Epoch-based validator rotation
- Proactive DKG resharing every epoch

Key insight from Commonware: treat blockchain as sequence of epochs,
reshare threshold key at epoch boundaries when all validators have
synchronized view of finalized state.

### Governance Model: ConvictionVoting + Phragmen

Use Polkadot's battle-tested governance primitives:

1. **ConvictionVoting** - lock ZEC longer for more voting power
2. **Democracy pallet** - referenda for parameter changes
3. **Phragmen election** - proportional DKG committee selection

#### Conviction Multipliers

```rust
/// Based on pallet_conviction_voting
enum Conviction {
    None,      // 0.1x voting power, no lock
    Locked1x,  // 1x voting power, 1 epoch lock
    Locked2x,  // 2x voting power, 2 epoch lock
    Locked3x,  // 3x voting power, 4 epoch lock
    Locked4x,  // 4x voting power, 8 epoch lock
    Locked5x,  // 5x voting power, 16 epoch lock
    Locked6x,  // 6x voting power, 32 epoch lock
}

fn voting_power(zec_amount: u64, conviction: Conviction) -> u64 {
    let multiplier = match conviction {
        Conviction::None => 0.1,
        Conviction::Locked1x => 1.0,
        Conviction::Locked2x => 2.0,
        Conviction::Locked3x => 3.0,
        Conviction::Locked4x => 4.0,
        Conviction::Locked5x => 5.0,
        Conviction::Locked6x => 6.0,
    };
    (zec_amount as f64 * multiplier) as u64
}
```

#### Phragmen DKG Election

```rust
/// Every epoch, elect DKG committee via sequential Phragmen
struct DkgElection {
    candidates: Vec<ValidatorCandidate>,  // people wanting to be in DKG
    nominators: Vec<Nominator>,           // ZEC holders backing candidates
    seats: u32,                           // DKG committee size (e.g., 10)
}

struct Nominator {
    account: AccountId,
    zec_locked: u64,
    conviction: Conviction,
    nominees: Vec<ValidatorCandidate>,    // who they're backing (up to 16)
}

/// Phragmen ensures proportional representation:
/// - Minority groups can pool votes to get a seat
/// - Whales can't take all seats
/// - Optimizes for fairness of representation
fn run_phragmen_election(election: DkgElection) -> Vec<ElectedValidator> {
    // Sequential Phragmen algorithm
    // Each seat filled by candidate with most "unused" backing
    // Backing gets "used up" as candidates win seats
    // Result: proportional representation
    sp_npos_elections::seq_phragmen(
        election.seats,
        election.candidates,
        election.nominators,
    )
}
```

#### Example Election

```
DKG committee: 10 seats

Candidates:
┌──────────┬─────────────────────────────────────────────────┐
│ Validator│ Backing                                         │
├──────────┼─────────────────────────────────────────────────┤
│ Alice    │ 1 whale: 5000 ZEC × 0.1 (no lock) = 500 votes   │
│ Bob      │ 50 users: 100 ZEC × 6x (32 epoch) = 30,000 votes│
│ Carol    │ 10 users: 500 ZEC × 4x (8 epoch) = 20,000 votes │
│ Dave     │ 5 users: 1000 ZEC × 2x (2 epoch) = 10,000 votes │
└──────────┴─────────────────────────────────────────────────┘

Phragmen result:
  Bob:   4 seats (strongest community backing)
  Carol: 3 seats (strong committed backing)
  Dave:  2 seats (medium backing)
  Alice: 1 seat (whale gets representation, but not dominance)

DKG shares distributed proportionally to seats:
  Bob:   40% of threshold key
  Carol: 30% of threshold key
  Dave:  20% of threshold key
  Alice: 10% of threshold key
```

#### Why Phragmen > Pure Stake-Weighted

```
Pure stake-weighted DKG:
┌─────────────────────────────────────────────────┐
│ Whale with 50% of ZEC = 50% of DKG shares       │
│ Can potentially collude with 1-2 others         │
│ Small holders have no voice                     │
└─────────────────────────────────────────────────┘

Phragmen-elected DKG:
┌─────────────────────────────────────────────────┐
│ Whale gets seats proportional to backing        │
│ But committed small holders pool together       │
│ Long-term lockers (conviction) outweigh whales  │
│ More diverse committee = harder to collude      │
└─────────────────────────────────────────────────┘
```

#### Governance via Democracy Pallet

ZEC holders can vote on:

```rust
enum Proposal {
    // DKG parameters
    SetCommitteeSize(u32),
    SetThresholdPercent(u8),
    SetEpochLength(u32),

    // Bridge parameters
    SetMinimumBridgeAmount(u64),
    SetWithdrawalDelay(u32),

    // Emergency
    PauseBridge,
    ResumeBridge,

    // Upgrades
    RuntimeUpgrade(Vec<u8>),
}

// Proposals pass with conviction-weighted majority
// Higher conviction = more say in governance
```

### Epoch Lifecycle

```
Epoch N                          Epoch N+1
─────────────────────────────────────────────────────────►
│                                │
│  1. Process blocks             │  1. New validator set active
│  2. Accept bridge-in txs       │  2. New threshold key in use
│  3. Track ZEC balances         │  3. Old validators can't sign
│                                │
└── Epoch boundary ──────────────┘
    - Snapshot ZEC holdings
    - Calculate new share weights
    - Run proactive DKG reshare
    - Derive new group polynomial
```

### Validator Registration Flow

```
┌─────────────────┐     ┌─────────────────┐     ┌─────────────────┐
│   Zcash Chain   │     │    Zidecar      │     │    Zanchor      │
└────────┬────────┘     └────────┬────────┘     └────────┬────────┘
         │                       │                       │
         │  Lock ZEC to          │                       │
         │  bridge script        │                       │
         │───────────────────────►                       │
         │                       │                       │
         │                       │  Generate ligerito    │
         │                       │  proof of lock        │
         │                       │───────────────────────►
         │                       │                       │
         │                       │                       │  Verify proof
         │                       │                       │  Add to pending
         │                       │                       │  validators
         │                       │                       │
         │                       │         ◄─────────────│
         │                       │         Epoch boundary│
         │                       │         DKG includes  │
         │                       │         new validator │
         │                       │                       │
```

### Bootstrap Problem & Solution

**Problem:** Need validators to verify bridge-in, but validators ARE bridge-in users.

**Solution:** Trusted genesis committee + migration

```rust
// Phase 1: Genesis (trusted)
GenesisConfig {
    initial_validators: vec![
        // Trusted operators run DKG ceremony
        TrustedValidator { pubkey, initial_shares: 10 },
        TrustedValidator { pubkey, initial_shares: 10 },
        TrustedValidator { pubkey, initial_shares: 10 },
    ],
    migration_threshold: 1000_00000000, // 1000 ZEC bridged
}

// Phase 2: Migration (after threshold)
// Once 1000 ZEC bridged, transition to ZEC-weighted shares
// Genesis validators can bridge ZEC to keep participating
// Or exit gracefully over N epochs
```

### Withdrawal & Unbonding

```rust
fn request_withdrawal(amount: u64) -> Result<()> {
    // Can't withdraw immediately - need to reshare first
    // Queue withdrawal for next epoch boundary
    PendingWithdrawals::insert(caller, amount);
    Ok(())
}

// At epoch boundary:
// 1. Process pending withdrawals
// 2. Reduce their share weight
// 3. Reshare excludes withdrawn stake
// 4. Generate withdrawal proof for Zcash side
```

### Security Properties

```
Attack: Malicious validator tries to sign fake blocks
Defense: Need 67% of shares (67% of staked ZEC)
Result: Attacker needs to bridge massive ZEC (expensive)

Attack: Validator withdraws ZEC, keeps old shares
Defense: Proactive resharing - old shares don't work in new epoch
Result: Can't use stale key material

Attack: Sybil - split into many small validators
Defense: Share weight based on total ZEC, not validator count
Result: Same voting power whether 1 account or 100

Attack: Long-range attack with old keys
Defense: Checkpoints + Zcash PoW (cumulative_work)
Result: Can't rewrite history without Zcash hashpower
```

### Comparison: Polkadot vs Sovereign

| Aspect | Polkadot Parachain | Sovereign Zanchor |
|--------|-------------------|-------------------|
| Validators | DOT stakers (don't care) | ZEC bridgers (skin in game) |
| Security | Polkadot shared security | ZEC stake + Zcash PoW |
| Cost | Coretime fees | Self-sustained |
| Complexity | Substrate runtime | Commonware + custom |
| Decentralization | Depends on DOT distribution | Depends on ZEC bridgers |

### Open Design Questions

1. **Minimum stake?**
   - Too low: many small validators, DKG overhead
   - Too high: plutocracy, few validators
   - Suggestion: 100 ZEC minimum (share_unit)

2. **Epoch length?**
   - Too short: frequent resharing overhead
   - Too long: slow validator rotation
   - Suggestion: ~1 hour (48 Zcash blocks)

3. **Threshold percentage?**
   - 67%: standard BFT assumption (tolerates 33% malicious)
   - Higher: more security, but harder to reach quorum
   - Suggestion: 67%

4. **Data availability?**
   - Fully sovereign: validators store everything
   - Hybrid: use Kusama for DA, sovereign consensus
   - Suggestion: Start hybrid, go sovereign later

5. **What if not enough validators?**
   - Fallback to trusted set?
   - Pause bridge?
   - Suggestion: Minimum 5 validators, pause if fewer

## References

- QMDB paper: https://arxiv.org/abs/2501.05262
- MMB paper: https://arxiv.org/abs/2511.13582
- NOMT: https://github.com/thrumdev/nomt
- Commonware/QMDB: https://github.com/LayerZero-Labs/qmdb
- Commonware resharing: https://blog.commonware.xyz/p/once-a-validator
- From Permissioned to PoS paper: https://arxiv.org/abs/2310.11431
