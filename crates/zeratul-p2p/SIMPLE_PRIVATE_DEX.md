# Simple Private P2P DEX

**Goal**: Pure P2P margin trading where participants leak minimal information about positions.

---

## Privacy Model

### What's PUBLIC (must be revealed):
- ✅ Asset types being traded (e.g., DOT/KSM pair)
- ✅ Total batch volume (aggregate of all swaps)
- ✅ Clearing price (computed from batch)

### What's PRIVATE (hidden via cryptography):
- ❌ Individual swap amounts (hidden in batch)
- ❌ User identities (anonymous commitments)
- ❌ Position sizes (encrypted)
- ❌ Liquidation status (only owner knows)
- ❌ PnL (only owner knows)
- ❌ Leverage used (only owner knows)

---

## Architecture (Dead Simple)

```
Trader A                P2P Network (QUIC)              Trader B
   │                           │                           │
   │ 1. Create swap intent     │                           │
   │    (encrypted amount)     │                           │
   ├──────────────────────────►│                           │
   │                           │◄──────────────────────────┤
   │                           │    (encrypted amount)     │
   │                           │                           │
   │                    2. Aggregate batch                 │
   │                       (homomorphic)                   │
   │                           │                           │
   │                    3. Compute price                   │
   │                       (on totals only)                │
   │                           │                           │
   │                    4. Generate proof                  │
   │                       (PolkaVM)                       │
   │                           │                           │
   │◄──────────────────────────┤                           │
   │   5. Verify + sign        │──────────────────────────►│
   │      if correct           │      Verify + sign        │
   │                           │                           │
   │                    6. Finalize when 2/3+ sign         │
   │                           │                           │
   │ 7. Decrypt output         │                           │
   │    (only you can)         │                           │
```

---

## Privacy Techniques

### 1. Encrypted Swap Amounts

```rust
pub struct PrivateSwapIntent {
    /// Trading pair (PUBLIC)
    pub pair: TradingPair,

    /// Direction (PUBLIC)
    pub direction: SwapDirection,

    /// Amount commitment (PRIVATE)
    /// Uses Pedersen commitment: C = amount·G + blinding·H
    pub amount_commitment: [u8; 32],

    /// Range proof (proves amount is reasonable, e.g., < 1 billion)
    pub range_proof: Vec<u8>,

    /// Owner's public key (for output decryption)
    pub owner_pubkey: [u8; 32],
}
```

### 2. Homomorphic Aggregation

```rust
// Sum encrypted amounts WITHOUT decrypting!
pub fn aggregate_encrypted_swaps(
    swaps: &[PrivateSwapIntent],
) -> PedersenCommitment {
    // C_total = Σ C_i = (Σ amounts)·G + (Σ blindings)·H
    swaps.iter()
        .map(|s| s.amount_commitment)
        .fold(PedersenCommitment::zero(), |acc, c| acc + c)
}
```

### 3. Batch Execution (Privacy via Anonymity Set)

```
Block N batch:
- 47 swaps for DOT/KSM
- Total volume: 10,000 DOT → ???,??? KSM
- Clearing price: 2.03 KSM/DOT

Privacy: Your 100 DOT swap is hidden among 47 swaps!
No one knows which swap is yours.
```

### 4. Position Encryption

```rust
pub struct PrivatePosition {
    /// Position ID (PUBLIC)
    pub id: [u8; 32],

    /// Encrypted position data (PRIVATE)
    /// Only owner can decrypt
    pub encrypted_data: Vec<u8>,

    /// Proof of solvency (proves position is valid)
    /// WITHOUT revealing amounts!
    pub solvency_proof: Vec<u8>,
}

// Encrypted data contains:
struct PositionData {
    collateral_amount: u64,
    borrowed_amount: u64,
    leverage: u8,
    entry_price: f64,
}
```

---

## What Participants Learn

### Trader A (you):
- ✅ Your own positions
- ✅ Your PnL
- ✅ Batch clearing prices
- ✅ Total batch volumes
- ❌ Other traders' positions
- ❌ Other traders' identities

### Trader B (others):
- ✅ Batch clearing prices
- ✅ Total batch volumes
- ✅ Number of swaps in batch
- ❌ Your position size
- ❌ Your identity
- ❌ Your PnL

