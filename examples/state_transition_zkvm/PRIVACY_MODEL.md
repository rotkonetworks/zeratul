# Privacy Model: Preventing Position Hunting

## Threat Model

### Primary Attack: Bot Position Hunting

**Attack Vector:**
```
1. Bot monitors on-chain data
2. Identifies large leveraged positions
3. Detects positions near liquidation
4. Front-runs liquidations OR trades against position
5. Profits from:
   - Liquidation penalties (5-10%)
   - Price manipulation of overleveraged positions
   - Front-running position closes
```

**Real Example:**
```
❌ Without Privacy:
Bot sees: Position A has $1M, 10x leverage, 85% LTV
→ Price drops 5%
→ Bot front-runs liquidation
→ Steals $50k penalty

✅ With Our Privacy:
Bot sees: "47 positions executed, $5M volume"
→ Cannot identify which positions are large
→ Cannot track health factors
→ Cannot front-run specific liquidations
```

## What We Reveal (Public)

### Level 1: Batch Aggregates Only

**Per Trading Pair, Per Block:**
```json
{
  "trading_pair": "UM/gm",
  "block_height": 12345,
  "num_orders": 47,
  "total_long_volume": "10000 UM",
  "total_short_volume": "8000 UM",
  "clearing_price": "1.05 gm/UM",
  "total_borrowed_from_pool": "150000 UM"
}
```

**Pool State (Aggregate):**
```json
{
  "asset": "UM",
  "total_supplied": "5000000 UM",
  "total_borrowed": "3750000 UM",
  "utilization": "75%",
  "borrow_rate": "12% APR",
  "supply_rate": "9% APR"
}
```

**Liquidations (Aggregate):**
```json
{
  "block_height": 12345,
  "num_liquidated": 3,
  "total_volume": "25000 UM",
  "average_penalty": "5%",
  "returned_to_pool": "26250 UM"
}
```

**What Bots Can Learn:**
- ✅ Market sentiment (longs vs shorts)
- ✅ Overall liquidity
- ✅ Pool utilization trends
- ❌ Individual position sizes
- ❌ Who owns positions
- ❌ Position health factors

## What We Hide (Private)

### Level 2: Individual Positions (Encrypted)

**Stored in NOMT:**
```rust
struct EncryptedPosition {
    commitment: [u8; 32],        // Random-looking hash
    nullifier: [u8; 32],         // For updates, unlinkable
    validity_proof: ZKProof,     // Proves valid without revealing data
    ciphertext: Vec<u8>,         // Only owner can decrypt
}
```

**Only Owner Knows:**
```json
// Decrypted with viewing key
{
  "collateral": [
    {"asset": "UM", "amount": "1000"}
  ],
  "debt": [
    {"asset": "UM", "amount": "2000"}
  ],
  "health_factor": 1.25,
  "entry_price": "1.02 gm/UM",
  "leverage": "3x",
  "pnl": "+250 UM"
}
```

**Privacy Guarantees:**
- ❌ Bots cannot see position size
- ❌ Bots cannot see health factor
- ❌ Bots cannot link positions to users
- ❌ Bots cannot detect when position closes
- ✅ Only owner can decrypt their position

## Privacy Techniques

### 1. Commitment Scheme

```
Position → Hash(owner_key || data || randomness) → Commitment

Properties:
- Hiding: Cannot reverse commitment to see data
- Binding: Cannot change data after commitment
- Randomized: Same position → different commitment each time
```

### 2. Nullifier System

```
Update Position:
1. Spend old commitment (burn nullifier)
2. Create new commitment (fresh randomness)
3. ZK proof links old → new without revealing data

Result:
- Unlinkable: Cannot tell old and new are same position
- No double-spend: Nullifier ensures one-time use
```

### 3. Batch Aggregation

```
Block N contains:
- Order 1: 100 UM @ 3x (hidden)
- Order 2: 500 UM @ 5x (hidden)
- Order 3: 50 UM @ 2x (hidden)
- ... 44 more orders

Public Output:
- Total: 650 UM executed (aggregate only)
- Clearing Price: 1.05 gm/UM (same for all)
- No individual order data leaked
```

### 4. Private Liquidation Detection

```
Traditional (Vulnerable):
1. Bot queries: getHealthFactor(position) → 0.95
2. Bot front-runs liquidation

Our Approach (Private):
1. Validators check health factors locally
2. Submit ZK proof: "I know N liquidatable positions"
3. Aggregate proofs → batch liquidation
4. Only reveal: "3 positions liquidated, 25k volume"
5. Bots cannot identify WHICH positions
```

