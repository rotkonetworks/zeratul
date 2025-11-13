# Zeratul Architecture - Quick Reference

**TL;DR**: Blockchain using AccidentalComputer pattern with Ligerito for zero-overhead ZK proofs.

---

## The Core Idea (5 Second Version)

```
Traditional: Data → DA encode + ZK encode = 2x cost
Zeratul:     Data → ZODA encode = DA + ZK = 1x cost ✅
```

**ZODA encoding IS the polynomial commitment!**

---

## The Core Idea (30 Second Version)

1. **Problem**: ZK rollups encode data twice (DA + ZK proofs)
2. **Insight**: Reed-Solomon encoding can serve as polynomial commitment
3. **Solution**: Use ZODA (Reed-Solomon) for both DA and ZK
4. **Result**: Zero overhead - one encoding serves two purposes!

**Papers**:
- AccidentalComputer (Jan 2025) - The pattern
- Ligerito Section 5 (May 2025) - How to use Ligerito with it

---

## How It Works (2 Minute Version)

### Proof Generation (Off-Chain)

```
State Transition
      ↓
Serialize witness data
      ↓
ZODA encode (Reed-Solomon)
  Z = GX̃G'ᵀ
      ↓
AccidentalComputerProof
  - Commitment (~32 bytes)
  - Shards (~10KB-1MB)
```

### Proof Verification (Three Ways)

```
┌─────────────────┐
│ Proof Data      │
└────────┬────────┘
         │
    ┌────┴────┐
    ↓         ↓
Full Nodes    Light Clients
Verify        Extract succinct proof
shards        via Ligerito prover
directly           ↓
(~1-5ms)      Verify in PolkaVM
(~MB)         (~20-30ms)
              (~KB)
```

---

## Architecture Stack (Visual)

```
Client Application
        ↓
Circuit (State Transitions)
        ↓
AccidentalComputer (ZODA encode)
        ↓
Blockchain (Commonware)
  ├─ Consensus (Safrole + FROST)
  ├─ Network (P2P + Gossip)
  └─ Storage (NOMT)
        ↓
   ┌────┴────┐
   ↓         ↓
Full Nodes  Light Clients
(Fast)      (Succinct)
```

---

## Key Components

### 1. Circuit
**What**: Computes valid state transitions
**Where**: `circuit/src/`
**Output**: Witness data

### 2. AccidentalComputer
**What**: Generates ZODA proofs
**Where**: `circuit/src/accidental_computer.rs`
**Output**: AccidentalComputerProof (with shards)

### 3. Light Client
**What**: Extracts succinct proofs
**Where**: `blockchain/src/light_client.rs`
**Output**: LigeritoSuccinctProof (compressed)

### 4. PolkaVM Verifier
**What**: Verifies succinct proofs deterministically
**Where**: `examples/polkavm_verifier/`
**Output**: Valid/Invalid

---

## Three Verification Strategies

### Strategy 1: Native (Full Nodes)
- **Speed**: ~1-5ms
- **Size**: ~MB
- **Use**: Full nodes with bandwidth

### Strategy 2: PolkaVM Consensus
- **Speed**: ~20-30ms
- **Size**: ~KB
- **Use**: On-chain verification (deterministic)

### Strategy 3: Light Client
- **Speed**: ~20-30ms
- **Size**: ~KB
- **Use**: Bandwidth-limited clients

---

## Dependencies (What We Use)

### Framework
- **Commonware**: Everything (consensus, network, storage)

### Crypto Libraries
- **sp-core**: Bandersnatch Ring VRF only
- **decaf377-frost**: FROST threshold sigs only
- **ligerito**: Polynomial commitments (TWO ways!)

### VM Library
- **polkavm**: Direct integration (no Substrate)

### Storage
- **NOMT**: Authenticated state tree

---

## The Ligerito Magic (Most Important!)

### Ligerito is Used TWO Ways:

**Way 1: Framework (AccidentalComputer)**
```rust
// Circuit generates ZODA proof
let (commitment, shards) = Zoda::<Sha256>::encode(&config, data)?;
// This IS Ligerito! Reed-Solomon = polynomial commitment
```

