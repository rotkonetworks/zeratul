# Security Hardening Analysis - Additional Attack Vectors

## Executive Summary

With Oracle MEV mitigations complete (🟢 LOW RISK), we now analyze additional attack vectors and hardening opportunities.

**Current Status**:
- ✅ Oracle MEV: **MITIGATED** (HIGH → LOW)
- ⚠️ Other attack vectors: **NEED ANALYSIS**

---

## Attack Vector Analysis

### 1. 🔴 Liquidation Timing Attacks (HIGH PRIORITY)

**Threat**: Attackers monitor private positions and time liquidations to maximize penalty extraction.

**Current State**:
- Liquidations proven via ZK proofs (which positions hidden ✅)
- No timing restrictions (when liquidations happen ❌)
- 5% penalty to liquidators (may incentivize timing attacks ❌)

**Attack Scenario**:
```
1. Attacker monitors on-chain commitments (can't decrypt, but sees count)
2. Price drops → attacker knows SOME positions became liquidatable
3. Attacker rushes to submit liquidation proofs first
4. Extracts maximum 5% penalty before others can liquidate
```

**Risk**: 🔴 **MEDIUM-HIGH**
- Privacy preserved (which positions unknown ✅)
- But first-mover advantage exists (timing attack ❌)
- Could lead to liquidation bot races

**Proposed Mitigations**:
1. **Liquidation Delay** (1-2 blocks after becoming liquidatable)
2. **Penalty Decay** (5% → 1% over 10 blocks)
3. **Liquidation Queuing** (FIFO processing, no front-running)
4. **Partial Liquidations** (liquidate minimum needed, not entire position)

---

### 2. 🟡 Position Size Concentration Risk (MEDIUM PRIORITY)

**Threat**: Single user opens massive leveraged position, systemic risk if liquidated.

**Current State**:
- No position size limits ❌
- Up to 20x leverage allowed ❌
- No circuit breakers ❌

**Attack Scenario**:
```
1. Whale deposits $10M collateral
2. Opens 20x leveraged long ($200M notional)
3. Price drops 6% → position liquidated
4. Entire lending pool drained to cover losses
5. Protocol insolvent
```

**Risk**: 🟡 **MEDIUM**
- Could destabilize entire protocol
- Single liquidation could cascade
- No protection against concentrated positions

**Proposed Mitigations**:
1. **Max Position Size** (% of pool TVL)
2. **Max Notional Exposure** (per user, per asset)
3. **Leverage Tiers** (higher leverage = lower max size)
4. **Circuit Breakers** (pause trading if liquidations > threshold)

---

### 3. 🟡 Oracle Manipulation via Validator Collusion (MEDIUM PRIORITY)

**Threat**: Byzantine validators collude to manipulate oracle prices.

**Current State**:
- Median of validator proposals (2/3+ required ✅)
- No outlier detection beyond spread check ❌
- No validator reputation system ❌

**Attack Scenario**:
```
1. 2/3+ validators collude (Byzantine threshold)
2. All submit fake price (e.g., 2x real price)
3. Median becomes fake price
4. Liquidate all shorts, or enable bad longs
5. Profit from liquidation penalties
```

**Risk**: 🟡 **MEDIUM**
- Requires 2/3+ validator collusion (hard ✅)
- But if successful, devastating (complete oracle control ❌)
- Current spread check (5%) insufficient for 2x manipulation

**Proposed Mitigations**:
1. **Tighter Spread Limits** (1-2% max, not 5%)
2. **External Oracle Fallback** (Chainlink, Pyth as backup)
3. **Validator Slashing** (stake loss if caught manipulating)
4. **Time-Weighted Average Price (TWAP)** (harder to manipulate)

---

### 4. 🟡 DoS via Spam Transactions (MEDIUM PRIORITY)

**Threat**: Attacker floods network with invalid orders to prevent legitimate trades.

**Current State**:
- No transaction fees yet ❌
- No rate limiting per user ❌
- No proof-of-work for transactions ❌

