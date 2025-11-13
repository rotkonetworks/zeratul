# FROST Threshold Signature Integration

## Overview

This document describes the integration of Penumbra's `decaf377-frost` implementation for multi-threshold signature verification in the Zeratul blockchain.

**Date**: 2025-11-12
**Status**: Foundation complete, integration in progress

---

## Why FROST?

### Benefits
- ✅ **Byzantine Fault Tolerance**: Threshold t/n signatures provide BFT guarantees
- ✅ **Single Signature Output**: Aggregated signature is indistinguishable from single signature
- ✅ **Penumbra Compatible**: Uses decaf377 curve (compatible with Penumbra ecosystem)
- ✅ **Flexible Thresholds**: Different security levels for different operations
- ✅ **Compact**: 64-byte signature regardless of threshold
- ✅ **Fast Verification**: Single signature verification (15x faster than 15 individual sigs)

### Performance
- **Latency**: ~100-200ms (2 network round trips)
- **Overhead**: <5% of 2-second block time
- **Bandwidth**: Saves ~896 bytes per operation (960 bytes → 64 bytes)
- **Storage**: 15x less signature data in blocks

---

## Multi-Threshold Architecture

### Tier 1: Any Validator (1/15)
**Threshold**: 1 out of 15 validators
**Required Signatures**: 1

**Use Cases**:
- Individual oracle price proposals
- Transaction inclusion in mempool
- Liquidation discovery (flagging positions)

**Rationale**: Maximum speed for operations where any honest validator is sufficient. Other validators verify independently.

**Performance**: <1ms (no coordination needed)

---

### Tier 2: Simple Majority (8/15)
**Threshold**: 8 out of 15 validators
**Required Signatures**: 8 (>50%)

**Use Cases**:
- Block proposals (standard consensus)
- Oracle consensus price (median of 8+ prices)
- Mempool ordering

**Rationale**: Standard simple majority for normal consensus operations. Balances security and speed.

**Performance**: ~100-200ms (FROST coordination)

---

### Tier 3: Byzantine Threshold (11/15 = 2/3+1)
**Threshold**: 11 out of 15 validators
**Required Signatures**: 11 (73.3%)

**Use Cases**:
- **Batch liquidation execution** (10/15 must approve)
- **Large fund movements** (>$100k equivalent)
- **Validator slashing decisions** (punishing malicious validators)
- **Emergency pause mechanisms**

**Rationale**: Byzantine fault tolerance - can tolerate up to 4 malicious/offline validators. Critical for security-sensitive operations.

**Performance**: ~100-200ms (acceptable for critical operations)

**Security**: Tolerates f=4 Byzantine validators (n=15, t=11, f=(n-t)/2=4)

---

### Tier 4: Supermajority (13/15)
**Threshold**: 13 out of 15 validators
**Required Signatures**: 13 (~87%)

**Use Cases**:
- **Protocol parameter changes** (interest rates, liquidation thresholds)
- **Smart contract upgrades**
- **Validator set changes** (adding/removing validators)
- **Emergency shutdown**
- **Major protocol upgrades**

**Rationale**: High consensus bar for irreversible governance changes. Prevents small coalition takeover.

**Performance**: ~100-200ms (but governance decisions take hours/days anyway)

---

## Implementation Details

### Module Structure

```
blockchain/src/frost.rs
├─ ThresholdRequirement (enum)
│  ├─ AnyValidator (1/15)
│  ├─ SimpleMajority (8/15)
│  ├─ ByzantineThreshold (11/15)
│  └─ Supermajority (13/15)
├─ FrostSignature (aggregated signature + metadata)
├─ FrostCoordinator (aggregation logic)
├─ ValidatorFrostKeys (per-validator key material)
└─ Signing protocol (Round 1 & Round 2)
```

### Key Types

```rust
pub struct FrostSignature {
    signature: [u8; 64],        // Aggregated decaf377-rdsa signature
    signers: Vec<ValidatorId>,  // Which validators participated
    threshold: ThresholdRequirement,
}

pub enum ThresholdRequirement {
    AnyValidator,         // 1/15
    SimpleMajority,       // 8/15
    ByzantineThreshold,   // 11/15
    Supermajority,        // 13/15
}
```

### Signing Protocol