### Observer (blockchain watcher):
- ✅ Trading pairs active
- ✅ Total volumes per pair
- ✅ Clearing prices
- ❌ Individual swap amounts
- ❌ Trader identities
- ❌ Position sizes

---

## Stake-Weighted Consensus (No Traditional Validators!)

### Native Token: ZT (Zeratul Token)

**Why we need it:**
- **Sybil resistance**: Can't create fake identities without stake
- **2/3 BFT threshold**: Based on % of staked ZT, not number of identities
- **Economic security**: Attacking costs real money (slashing)
- **Protocol governance**: Upgrade decisions via stake-weighted voting

**Token economics (Anti-Delegation Design):**

**Problem with traditional PoS**: Rewards for staking → centralization (rich get richer, delegation pools)

**Our solution**: Only bonded tokens track supply growth!

### Delegation tokens (like Penumbra):

1. **Delegate ZT → get delZT(v)**:
   - Burn X ZT
   - Mint Y delZT(validator) where Y = X / ψ_v(epoch)

2. **Exchange rate ψ_v grows silently (shielded rewards)**:
   - Your delZT amount stays **constant**
   - Exchange rate ψ_v increases as inflation is distributed
   - **No visible reward payments** (privacy!)
   - Each validator has own ψ_v (can differ based on uptime/commission)

3. **Undelegate delZT(v) → get ZT (rewards realized)**:
   - Burn Y delZT(v)
   - Mint X ZT where X = Y * ψ_v(epoch)
   - **Only now** do you see your rewards (X > Y)

**Key dynamics (like Penumbra)**:
- **Initial supply**: 100M ZT
- **Inflation**: 2% per year (to stakers only)
- **Target staking ratio**: 50%
- **APY varies**: Based on actual staking ratio

### Target staking ratio (Penumbra-style):

**How it works**:
```
If 10% staked:
  - 2M ZT/year inflation → 10M staked
  - APY = 2M / 10M = 20% (high rewards → encourages staking)

If 50% staked (target):
  - 2M ZT/year inflation → 50M staked
  - APY = 2M / 50M = 4% (equilibrium)

If 100% staked:
  - 2M ZT/year inflation → 100M staked
  - APY = 2M / 100M = 2% (low rewards → encourages unstaking)
```

**Formula**: `APY = base_inflation / staking_ratio`

**Why this prevents centralization**:
- All validators have same APY (based on global ratio, not per-validator)
- No "best validator" competition
- Delegation pools offer same rate as solo staking
- Self-balancing: system pushes toward 50% staked

**Benefits**:
- ✅ Works on any exchange/bridge/chain (normal token)
- ✅ Simple inflation model (like Bitcoin block rewards)
- ✅ Proven design (Penumbra uses this)
- ✅ No demurrage/rebasing complexity

**Example (shielded rewards)**:
```
Year 0:
  Total supply: 100M ZT
  Staking ratio: 50% (50M bonded, 50M unbonded)
  Current APY: 2% / 50% = 4%

  Alice: Delegate 10M ZT → get 10M delZT(v)
         Her balance shows: 10M delZT(v)
  Bob:   Hold 10M ZT unbonded

Year 1:
  Inflation minted: 2M ZT (2% of 100M)
  Total supply: 102M ZT

  What Alice sees:
    - Still shows: 10M delZT(v) (unchanged!)
    - Exchange rate silently updated: ψ_v = 1.04
    - NO visible reward payments (shielded!)

  Alice checks balance:
    - Shows: 10M delZT(v)
    - She doesn't see "400K ZT reward"
    - Rewards are hidden in exchange rate

  Alice undelegates (rewards realized):
    - Burns: 10M delZT(v)
    - Receives: 10M * 1.04 = 10.4M ZT
    - NOW she sees her rewards! ✅

  Bob:
    - Still has 10M ZT
    - Lost 0.2% of network (10M/100M → 10M/102M)
    - Didn't participate in inflation ❌

Privacy benefit:
  - Observers can't see individual reward amounts
  - Only know: "Alice has 10M delZT(v)"
  - Don't know her rewards until she undelegates
  - Even then, can't tell if she just delegated or held for years
```