**Attack Scenario**:
```
1. Attacker generates 10,000 fake margin orders
2. All have invalid ZK proofs (but still need verification)
3. Block producers spend all time verifying invalid proofs
4. Legitimate orders never get processed
5. Network halts
```

**Risk**: 🟡 **MEDIUM**
- Could prevent trading during critical times
- Relatively cheap attack (no fees)
- Mitigated by proof verification (takes time to generate fakes)

**Proposed Mitigations**:
1. **Transaction Fees** (spam becomes expensive)
2. **Proof-of-Work** (small PoW per transaction)
3. **Rate Limiting** (max orders per address per block)
4. **Priority Queue** (higher fees = faster execution)

---

### 5. 🟢 Privacy Leakage via Timing Analysis (LOW-MEDIUM PRIORITY)

**Threat**: Attacker correlates order submission timing with price movements to deanonymize.

**Current State**:
- Orders revealed when submitted ✅ (timing visible)
- Batch execution hides individual fills ✅
- But submission timing can leak info ❌

**Attack Scenario**:
```
1. Attacker monitors when commitments appear on-chain
2. Correlates with known whale wallet activity on Penumbra
3. "This commitment appeared 2 seconds after whale deposited 100M UM"
4. Infers this is likely the whale's position
5. Hunts it for liquidation
```

**Risk**: 🟢 **LOW-MEDIUM**
- Requires sophisticated correlation analysis
- Only works for very large whales
- Mitigated by viewing keys (only owner knows true state)

**Proposed Mitigations**:
1. **Transaction Batching** (accumulate orders, submit as batch)
2. **Random Delays** (0-5 second random delay before submission)
3. **Decoy Transactions** (fake commitments to add noise)
4. **Time-lock Commitments** (commit now, reveal later)

---

### 6. 🟢 Interest Rate Manipulation (LOW PRIORITY)

**Threat**: Large borrower manipulates pool utilization to affect rates for others.

**Current State**:
- Two-slope interest rate model ✅
- Dynamic rates based on utilization ✅
- But single large borrow could spike rates ❌

**Attack Scenario**:
```
1. Attacker borrows 90% of pool liquidity
2. Utilization spikes → interest rate jumps to 100%
3. All existing borrowers pay massive interest
4. Attacker repays immediately (minimal interest)
5. Profit from others' forced deleveraging
```

**Risk**: 🟢 **LOW**
- Attack is expensive (need capital to borrow 90%)
- Only affects interest rates, not liquidations
- Self-limiting (attacker also pays high rates)

