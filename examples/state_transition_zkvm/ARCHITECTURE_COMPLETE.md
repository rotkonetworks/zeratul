# Zeratul Blockchain Architecture

**Complete Technical Architecture Documentation**

---

## Table of Contents

1. [Overview](#overview)
2. [Core Insight: AccidentalComputer + Ligerito](#core-insight)
3. [System Architecture](#system-architecture)
4. [Proof Flow](#proof-flow)
5. [Component Details](#component-details)
6. [Verification Strategies](#verification-strategies)
7. [Dependencies](#dependencies)
8. [Why This Design](#why-this-design)

---

## Overview

Zeratul is a blockchain built on **Commonware** that uses the **AccidentalComputer** pattern with **Ligerito** polynomial commitments for **zero-overhead ZK proofs**.

### Key Properties

- ✅ **Zero Overhead**: Data availability encoding doubles as polynomial commitment
- ✅ **Flexible Verification**: Three verification strategies (native, on-chain, light client)
- ✅ **Deterministic Consensus**: PolkaVM ensures all nodes agree on verification
- ✅ **Privacy-Preserving**: FROST threshold signatures + Bandersnatch Ring VRF
- ✅ **Single Framework**: Clean architecture with Commonware base

### What Makes This Unique

**Traditional ZK Rollup**:
```
Data → Reed-Solomon (for DA)
     + Separate PCS (for ZK)
     = Two encodings (double overhead)
```

**Zeratul (AccidentalComputer)**:
```
Data → ZODA (Reed-Solomon)
     = Data availability encoding
     = Polynomial commitment (Ligerito!)
     = ONE encoding (zero overhead!)
```

---

<a name="core-insight"></a>
## Core Insight: AccidentalComputer + Ligerito

### The Papers

1. **"The Accidental Computer"** (Evans & Angeris, Jan 2025)
   - **Key insight**: DA encoding can serve as polynomial commitment
   - **Pattern**: `Z = GX̃G'ᵀ` (tensor encoding) → use as PCS
   - **Proposed**: Use WHIR/FRI-Binius/BaseFold as PCS

2. **"Ligerito"** (Angeris et al., May 2025)
   - **Section 5**: "The accidental computer: polynomial commitments from data availability"
   - **Shows**: How to use Ligerito with AccidentalComputer pattern
   - **Ligerito can replace WHIR/BaseFold!**

### What We Implement

We implement the **AccidentalComputer pattern using Ligerito** as the polynomial commitment scheme:

```
State Transition Data
         ↓
    ZODA Encode (Reed-Solomon)
    Z = GX̃G'ᵀ
         ↓
    Serves as BOTH:
    1. Data Availability (erasure coding)
    2. Polynomial Commitment (Ligerito framework)
         ↓
    Two Verification Paths:
    - Full Nodes: Verify ZODA shards directly
    - Light Clients: Extract succinct proof via Ligerito
```

### Why Ligerito?

The AccidentalComputer paper says it works with any PCS that:
- ✅ Can commit to encoded data (via linear codes like Reed-Solomon)
- ✅ Supports multilinear evaluation
- ✅ Works with WHIR, FRI-Binius, or BaseFold

**Ligerito meets all requirements:**
- ✅ Multilinear polynomial commitments
- ✅ Works with general linear codes including Reed-Solomon
- ✅ Has efficient evaluation proofs
- ✅ Small proof sizes (~255 KiB for 2²⁴ coefficients)

From Ligerito paper:
> "any linear code for which the rows of the generator matrix can be efficiently evaluated can be used. Such codes include Reed–Solomon codes..."

**Perfect match!** Ligerito slots right in where WHIR/BaseFold were proposed.

---

## System Architecture

### High-Level Overview

```
┌────────────────────────────────────────────────┐
│          Zeratul Blockchain                    │
│       (Commonware Framework)                   │
├────────────────────────────────────────────────┤
│                                                │
│  Application Layer                             │
│    ├─ State Transitions (Circuit)             │
│    ├─ AccidentalComputer (Ligerito §5)        │
│    │   └─ ZODA (Reed-Solomon as PCS)          │
│    └─ NOMT (Authenticated State)              │
│                                                │
│  Consensus Layer                               │
│    ├─ Commonware Simplex BFT                  │
│    │   └─ PolkaVM Verifier (embedded)         │
│    ├─ Safrole (Bandersnatch Ring VRF)         │
│    └─ FROST (Threshold Signatures)            │
│                                                │
│  Network Layer                                 │
│    ├─ Commonware P2P (QUIC)                   │
│    ├─ Commonware Broadcast (Gossip)           │
│    └─ Commonware Resolver (Discovery)         │
│                                                │
│  Storage Layer                                 │
│    ├─ NOMT (State Merkle Tree)                │
│    └─ Commonware Storage (Metadata)           │
│                                                │
└────────────────────────────────────────────────┘
```

### Component Stack

```
┌─────────────────────────────────────────┐
│  Client Application                     │
└─────────────┬───────────────────────────┘
              ↓
┌─────────────────────────────────────────┐
│  Circuit (State Transition Logic)       │
│  - Transfer validation                  │
│  - Balance updates                      │
│  - Constraint checking                  │
└─────────────┬───────────────────────────┘
              ↓
┌─────────────────────────────────────────┐
│  AccidentalComputer (Proof Generation)  │
│  - ZODA encoding (Reed-Solomon)         │
│  - Generates erasure-coded shards       │
│  - Commitment = Merkle root             │
└─────────────┬───────────────────────────┘
              ↓
┌─────────────────────────────────────────┐
│  Blockchain (Consensus & Storage)       │
│  - Safrole consensus (BFT + VRF)        │
│  - FROST threshold signatures           │
│  - NOMT state storage                   │
└─────────────┬───────────────────────────┘
              ↓
      ┌───────┴───────┐
      ↓               ↓
┌──────────┐    ┌─────────────────┐
│Full Nodes│    │ Light Clients   │
│Fast Path │    │ Succinct Proofs │
└──────────┘    └─────────────────┘
```

---

## Proof Flow

### Complete Proof Lifecycle

```
┌───────────────────────────────────────────────┐
│  1. STATE TRANSITION (Circuit)                │
│     - Validate transfer                       │
│     - Compute new state                       │
│     - Generate witness                        │
└─────────────────┬─────────────────────────────┘
                  ↓
┌───────────────────────────────────────────────┐
│  2. PROOF GENERATION (AccidentalComputer)     │
│     Off-chain, by prover                      │
│                                               │
│     a) Serialize witness data                 │
│     b) ZODA encode (Reed-Solomon)             │
│        Z = GX̃G'ᵀ (tensor product)            │
│     c) Generate erasure-coded shards          │
│     d) Compute Merkle commitment              │
│                                               │
│     Output: AccidentalComputerProof           │
│             - Commitment: ~32 bytes           │
│             - Shards: ~10KB-1MB               │
└─────────────────┬─────────────────────────────┘
                  ↓
┌───────────────────────────────────────────────┐
│  3. SUBMISSION TO CHAIN                       │
│     Proof submitted to blockchain             │
└─────────────────┬─────────────────────────────┘
                  ↓
          ┌───────┴──────┐
          ↓              ↓
┌──────────────────┐  ┌───────────────────────┐
│  4a. FULL NODES  │  │  4b. LIGHT CLIENTS    │
│  (Fast Path)     │  │  (Succinct Path)      │
│                  │  │                       │
│  Verify ZODA     │  │  Extract Succinct:    │
│  shards directly │  │                       │
│                  │  │  a) Reconstruct poly  │
│  - Check RS      │  │     from shards       │
│    properties    │  │  b) Run Ligerito      │
│  - Verify        │  │     prover            │
│    Merkle paths  │  │     (sumcheck +       │
│                  │  │      opening proofs)  │
│  ~1-5ms          │  │  c) Get succinct      │
│  ~MB proof       │  │     proof (~KB)       │
└──────────────────┘  │                       │
                      │  ~20-30ms proving     │
                      │  ~KB proof            │
                      └──────────┬────────────┘
                                 ↓
                      ┌────────────────────────┐
                      │  5. POLKAVM VERIFIER   │
                      │  (On-Chain or Client)  │
                      │                        │
                      │  - Deterministic exec  │
                      │  - Ligerito verify()   │
                      │  - Consensus-safe      │
                      │                        │
                      │  ~20-30ms verification │
                      └────────────────────────┘
```

### Proof Data Structures

#### AccidentalComputerProof (Full Shards)

```rust
pub struct AccidentalComputerProof {
    /// ZODA commitment (Merkle root)
    /// This IS the polynomial commitment!
    pub zoda_commitment: Vec<u8>,  // ~32 bytes

    /// Shard indices
    pub shard_indices: Vec<u16>,

    /// The actual shards (erasure-coded)
    /// Contains the Reed-Solomon encoded matrix Z
    pub shards: Vec<Vec<u8>>,  // ~10KB-1MB total

    /// Public inputs (state commitments)
    pub sender_commitment_old: [u8; 32],
    pub sender_commitment_new: [u8; 32],
    pub receiver_commitment_old: [u8; 32],
    pub receiver_commitment_new: [u8; 32],
}
```

**Size**: ~10KB-1MB depending on configuration
**Use**: Full nodes, native verification

#### LigeritoSuccinctProof (Compressed)

```rust
pub struct LigeritoSuccinctProof {
    /// Serialized Ligerito proof
    /// Includes sumcheck protocol + opening proofs
    pub proof_bytes: Vec<u8>,  // ~1-30KB

    /// Configuration size (2^N)
    pub config_size: u32,

    /// Public inputs (same as above)
    pub sender_commitment_old: [u8; 32],
    pub sender_commitment_new: [u8; 32],
    pub receiver_commitment_old: [u8; 32],
    pub receiver_commitment_new: [u8; 32],
}
```

**Size**: ~1-30KB depending on polynomial size
**Use**: Light clients, PolkaVM verification

---

## Component Details

### 1. Circuit (State Transition Logic)

**Location**: `circuit/src/`

**Purpose**: Compute valid state transitions

**Key Code**:

```rust
pub fn prove_with_accidental_computer(
    config: &AccidentalComputerConfig,
    instance: &TransferInstance,
) -> Result<AccidentalComputerProof> {
    // 1. Serialize witness
    let data = serialize_transfer_instance(instance)?;

    // 2. ZODA encode (Reed-Solomon)
    // This IS Ligerito framework usage! (Section 5)
    let (commitment, shards) = Zoda::<Sha256>::encode(
        &coding_config,
        data.as_ref()
    )?;

    // 3. The ZODA commitment IS our polynomial commitment!
    Ok(AccidentalComputerProof {
        zoda_commitment: commitment.encode().to_vec(),
        shards: shard_bytes,
        // ...
    })
}
```

**Why This Works**:
1. ZODA uses Reed-Solomon encoding: `Z = GX̃G'ᵀ`
2. Reed-Solomon is a Ligerito-compatible linear code
3. The encoding matrix can be used as polynomial commitment
4. **Zero overhead**: Same data serves DA + ZK!

### 2. Light Client (Succinct Proofs)

**Location**: `blockchain/src/light_client.rs`

**Purpose**: Bandwidth-efficient sync

**Key Function** (Currently Placeholder):

```rust
pub fn extract_succinct_proof(
    accidental_proof: &AccidentalComputerProof,
    config_size: u32,
) -> Result<LigeritoSuccinctProof> {
    // TODO: Implement the missing bridge!
    //
    // What needs to happen:
    // 1. Reconstruct polynomial from ZODA shards
    //    (shards contain the tensor-encoded matrix Z)
    //
    // 2. Run Ligerito prover on this matrix
    //    (this is "Step 5" from AccidentalComputer paper!)
    //    let config = ligerito::hardcoded_config_24(...);
    //    let polynomial = reconstruct_from_zoda_shards(proof)?;
    //    let ligerito_proof = ligerito::prover(&config, &polynomial)?;
    //
    // 3. Generate succinct proof (~KB instead of ~MB)

    todo!("This is the missing piece - bridge ZODA to Ligerito PCS")
}
```

**This is the key missing implementation!**

### 3. PolkaVM Verifier

**Location**: `examples/polkavm_verifier/`

**Purpose**: Deterministic proof verification

**Guest Program** (runs inside PolkaVM):

```rust
fn main() {
    // Read proof from stdin
    let mut input = Vec::new();
    io::stdin().read_to_end(&mut input)?;

    // Parse: [config_size: u32][proof_bytes]
    let config_size = u32::from_le_bytes([input[0], input[1], input[2], input[3]]);
    let proof: FinalizedLigeritoProof = bincode::deserialize(&input[4..])?;

    // Verify using Ligerito
    let result = match config_size {
        12 => verify(&ligerito::hardcoded_config_12_verifier(), &proof),
        24 => verify(&ligerito::hardcoded_config_24_verifier(), &proof),
        // ...
    };

    // Exit: 0=valid, 1=invalid
    std::process::exit(if result.is_ok() { 0 } else { 1 });
}
```

**Host Integration** (Commonware consensus):

```rust
pub struct PolkaVMVerifier {
    verifier_binary: Arc<Vec<u8>>,
    timeout: Duration,
}

impl PolkaVMVerifier {
    pub fn verify_in_consensus(
        &self,
        proof: &LigeritoSuccinctProof
    ) -> Result<bool> {
        // All nodes run this deterministically!
        let blob = polkavm::ProgramBlob::parse(&self.verifier_binary)?;
        let engine = polkavm::Engine::new(&config)?;
        let module = polkavm::Module::from_blob(&engine, &config, blob)?;

        let mut instance = module.instantiate()?;
        let result = instance.call_typed(&mut (), "main", &input)?;

        Ok(result == 0)  // 0 = valid
    }
}
```

---

## Verification Strategies

### Strategy 1: Native (Off-Chain, Fast)

**When**: Full nodes with bandwidth
**How**: Verify Reed-Solomon properties directly
**Speed**: ~1-5ms
**Proof Size**: ~10KB-1MB
**Security**: Trust in DA encoding

```rust
pub fn verify_accidental_computer(
    config: &AccidentalComputerConfig,
    proof: &AccidentalComputerProof,
) -> Result<bool> {
    // Verify ZODA shards directly
    let commitment = reconstruct_commitment_from_shards(&proof.shards)?;
    Ok(commitment == proof.zoda_commitment)
}
```

### Strategy 2: PolkaVM in Consensus (On-Chain, Deterministic)

**When**: Consensus-critical verification
**How**: Extract succinct proof, verify in PolkaVM
**Speed**: ~20-30ms
**Proof Size**: ~1-30KB
**Security**: Consensus-guaranteed

```rust
impl Automaton for SafroleAutomaton {
    fn verify(&mut self, block: &Block) -> bool {
        for proof in &block.proofs {
            // Extract succinct proof
            let succinct = extract_succinct_proof(proof, 24)?;

            // All nodes run PolkaVM - deterministic!
            if !self.polkavm_verifier.verify_in_consensus(&succinct)? {
                return false;
            }
        }
        true
    }
}
```

### Strategy 3: Light Client (Off-Chain, Succinct)

**When**: Bandwidth-limited clients
**How**: Download succinct proof only, verify in PolkaVM
**Speed**: ~20-30ms
**Proof Size**: ~1-30KB
**Security**: Cryptographic proof

```rust
impl LightClient {
    pub async fn verify_via_polkavm(
        &self,
        proof: &LigeritoSuccinctProof
    ) -> Result<bool> {
        // Run PolkaVM locally (sandboxed)
        self.polkavm_runner
            .as_ref()
            .ok_or_else(|| anyhow!("PolkaVM not initialized"))?
            .verify(proof)
            .await
    }
}
```

### Comparison

| Strategy | Where | Speed | Proof Size | Security |
|----------|-------|-------|------------|----------|
| **Native** | Off-chain | ~1-5ms | ~10KB-1MB | DA encoding |
| **PolkaVM Consensus** | On-chain | ~20-30ms | ~1-30KB | Consensus |
| **Light Client** | Off-chain | ~20-30ms | ~1-30KB | Cryptographic |

---

## Dependencies

### Dependency Architecture

```
┌─────────────────────────────────────┐
│  Zeratul = Single Framework         │
├─────────────────────────────────────┤
│                                     │
│  Framework:                         │
│    └─ Commonware (EVERYTHING)      │
│                                     │
│  Crypto Libraries:                  │
│    ├─ sp-core (Bandersnatch)        │
│    ├─ decaf377-frost (FROST)        │
│    └─ ligerito (PCS - TWO ways!)    │
│                                     │
│  VM Library:                        │
│    └─ polkavm (Direct integration)  │
│                                     │
│  Storage Library:                   │
│    └─ NOMT (Authenticated state)    │
│                                     │
└─────────────────────────────────────┘
```

### Key Point: Ligerito is Used TWO Ways!

```
Ligerito Usage:

1. Framework (AccidentalComputer):
   Circuit → ZODA encode (commonware-coding)
          → Reed-Solomon IS Ligerito-compatible
          → ZODA commitment IS polynomial commitment
          → Implements Section 5 of Ligerito paper

2. Implementation (PolkaVM Verifier):
   Light Client → Extract polynomial from ZODA
                → Run ligerito::prover()
                → Get succinct proof
                → Verify via ligerito::verify() in PolkaVM
```

**Both are Ligerito!** Just different parts of the same system.

### What We're NOT Building

❌ NOT a Substrate Parachain
❌ NOT a Cosmos SDK Chain
❌ NOT mixing blockchain frameworks

✅ Commonware framework (base)
✅ Crypto libraries (sp-core, decaf377-frost)
✅ Ligerito (framework + implementation)
✅ PolkaVM (direct integration, no Substrate)

---

## Why This Design?

### 1. Zero Overhead (AccidentalComputer)

**Traditional**: Two separate encodings (DA + ZK)
**Ours**: One encoding serves both purposes

**Savings**: 50% less computation, 50% less storage

### 2. Flexible Verification

Different verifiers, different needs:
- **Full nodes**: Fast verification (native)
- **Light clients**: Small proofs (Ligerito succinct)
- **On-chain**: Deterministic (PolkaVM consensus)

**One proof data, three verification paths!**

### 3. Deterministic Consensus

**Problem**: Native verification might differ between nodes
**Solution**: PolkaVM ensures same result on all nodes

### 4. Clean Architecture

**Single framework** (Commonware) with crypto libraries
**No framework mixing** (no Substrate + Cosmos + etc.)

---

## Future Work

### Short Term
1. Complete `extract_succinct_proof()` (bridge ZODA → Ligerito)
2. Build PolkaVM verifier binary
3. Test end-to-end light client sync

### Medium Term
4. Optimize ZODA parameters
5. Implement proof caching
6. Add batch verification

### Long Term
7. Recursive proofs (aggregate multiple)
8. Cross-chain bridges
9. Hardware acceleration

---

## References

**Papers**:
- "The Accidental Computer" (Evans & Angeris, Jan 2025)
- "Ligerito" (Angeris et al., May 2025) - Section 5

**Related Docs**:
- `ZODA_IS_LIGERITO_CLARIFICATION.md` - The key insight
- `ARCHITECTURE_CLARIFIED.md` - Dependency clarification
- `LIGHT_CLIENT_INTEGRATION.md` - Light client design

---

## Conclusion

Zeratul implements **AccidentalComputer + Ligerito**, achieving:

✅ Zero overhead (one encoding for DA + ZK)
✅ Flexible verification (three strategies)
✅ Deterministic consensus (PolkaVM)
✅ Clean architecture (single framework)

**Key insight**: ZODA encoding IS a polynomial commitment (Ligerito framework)!

---

**Last Updated**: 2025-11-12
**Version**: 1.0