**Why this prevents centralization**:
- Shielded rewards → privacy (can't see who earns what)
- Validator-specific ψ_v → differs based on uptime/commission
- But no "best validator" guaranteed (uptime varies)
- Users choose based on trust/commission, not APY hype

**Validator commission**:
- Each validator sets commission rate (e.g., 5%)
- Commission taken BEFORE distributing to delegators
- Example:
  ```
  Validator V earns 1000 ZT inflation
  Commission = 5% = 50 ZT (goes to validator)
  Delegators get: 950 ZT (via ψ_v increase)
  ```
- Higher commission → lower returns → fewer delegators
- Market forces keep commission reasonable

---

## Two Staking Options

### Option 1: Direct delegation (delZT per validator)

**Choose your validator**:
- Stake to specific validator → get delZT(v)
- Each validator has own exchange rate ψ_v
- You control which validator (based on commission/trust)
- **If validator slashed**: ψ_v decreases, you lose value

**Slashing mechanism**:
```
Alice delegates 10M ZT to Validator V → gets 10M delZT(V)
  ψ_v = 1.0

Validator V slashed 10%:
  ψ_v *= 0.9  (exchange rate reduced by 10%)
  ψ_v = 0.9

Alice undelegates:
  10M delZT(V) * 0.9 = 9M ZT
  Lost 1M ZT to slashing
```

**Your delZT(v) amount never changes** - only ψ_v changes!

**Best for**:
- Users who want control
- Users who trust specific validator
- Users who can monitor validator performance

### Option 2: Pooled staking (sZT)

**Pool manages validators**:
- Stake to pool → get sZT
- Single exchange rate ψ (not per-validator)
- Pool automatically distributes across all validators
- If ANY validator slashed, ALL sZT holders share loss

**How it works**:
```
Alice stakes 10M ZT → gets 10M sZT
Pool distributes:
  - 5M ZT to Validator A (via delZT(A))
  - 5M ZT to Validator B (via delZT(B))

Validator B slashed 10%:
  - Pool loses 500K ZT (10% of 5M)
  - Exchange rate: ψ = 9.5M / 10M = 0.95
  - Alice unstakes: 10M sZT → 9.5M ZT (lost 500K)
```

**Slashing via exchange rate** (like Penumbra):
- When validator slashed, exchange rate decreases
- ψ_new = ψ_old * (1 - penalty * validator_share)
- Example: 10% slash on 50% of pool = 5% total loss
- ALL sZT holders affected proportionally

**Superlinear slashing** (like Polkadot):
- Single validator offline: Small penalty (~1%)
- Many validators together: Large penalty (coordinated attack)
- Formula: `penalty = min(3 * (k/n)^2, 1)`
  - k = number slashed
  - n = total validators

Examples (out of 100 validators):
```
1 validator offline:    0.03% penalty (accident)
10 validators offline:  3% penalty (suspicious)
30 validators offline:  27% penalty (coordinated attack!)
60+ validators offline: 100% penalty (total loss)
```

Why this works:
- Accidents affect few validators → low penalty
- Attacks affect many validators → high penalty
- Discourages coordination among malicious validators

**Best for**:
- Users who want simplicity (no validator selection)
- Users who want automatic diversification
- DeFi protocols (single token easier to integrate)
- Accepts shared slashing risk for convenience

### How batches finalize:

```rust
pub struct BatchProposal {
    /// Batch number
    pub batch_id: u64,

    /// Encrypted swaps
    pub swaps: Vec<PrivateSwapIntent>,

    /// Proof that batch was executed correctly
    pub execution_proof: PolkaVMProof,

    /// Stake-weighted signatures
    pub signatures: Vec<StakeSignature>,
}

pub struct StakeSignature {
    /// Signer's public key
    pub pubkey: [u8; 32],
    /// Signature
    pub signature: [u8; 64],
    /// Amount of ZT staked
    pub stake_amount: u64,
}

// Finalization rule:
// Batch is valid when signatures represent 2/3+ of total staked ZT
pub fn is_finalized(batch: &BatchProposal, total_stake: u64) -> bool {
    let signed_stake: u64 = batch.signatures.iter()
        .map(|s| s.stake_amount)
        .sum();

    signed_stake >= (total_stake * 2) / 3
}
```

### State Transition: Who runs it?

**Penumbra uses Tendermint/CometBFT**:
- One validator selected as block proposer (leader)
- Leader executes STF, generates proof, proposes block
- All validators verify and vote
- 2/3+ votes → block finalized

**Our options**:

**Option A: Leader-based (like Penumbra)**
- Pro: No duplicate work, proven model
- Con: Leader has MEV opportunity

**Option B: Leaderless (pure P2P)**
- Pro: No MEV (deterministic ordering), more decentralized
- Con: All validators prove (expensive)

**Recommended: Batch Auction (like Penumbra - NO MEV!)**

**How it eliminates MEV**:
1. **Collect all swaps in block** (any order)
2. **Aggregate to batch totals**:
   ```
   All DOT→KSM swaps: Δ₁ = 1000 DOT
   All KSM→DOT swaps: Δ₂ = 500 KSM
   ```
3. **Execute aggregate against liquidity**:
   - Compute single clearing price p
   - ALL swaps get same price!
4. **Pro-rata distribution**:
   ```
   Alice swapped 100 DOT (10% of batch)
   Bob swapped 50 DOT (5% of batch)

   Both get same price p
   Alice gets: 100 * p KSM
   Bob gets: 50 * p KSM
   ```

**Why order doesn't matter** (Penumbra's key insight):
- Leader can order swaps however they want
- Doesn't matter! All swaps batched into single aggregate
- Single clearing price for entire batch
- No frontrunning possible
- No sandwich attacks possible