**Round 1: Commitment**
```rust
// Each validator generates and broadcasts commitment
let (nonces, commitments) = validator_keys.round1_commit();
// Broadcast commitments to all other validators
```

**Round 2: Signature Share**
```rust
// After receiving all commitments, generate signature share
let signing_package = SigningPackage::new(commitments, message);
let share = validator_keys.round2_sign(&signing_package, &nonces)?;
// Broadcast share to coordinator
```

**Aggregation: Coordinator**
```rust
// Coordinator collects threshold shares and aggregates
let frost_sig = coordinator.aggregate(
    &signing_package,
    &signature_shares,
    ThresholdRequirement::ByzantineThreshold,
)?;
```

---

## Integration Roadmap

### Phase 1: Foundation ✅ COMPLETE
- [x] FROST module with multi-threshold support
- [x] ThresholdRequirement enum (1/15, 8/15, 11/15, 13/15)
- [x] FrostSignature type with validation
- [x] Integration with decaf377-frost (Penumbra)
- [x] Tests for threshold validation

**Files Created**:
- `blockchain/src/frost.rs` (~400 lines)
- Updated `Cargo.toml` with decaf377 dependencies

---

### Phase 2: Oracle Integration (Next)
**Goal**: Replace individual Ed25519 signatures with FROST in oracle consensus

**Tasks**:
1. Update `OracleProposal` to use `FrostSignature`
2. Implement Round 1/2 protocol for oracle price proposals
3. Coordinator aggregates into consensus price
4. Use **SimpleMajority (8/15)** threshold for oracle consensus

**Files to Modify**:
- `blockchain/src/penumbra/oracle.rs`
- Oracle consensus logic

**Expected Timeline**: 2-3 days

---

### Phase 3: Liquidation Integration (Next)
**Goal**: Use FROST Byzantine threshold for liquidation approvals

**Tasks**:
1. Update `LiquidationProposal` to use `FrostSignature`
2. Validators coordinate on batch liquidations
3. Use **ByzantineThreshold (11/15)** for execution
4. Single aggregated signature per batch

**Files to Modify**:
- `blockchain/src/lending/liquidation.rs`
- Liquidation engine

**Expected Timeline**: 2-3 days

---

### Phase 4: Governance System (Later)
**Goal**: Add governance with supermajority threshold

**Tasks**:
1. Create `governance.rs` module
2. Define `ProposalType` (parameter changes, upgrades, etc.)
3. Implement voting with FROST signatures
4. Use **Supermajority (13/15)** for execution
5. Time-locked execution (24-48 hour delay)

**New Files**:
- `blockchain/src/governance.rs`

**Expected Timeline**: 1-2 weeks

---

## Security Considerations

### Threshold Selection Rationale

**1/15 (Any Validator)**:
- Acceptable when other validators verify independently
- No coordination overhead
- Example: Oracle proposals (consensus uses median anyway)

**8/15 (Simple Majority)**:
- Standard BFT majority (>50%)
- Can tolerate 7 offline/malicious validators
- Good for regular consensus operations

**11/15 (Byzantine Threshold)**:
- 2/3 + 1 for Byzantine fault tolerance
- Can tolerate 4 offline/malicious validators
- Required for security-critical operations
- Standard BFT threshold

**13/15 (Supermajority)**:
- ~87% consensus for governance
- Can tolerate only 2 dissenting validators
- High bar prevents hasty changes
- Appropriate for irreversible decisions

### Attack Resistance

**Sybil Attack**:
- ✅ Mitigated: Fixed validator set (15 pre-selected validators)
- Attacker needs to compromise multiple validators

**Collusion Attack**:
- ✅ Mitigated by thresholds:
  - Simple operations: Need 8/15 validators
  - Critical operations: Need 11/15 validators
  - Governance: Need 13/15 validators

**Denial of Service**:
- ✅ Mitigated: Lower thresholds allow progress with fewer validators
- Can continue operating with 11/15 validators online
- Governance requires more (13/15) - acceptable delay

**Key Compromise**:
- ⚠️ Risk: If attacker gets (t-1) private keys, system vulnerable
- Mitigation: Hardware security modules (HSMs) for validator keys
- Mitigation: Regular key rotation (future work)

---

## Performance Benchmarks

### Oracle Price Update (SimpleMajority 8/15)