## Attack Resistance

### Attack 1: Liquidation Sniping ✅ PREVENTED

**Without Privacy:**
```
Bot: getPosition(0x123) → health: 0.95
Bot: Oh! About to liquidate
Bot: sendTx(liquidate(0x123)) with high gas
Profit: Front-runs honest liquidators
```

**With Privacy:**
```
Bot: getPosition(0x123) → encrypted blob
Bot: Cannot decode health factor
Bot: Cannot tell if liquidatable
Result: Must wait for batch liquidation (fair)
```

### Attack 2: Position Hunting ✅ PREVENTED

**Without Privacy:**
```
Bot: See position A: $1M long, 10x leverage
Bot: Price drops 3%
Bot: Trade against position (push price down more)
Bot: Force liquidation
Profit: Manipulate large positions
```

**With Privacy:**
```
Bot: See batch: 47 orders, $5M volume
Bot: Cannot identify large positions
Bot: Cannot tell who is overleveraged
Result: Cannot target specific positions
```

### Attack 3: Unwinding Detection ✅ PREVENTED

**Without Privacy:**
```
Bot: See position A closing 500 UM
Bot: Implies profit-taking or stop-loss
Bot: Front-run with same trade
Profit: Free alpha from position flow
```

**With Privacy:**
```
Bot: See batch result only
Bot: Cannot identify individual closes
Bot: Cannot front-run specific orders
Result: Batch execution absorbs all flow fairly
```

## Comparison: Privacy Levels

| System | Individual Orders | Position Health | Liquidations | TVL |
|--------|------------------|-----------------|--------------|-----|
| **GMX V1** | 🔴 Public | 🔴 Public | 🔴 Public | 🟡 Aggregate |
| **GMX V2** | 🔴 Public | 🔴 Public | 🟡 Delayed | 🟡 Aggregate |
| **Aave** | 🔴 Public | 🔴 Public | 🔴 Public | 🟡 Aggregate |
| **dYdX V4** | 🟡 Off-chain | 🔴 Public | 🔴 Public | 🟡 Aggregate |
| **Penumbra DEX** | 🟢 Private | N/A | N/A | 🟡 Aggregate |
| **Zeratul (Our)** | 🟢 Private | 🟢 Private | 🟢 Private | 🟡 Aggregate |

## Trade-offs

### What We Sacrifice

**Transparency:**
- Cannot verify individual position health publicly
- Cannot track specific user activity
- Harder to audit individual liquidations

**Composability:**
- Other contracts cannot read position data
- Limited integration with transparent DeFi

### What We Gain

**MEV Protection:**
- ✅ No liquidation sniping
- ✅ No position hunting
- ✅ No front-running

**Fair Markets:**
- ✅ Batch execution (order doesn't matter)
- ✅ Same price for everyone
- ✅ Liquidations at fair auction prices

**User Safety:**
- ✅ Large positions cannot be targeted
- ✅ Whales cannot be hunted
- ✅ Overleveraged users not front-run

## Implementation Strategy

### Phase 1: Commitment-Based Positions
```rust
✅ Encrypt position data
✅ Store commitments in NOMT
✅ Only owner can decrypt
✅ Nullifiers for updates
```

### Phase 2: Private Batch Execution
```rust
✅ Aggregate orders
✅ Hide individual sizes
✅ Only emit batch results
✅ Fair clearing price
```

### Phase 3: Private Liquidations
```rust
🔜 ZK proofs of health < 1.0
🔜 Anonymous liquidation set
🔜 Batch liquidation auction
🔜 Only aggregate revealed
```

### Phase 4: Viewing Keys
```rust
🔜 Owner decrypts own position
🔜 Optional sharing with trusted parties
🔜 Auditor keys for compliance
🔜 Emergency recovery
```

## Conclusion

**Privacy is NOT about hiding malicious activity.**

**Privacy is about:**
- Preventing bots from hunting your position
- Ensuring fair liquidations
- Eliminating toxic MEV
- Protecting large traders from manipulation

**Our model: "Penumbra-level privacy for margin trading"**

- ✅ Batch aggregates public (needed for price discovery)
- ✅ Individual positions private (prevents hunting)
- ✅ Only owner sees their health factor
- ✅ Fair liquidations with no front-running

This is the **only way** to build a margin trading protocol that doesn't get eaten alive by MEV bots.