**Proposed Mitigations**:
1. **Borrow Caps** (max % of pool per user)
2. **Interest Rate Smoothing** (TWAP of utilization)
3. **Minimum Borrow Duration** (can't flash-borrow)

---

### 7. 🟢 Validator Censorship (LOW-MEDIUM PRIORITY)

**Threat**: Malicious block proposer censors specific transactions.

**Current State**:
- Leader rotation (fair leader selection ✅)
- But current leader fully controls block contents ❌
- No censorship resistance guarantees ❌

**Attack Scenario**:
```
1. Validator becomes block proposer
2. User submits liquidation of validator's own position
3. Validator censors (excludes) this transaction
4. Validator's position doesn't get liquidated
5. Validator rotates out, keeps unsafe position
```

**Risk**: 🟢 **LOW-MEDIUM**
- Only works for 1 block (2 seconds)
- Next validator would include the transaction
- But could delay critical liquidations

**Proposed Mitigations**:
1. **Censorship Resistance Protocol** (validators must justify exclusions)
2. **Transaction Inclusion Proofs** (proof tx was broadcast)
3. **Mempool Monitoring** (detect if validators dropping txs)
4. **Forced Inclusion** (if tx not included after N blocks, slash proposer)

---

## Priority Matrix

| Attack Vector | Risk Level | Impact | Likelihood | Priority |
|--------------|------------|--------|------------|----------|
| **Liquidation Timing** | 🔴 High-Medium | High | Medium | **1** |
| **Position Size Concentration** | 🟡 Medium | Critical | Low | **2** |
| **Oracle Manipulation** | 🟡 Medium | Critical | Very Low | **3** |
| **DoS via Spam** | 🟡 Medium | High | Medium | **4** |
| **Privacy via Timing** | 🟢 Low-Medium | Medium | Low | **5** |
| **Interest Rate Manipulation** | 🟢 Low | Low | Low | **6** |
| **Validator Censorship** | 🟢 Low-Medium | Medium | Low | **7** |

---

## Recommended Implementation Order

### Phase 1: Critical Hardening (Week 1)
**Must-have before testnet launch**

1. **Liquidation Timing Attack Prevention** 🔴
   - Implement liquidation delay (1-2 blocks)
   - Add penalty decay (5% → 1% over time)
   - Estimated: 200 lines, 1 day

2. **Position Size Limits** 🟡
   - Max position size (10% of pool TVL)
   - Max notional exposure per user
   - Leverage tiers
   - Estimated: 150 lines, 1 day

3. **Circuit Breakers** 🟡
   - Pause trading if liquidations > 20% of positions
   - Emergency shutdown mechanism
   - Estimated: 100 lines, 0.5 day

### Phase 2: Important Hardening (Week 2)
**Should-have before mainnet**

4. **DoS Prevention** 🟡
   - Transaction fees (simple fee model)
   - Rate limiting (max 10 orders/address/block)
   - Estimated: 100 lines, 0.5 day

5. **Oracle Hardening** 🟡
   - Tighter spread limits (2% max, not 5%)
   - TWAP implementation (5-block average)
   - Estimated: 150 lines, 1 day

6. **Validator Slashing** 🟡
   - Detect oracle manipulation
   - Slash validator stake
   - Estimated: 200 lines, 1 day

### Phase 3: Nice-to-Have (Week 3+)
**Can wait until after initial mainnet**

7. **Privacy Enhancements** 🟢
   - Transaction batching
   - Random delays
   - Estimated: 100 lines, 0.5 day

8. **Interest Rate Protections** 🟢
   - Borrow caps
   - Rate smoothing
   - Estimated: 80 lines, 0.5 day

9. **Censorship Resistance** 🟢
   - Inclusion proofs
   - Mempool monitoring
   - Estimated: 150 lines, 1 day

---

## Estimated Timeline

**Phase 1 (Critical)**: 2.5 days of implementation
**Phase 2 (Important)**: 3 days of implementation
**Phase 3 (Nice-to-have)**: 2 days of implementation

**Total**: ~1.5 weeks for all hardening

**Recommendation**: Implement Phase 1 immediately, Phase 2 before mainnet, Phase 3 can wait.

---

## Risk Assessment After Full Hardening

| Category | Current | After Phase 1 | After Phase 2 | After Phase 3 |
|----------|---------|---------------|---------------|---------------|
| **MEV Attacks** | 🟢 Low | 🟢 Low | 🟢 Low | 🟢 Low |
| **Liquidation Attacks** | 🔴 Medium-High | 🟢 Low | 🟢 Low | 🟢 Low |
| **Systemic Risk** | 🟡 Medium | 🟢 Low | 🟢 Low | 🟢 Low |
| **Oracle Manipulation** | 🟡 Medium | 🟡 Medium | 🟢 Low | 🟢 Low |
| **DoS Attacks** | 🟡 Medium | 🟡 Medium | 🟢 Low | 🟢 Low |
| **Privacy Leakage** | 🟢 Low-Medium | 🟢 Low-Medium | 🟢 Low-Medium | 🟢 Low |
| **Overall Security** | 🟡 **Medium** | 🟢 **Good** | 🟢 **Strong** | 🟢 **Excellent** |

---

## Next Steps

1. **Review this analysis** - Confirm priority order
2. **Start Phase 1 implementation** - Liquidation timing + position limits
3. **Create test suite** - Attack simulation scenarios
4. **Deploy to testnet** - Real-world testing
5. **Bug bounty** - Incentivize white-hat researchers

---

**Analysis Date**: 2025-11-12
**Status**: 🟡 **NEEDS HARDENING**
**Recommendation**: Implement Phase 1 (Critical) before testnet launch