**State transition is deterministic**:
- Input: Set of swaps (unordered)
- Output: Batch totals Δ (deterministic)
- Clearing price: Function of Δ only
- All validators compute same result!

### Who can be validator?
**Anyone who stakes minimum ZT!**

You don't need permission - just:
1. Hold minimum stake (100 ZT)
2. Run a node
3. Verify batches and sign if correct
4. Receive inflation proportional to stake

### Light participation (discouraged by design!)

**Traditional PoS**: Delegation pools emerge because users want passive rewards
- Rich validators get richer (compounding rewards)
- Users delegate to maximize returns
- Result: Centralization around top validators

**Our design**: NO PASSIVE REWARDS
- If you stake yourself: Maintain % of supply ✅
- If you delegate: Still maintain % of supply ✅
- **BUT**: No extra rewards means no incentive to delegate!

**What should light clients do instead?**
1. **Just hold unstaked** (if planning to trade soon)
   - Lose ~10% per year, but gas-free
   - Make up for it via trading (fees burned → preserves value)

2. **Stake directly** (if holding long-term)
   - Run lightweight node
   - Sign batches when online
   - No need to delegate to a pool

3. **Use a custodial service** (trust model)
   - They stake your tokens
   - You pay them a fee
   - Not recommended (centralization risk)

**Key insight**: Removing rewards removes the incentive to pool capital!

---

## Information Leakage Analysis

### What you MUST reveal (minimum):

1. **Existence of position**
   - Can't hide that you're trading
   - But can hide amounts via encryption

2. **Trading pair**
   - Must reveal DOT/KSM to match with other side
   - But hides which direction (aggregated)

3. **Participation in batch**
   - Must prove you're part of batch to get output
   - But hidden among all other swaps

### What you CAN hide:

1. **Exact amounts**
   - Use Pedersen commitments
   - Only reveal aggregate

2. **Identity**
   - Use anonymous commitments
   - One-time keys per swap

3. **Timing patterns**
   - Randomize submission time
   - Batch every N seconds regardless

4. **Liquidation status**
   - Only owner knows health factor
   - Self-liquidate privately

---

## Privacy vs Functionality Tradeoffs

### Maximum Privacy (but limited functionality):
```rust
// Fully encrypted positions
// Pro: Zero leakage
// Con: Can't do liquidations (no one knows if underwater)
```

### Practical Privacy (recommended):
```rust
// Encrypted individual positions
// Public aggregates (for price discovery)
// Private liquidations (users self-liquidate or lose collateral)

pub struct PracticalPrivacy {
    individual_swaps: Encrypted,      // ✅ Private
    batch_totals: Public,              // ❌ Public (needed for price)
    position_health: EncryptedToOwner, // ✅ Private to owner
    liquidations: SelfExecuted,        // ✅ Owner triggers privately
}
```

### Why this works:
- **Price discovery**: Needs aggregate volumes (public)
- **Individual privacy**: Amounts hidden in batch
- **Self-sovereignty**: You control your liquidations