**Individual Ed25519 (previous)**:
```
Sign: 8 × 50μs = 400μs
Broadcast: 8 × 64 bytes = 512 bytes
Network: 1 round trip = ~50-100ms
Verify: 8 × 50μs = 400μs
Total: ~50-100ms
```

**FROST (new)**:
```
Round 1: 8 validators broadcast commitments
  ├─ Compute: 8 × 100μs = 800μs
  ├─ Network: ~50-100ms
  └─ Bandwidth: 8 × 64 bytes = 512 bytes

Round 2: 8 validators broadcast shares
  ├─ Compute: 8 × 100μs = 800μs
  ├─ Network: ~50-100ms
  └─ Bandwidth: 8 × 32 bytes = 256 bytes

Aggregation: Coordinator combines
  ├─ Compute: ~500μs
  └─ Result: 64 bytes

Total: ~100-200ms, 832 bytes bandwidth
Verification: 50μs (single signature!)
```

**Verdict**: ~2x latency increase (50→100ms), but within acceptable range for 2-second blocks

---

### Liquidation Batch (ByzantineThreshold 11/15)

**Individual Signatures**:
- 11 signatures × 64 bytes = 704 bytes per batch
- Verification: 11 × 50μs = 550μs

**FROST**:
- 1 signature × 64 bytes = 64 bytes per batch
- Verification: 50μs (11x faster!)
- Historical storage: **640 bytes saved per liquidation batch**

**Over 1 million blocks**:
- Individual: 704 MB signature data
- FROST: 64 MB signature data
- **Savings: 640 MB (91% reduction!)**

---

## Testing Strategy

### Unit Tests ✅
- Threshold calculation (1/15, 8/15, 11/15, 13/15)
- Threshold validation (is_met checks)
- Signature validation (duplicate signers, out-of-range IDs)

### Integration Tests (TODO)
- Full Round 1 + Round 2 + Aggregation flow
- Multiple validators coordinating
- Threshold enforcement in production paths

### End-to-End Tests (TODO)
- 15-validator testnet with FROST
- Oracle consensus with 8/15 threshold
- Liquidation execution with 11/15 threshold
- Governance vote with 13/15 threshold

---

## Decaf377 Compatibility

### What is Decaf377?
- Prime-order elliptic curve group
- Built on top of Ristretto255
- Used throughout Penumbra ecosystem
- Efficient and cryptographically sound

### Why Decaf377?
- ✅ Native Penumbra compatibility
- ✅ Fast group operations
- ✅ 64-byte signatures (compact)
- ✅ Battle-tested in production (Penumbra mainnet)
- ✅ FROST implementation available (decaf377-frost)

### Dependencies Added
```toml
decaf377-frost = { path = "../../../penumbra/crates/crypto/decaf377-frost" }
decaf377-rdsa = { path = "../../../penumbra/crates/crypto/decaf377-rdsa" }
decaf377 = { path = "../../../penumbra/crates/crypto/decaf377" }
```

---

## Next Steps

### Immediate (This Week)
1. ✅ FROST module foundation complete
2. ⏳ Integrate with oracle consensus (SimpleMajority 8/15)
3. ⏳ Test oracle FROST coordination on local testnet

### Short-term (Next 2 Weeks)
1. Integrate with liquidation system (ByzantineThreshold 11/15)
2. Add validator key generation and distribution tooling
3. Test multi-validator FROST coordination

### Medium-term (Next Month)
1. Add governance system (Supermajority 13/15)
2. Implement key rotation mechanism
3. External security audit of FROST integration

### Long-term (Next Quarter)
1. Hardware security module (HSM) integration for validator keys
2. Trusted setup ceremony for initial key distribution
3. Production deployment on mainnet

---

## Resources

### Penumbra FROST Implementation
- Path: `/home/alice/rotko/penumbra/crates/crypto/decaf377-frost/`
- Documentation: https://protocol.penumbra.zone/main/crypto/decaf377.html
- Paper: "FROST: Flexible Round-Optimized Schnorr Threshold Signatures"

### References
- [FROST Paper](https://eprint.iacr.org/2020/852.pdf)
- [Penumbra Crypto](https://github.com/penumbra-zone/penumbra/tree/main/crates/crypto)
- [Decaf377 Spec](https://penumbra.zone/crypto/decaf377/)

---

**Status**: Foundation complete, ready for integration
**Next**: Oracle consensus integration with SimpleMajority threshold