**Way 2: Implementation (PolkaVM Verifier)**
```rust
// Light client extracts succinct proof
let polynomial = reconstruct_from_zoda_shards(proof)?;
let succinct = ligerito::prover(&config, &polynomial)?;

// Verifier checks it
let valid = ligerito::verify(&config, &succinct)?;
```

**Both are Ligerito!** Just different applications.

---

## What Makes This Work

### The Math
```
ZODA encoding uses Reed-Solomon: Z = GX̃G'ᵀ
Reed-Solomon is a linear code
Linear codes can be polynomial commitments (Ligerito framework)
→ ZODA commitment IS polynomial commitment! ✅
```

### The Papers
```
AccidentalComputer paper (Jan 2025):
  "Use DA encoding as polynomial commitment"
  "Works with WHIR/FRI-Binius/BaseFold"

Ligerito paper Section 5 (May 2025):
  "The accidental computer pattern"
  "Works with any linear code (Reed-Solomon, etc.)"

→ Ligerito can replace WHIR/BaseFold! ✅
```

---

## Code Flow Example

### Full Node (Fast Path)

```rust
// 1. Receive proof
let proof: AccidentalComputerProof = receive_from_network();

// 2. Verify ZODA shards directly
let valid = verify_accidental_computer(&config, &proof)?;

// Done! (~1-5ms)
```

### Light Client (Succinct Path)

```rust
// 1. Receive proof
let proof: AccidentalComputerProof = receive_from_network();

// 2. Extract succinct proof (TODO: implement this!)
let succinct = extract_succinct_proof(&proof, 24)?;

// 3. Verify via PolkaVM
let valid = verify_via_polkavm(&succinct).await?;

// Done! (~20-30ms, but only ~KB downloaded)
```

---

## Missing Piece (TODO)

The bridge between ZODA and Ligerito PCS:

```rust
pub fn extract_succinct_proof(
    accidental_proof: &AccidentalComputerProof,
    config_size: u32,
) -> Result<LigeritoSuccinctProof> {
    // TODO: Implement this!
    // 1. Reconstruct polynomial from ZODA shards
    // 2. Run ligerito::prover() on it
    // 3. Return succinct proof

    todo!("This is the key missing implementation")
}
```

**This connects everything together!**

---

## Why This Design?

### Zero Overhead
```
One encoding serves both DA and ZK
= 50% less computation
= 50% less storage
```

### Flexible Verification
```
Same proof data
= Three verification strategies
= Choose speed vs size vs determinism
```

### Clean Architecture
```
Single framework (Commonware)
+ Crypto libraries (not frameworks)
= Maintainable, modular
```

---

## Quick Reference Card

| What | Where | Purpose |
|------|-------|---------|
| **Circuit** | `circuit/src/` | State transitions |
| **AccidentalComputer** | `circuit/src/accidental_computer.rs` | ZODA proof generation |
| **Light Client** | `blockchain/src/light_client.rs` | Succinct proof extraction |
| **PolkaVM Verifier** | `examples/polkavm_verifier/` | Deterministic verification |
| **Blockchain** | `blockchain/src/` | Consensus + storage |

| Proof Type | Size | Speed | Use Case |
|------------|------|-------|----------|
| **AccidentalComputerProof** | ~MB | ~1-5ms | Full nodes |
| **LigeritoSuccinctProof** | ~KB | ~20-30ms | Light clients |

| Dependency | Role | What We Use |
|------------|------|-------------|
| **Commonware** | Framework | Everything |
| **sp-core** | Crypto lib | Bandersnatch only |
| **decaf377-frost** | Crypto lib | FROST only |
| **ligerito** | Proof system | Framework + Implementation |
| **polkavm** | VM lib | Direct (no Substrate) |
| **NOMT** | Storage | State tree |

---

## Next Steps

1. Read `ARCHITECTURE_COMPLETE.md` for full details
2. Read `ZODA_IS_LIGERITO_CLARIFICATION.md` for the key insight
3. Implement `extract_succinct_proof()` bridge
4. Test end-to-end light client sync

---

## Key Takeaway

```
ZODA encoding (Reed-Solomon) IS a polynomial commitment!

This is Ligerito Section 5 (AccidentalComputer pattern)

One encoding = Data availability + Zero-knowledge proofs

Zero overhead! 🎉
```

---

**Last Updated**: 2025-11-12