---

## Implementation Simplicity

### What we need:

1. **zeratul-p2p** (already built!)
   - QUIC gossip
   - Message routing

2. **Pedersen commitments** (simple!)
   ```rust
   use curve25519_dalek::edwards::EdwardsPoint;

   pub fn commit(amount: u64, blinding: Scalar) -> EdwardsPoint {
       amount * G + blinding * H  // One line!
   }
   ```

3. **PolkaVM execution** (already designed!)
   - Deterministic batch execution
   - Proof generation

4. **Simple BFT** (trivial!)
   ```rust
   if signatures.len() >= (traders.len() * 2 / 3) {
       finalize_batch();
   }
   ```

---

## No Validators Needed!

### Traditional DEX:
```
Validators → Sequence trades → Extract MEV 💰
```

### Our DEX:
```
Traders → Batch trades → No MEV (batch execution)
       → Sign batch → No validators needed!
```

---

## Privacy Guarantees

### What an attacker learns:

**Passive observer**:
- Trading pairs
- Batch volumes
- Number of swaps
- ❌ Individual amounts
- ❌ Trader identities

**Active trader**:
- Everything above
- Your own swaps (obviously)
- ❌ Other traders' amounts
- ❌ Other traders' identities

**Colluding traders (< 1/3)**:
- Their combined swaps
- Can subtract from batch total
- But still don't know individual breakdowns of others

---

## Summary

**Core idea**:
1. Encrypt individual swap amounts
2. Aggregate homomorphically
3. Execute batch on totals
4. Prove execution with PolkaVM
5. Everyone signs if correct
6. Finalize when 2/3+ sign

**Privacy**:
- Individual amounts: HIDDEN
- Identities: HIDDEN
- Batch totals: PUBLIC (needed for pricing)
- Position health: HIDDEN (owner only)

**No validators, no staking, no complex consensus.**

Just P2P traders agreeing on batches via cryptographic proofs.

**THIS is what we should build!** 🎯

---

## Economic Security Model

### Three ways to preserve value:

1. **Delegate tokens** (participate in consensus)
   - Delegate ZT → get delZT(v)
   - Exchange rate ψ_v tracks supply growth
   - Maintain % of network over time
   - Must run node and sign batches (or delegate to someone who does)

2. **Trade actively** (use the DEX)
   - Trading fees are BURNED
   - Burns reduce total supply
   - Your unbonded ZT becomes more valuable
   - Can offset the value loss from not bonding

3. **Do nothing** (idle holder)
   - Hold unbonded ZT
   - Lose ~10% of network per year
   - Your ZT amount stays same, but % of supply shrinks
   - Eventually becomes insignificant

### Why this prevents centralization:

**Traditional PoS centralization vectors:**
- Staking rewards compound → rich get richer (10% becomes 11%, becomes 12.1%...)
- Delegation pools emerge to maximize APY
- Users chase highest yield validators
- Top 10 validators control >50% of stake

**Our design removes these vectors:**
- No compounding: delZT amount is constant, only exchange rate changes
- All validators have same exchange rate growth (based on global supply, not validator)
- No APY competition → no incentive to pool capital
- You delegate to participate, not to earn

### Inflation dynamics (target staking ratio):

```
Initial supply: 100M ZT
Inflation:      2% per year (to bonded holders only)
Target ratio:   50% staked

At 10% staked:  APY = 20% (encourages staking)
At 50% staked:  APY = 4% (equilibrium)
At 100% staked: APY = 2% (encourages unstaking for trading)

Bonded delZT:   ψ_v grows from inflation → maintain/gain % of network
Unbonded ZT:    Balance unchanged → slowly lose % of network

Result: Self-balancing system that targets 50% staking ratio
```

**Key insights**:
- Like Penumbra's proven model
- All validators have same APY (no competition)
- System naturally balances toward target ratio
- Unbonded holders slowly diluted (incentive to stake or trade)

### Attack cost:

To attack (sign invalid batches):
- Need 33%+ of bonded ZT (measured in ZT value, not delZT)
- Slashing burns your delZT → permanent loss
- No rewards to recover losses
- Pure economic loss

**Result**: Attacking is expensive, defending is cost-free (just maintain % of supply)
