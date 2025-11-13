# Ligerito Usage Audit

**Date**: 2025-11-12
**Question**: Is Ligerito being used now?

## TL;DR Answer

**NO** - Ligerito is **NOT fully integrated yet**. Here's what we have:

```
✅ Ligerito verifier: WORKS (in PolkaVM guest)
⚠️ Ligerito prover: EXISTS but NOT WIRED UP
⚠️ Proof extraction: PLACEHOLDER only
❌ End-to-end flow: NOT COMPLETE
```

## Detailed Analysis

### 1. PolkaVM Verifier (✅ Working)

**File**: `examples/polkavm_verifier/main.rs`

**Status**: ✅ **FULLY FUNCTIONAL**

```rust
use ligerito::{verify, FinalizedLigeritoProof};
use binary_fields::{BinaryElem32, BinaryElem128};

fn main() {
    // Read proof from stdin
    let proof: FinalizedLigeritoProof<BinaryElem32, BinaryElem128> =
        bincode::deserialize(proof_bytes)?;

    // Verify using Ligerito (REAL)
    let result = match config_size {
        12 => verify(&ligerito::hardcoded_config_12_verifier(), &proof),
        16 => verify(&ligerito::hardcoded_config_16_verifier(), &proof),
        20 => verify(&ligerito::hardcoded_config_20_verifier(), &proof),
        24 => verify(&ligerito::hardcoded_config_24_verifier(), &proof),
        28 => verify(&ligerito::hardcoded_config_28_verifier(), &proof),
        30 => verify(&ligerito::hardcoded_config_30_verifier(), &proof),
        _ => return Err("Unsupported config size"),
    };

    std::process::exit(if result.is_ok() { 0 } else { 1 });
}
```

**This works!** The PolkaVM guest can verify real Ligerito proofs.

### 2. Ligerito Prover (⚠️ Exists but Not Used)

**File**: `circuit/src/prover.rs`

**Status**: ⚠️ **EXISTS BUT NOT WIRED UP**

```rust
use ligerito::{hardcoded_config_20, prover, FinalizedLigeritoProof};

pub fn prove_transfer(instance: &TransferInstance) -> Result<StateTransitionProof> {
    // Build constraint polynomial
    let poly = build_constraint_polynomial(instance)?;

    // Generate Ligerito proof (REAL LIGERITO!)
    let config = hardcoded_config_20(
        PhantomData::<BinaryElem32>,
        PhantomData::<BinaryElem128>,
    );

    let pcs_proof = prover(&config, &poly)?;  // ← REAL LIGERITO PROVER

    Ok(StateTransitionProof {
        pcs_proof,
        sender_commitment_old: instance.sender_commitment_old,
        sender_commitment_new: instance.sender_commitment_new,
        receiver_commitment_old: instance.receiver_commitment_old,
        receiver_commitment_new: instance.receiver_commitment_new,
    })
}
```

**Problem**: This function EXISTS but **nobody calls it**!

### 3. AccidentalComputer (✅ Used, No Ligerito)

**File**: `circuit/src/accidental_computer.rs`

**Status**: ✅ **FULLY USED** (but doesn't use Ligerito)

```rust
pub fn prove_with_accidental_computer(
    config: &AccidentalComputerConfig,
    instance: &TransferInstance,
) -> Result<AccidentalComputerProof> {
    // Step 1: Serialize data
    let data = serialize_transfer_instance(instance)?;

    // Step 2: ZODA encode (NOT Ligerito!)
    let (commitment, shards) = Zoda::<Sha256>::encode(&coding_config, data)?;

    // Step 3: Return ZODA proof
    Ok(AccidentalComputerProof {
        zoda_commitment: commitment.encode().to_vec(),
        shard_indices,
        shards: shard_bytes,
        sender_commitment_old: instance.sender_commitment_old,
        sender_commitment_new: instance.sender_commitment_new,
        receiver_commitment_old: instance.receiver_commitment_old,
        receiver_commitment_new: instance.receiver_commitment_new,
    })
}

pub fn verify_accidental_computer(
    config: &AccidentalComputerConfig,
    proof: &AccidentalComputerProof,
) -> Result<bool> {
    // Verify ZODA shards (NOT Ligerito!)
    for (index, shard_bytes) in proof.shard_indices.iter().zip(&proof.shards) {
        let shard = <Zoda<Sha256> as Scheme>::Shard::read_cfg(&mut buf, &codec_config)?;
        let (_checking_data, checked_shard, _reshard) =
            Zoda::<Sha256>::reshard(&coding_config, &commitment, *index, shard)?;
        checked_shards.push(checked_shard);
    }

    // Just check we have enough shards
    Ok(checked_shards.len() >= config.minimum_shards as usize)
}
```

**This is what's actually running!** Full nodes use this for verification.

**Key Point**: AccidentalComputer uses **ZODA directly**, not Ligerito. This is by design (AccidentalComputer pattern).

### 4. Light Client Proof Extraction (❌ Placeholder)

**File**: `blockchain/src/light_client.rs`

**Status**: ❌ **PLACEHOLDER ONLY**

```rust
pub fn extract_succinct_proof(
    accidental_proof: &AccidentalComputerProof,
    config_size: u32,
) -> Result<LigeritoSuccinctProof> {
    // Step 1: Decode ZODA shards ✅
    let commitment: Summary = Read::read_cfg(&mut commitment_bytes, &())?;

    // Step 2: Collect shards ✅
    let mut checked_shards = Vec::new();
    for (index, shard_bytes) in proof.shard_indices.iter().zip(&proof.shards) {
        let shard = <Zoda<Sha256> as Scheme>::Shard::read_cfg(&mut buf, &codec_config)?;
        let (_checking_data, checked_shard, _reshard) =
            Zoda::<Sha256>::reshard(&coding_config, &commitment, *index, shard)?;
        checked_shards.push((index, checked_shard));
    }

    // Step 3: Reconstruct polynomial ❌ TODO
    // TODO: Actual ZODA recovery - for now, placeholder
    //
    // In real implementation:
    // let data = Zoda::<Sha256>::recover(&checked_shards)?;
    // let polynomial = reconstruct_polynomial_from_data(&data);
    // let ligerito_proof = ligerito::prove(&config, &polynomial)?;

    // Step 4: Generate Ligerito proof ❌ PLACEHOLDER
    let proof_bytes = create_placeholder_ligerito_proof(
        config_size,
        &accidental_proof.sender_commitment_old,
        &accidental_proof.sender_commitment_new,
        &accidental_proof.receiver_commitment_old,
        &accidental_proof.receiver_commitment_new,
    )?;

    Ok(LigeritoSuccinctProof {
        proof_bytes,  // ← NOT A REAL LIGERITO PROOF!
        config_size,
        // ...
    })
}
```

**This is a placeholder!** It doesn't actually call Ligerito.

## What's Actually Running

### Current Data Flow (What Works Today)

```
Client
  ↓
prove_with_accidental_computer()
  ↓ Uses ZODA encoding (Commonware)
  ↓ NO Ligerito involved
AccidentalComputerProof
  ↓
Validator Mempool
  ↓
Block Production
  ↓
Full Node Verification
  ↓
verify_accidental_computer()
  ↓ Verifies ZODA shards (Commonware)
  ↓ NO Ligerito involved
✅ NOMT State Update
```

**Ligerito is NOT in this flow!**

### Intended Data Flow (Not Implemented)

```
Client
  ↓
prove_with_accidental_computer()
  ↓ ZODA encoding
AccidentalComputerProof
  ↓
Light Client
  ↓
extract_succinct_proof() ❌ PLACEHOLDER
  ↓
  ├─ Recover data from ZODA
  ├─ Reconstruct polynomial
  └─ ligerito::prove() ← WOULD USE LIGERITO
  ↓
LigeritoSuccinctProof
  ↓
PolkaVM Verifier ✅ WORKS
  ↓
ligerito::verify() ← USES LIGERITO
  ↓
✅ Valid/Invalid
```

**Gap**: The `extract_succinct_proof()` function doesn't actually generate Ligerito proofs!

## Components Status

| Component | Status | Ligerito Usage |
|-----------|--------|----------------|
| **PolkaVM Verifier** | ✅ Complete | ✅ Uses `ligerito::verify()` |
| **Circuit Prover** | ⚠️ Exists | ✅ Uses `ligerito::prover()` but not called |
| **AccidentalComputer Prover** | ✅ Complete | ❌ Uses ZODA only (by design) |
| **AccidentalComputer Verifier** | ✅ Complete | ❌ Uses ZODA only (by design) |
| **Proof Extraction** | ❌ Placeholder | ❌ Should use Ligerito but doesn't |
| **Light Client** | ⚠️ Architecture only | ⚠️ Would use Ligerito (not impl) |

## Why This Confusion Exists

### The AccidentalComputer Pattern

**Key Insight**: AccidentalComputer is designed to **reuse ZODA encoding**, not to explicitly use Ligerito!

```
Traditional Approach:
  Data → Reed-Solomon (DA) + Ligerito PCS (ZK) = Two steps

AccidentalComputer Approach:
  Data → ZODA (DA) = Also serves as PCS (ZK) = One step!
```

**The ZODA encoding IS the polynomial commitment!**

So when we use AccidentalComputer, we're using the **philosophy** of Ligerito (binary fields, Reed-Solomon as PCS) but not calling `ligerito::prove()` explicitly.

### Two Proof Types

```
Type 1: AccidentalComputerProof (What full nodes use)
  ├─ ZODA commitment
  ├─ ZODA shards (Reed-Solomon)
  └─ Verification: Check ZODA shards
  └─ Size: ~10KB - 1MB

Type 2: LigeritoSuccinctProof (What light clients would use)
  ├─ Polynomial evaluations
  ├─ Sumcheck proofs
  ├─ Opening proofs
  └─ Verification: ligerito::verify()
  └─ Size: ~1-30KB
```

**Full nodes** use Type 1 (AccidentalComputer - no explicit Ligerito)
**Light clients** would use Type 2 (Succinct - explicit Ligerito) ← NOT IMPLEMENTED

## What Needs To Be Done

### High Priority: Complete Light Client Flow

```rust
// Fix: blockchain/src/light_client.rs
pub fn extract_succinct_proof(
    accidental_proof: &AccidentalComputerProof,
    config_size: u32,
) -> Result<LigeritoSuccinctProof> {
    // Step 1: Recover data from ZODA shards ✅ Works
    let data = recover_from_zoda_shards(&accidental_proof)?;

    // Step 2: Reconstruct polynomial from data ❌ TODO
    let polynomial = reconstruct_polynomial_from_data(&data)?;

    // Step 3: Generate Ligerito proof ❌ TODO
    use ligerito::{prover, hardcoded_config_24_prover};
    let config = hardcoded_config_24_prover(
        PhantomData::<BinaryElem32>,
        PhantomData::<BinaryElem128>,
    );

    let ligerito_proof: FinalizedLigeritoProof<BinaryElem32, BinaryElem128> =
        prover(&config, &polynomial)?;

    // Step 4: Serialize
    let proof_bytes = bincode::serialize(&ligerito_proof)?;

    Ok(LigeritoSuccinctProof {
        proof_bytes,  // ← NOW A REAL LIGERITO PROOF!
        config_size,
        // ...
    })
}
```

### Medium Priority: Optional Direct Ligerito Path

```rust
// Add: circuit/src/lib.rs
pub fn prove_with_ligerito(instance: &TransferInstance) -> Result<StateTransitionProof> {
    // Build polynomial from constraints
    let poly = build_constraint_polynomial(instance)?;

    // Use Ligerito directly (not ZODA)
    use ligerito::{prover, hardcoded_config_20};
    let config = hardcoded_config_20(
        PhantomData::<BinaryElem32>,
        PhantomData::<BinaryElem128>,
    );

    let pcs_proof = prover(&config, &poly)?;

    Ok(StateTransitionProof {
        pcs_proof,
        // ...
    })
}
```

This would provide an alternative to AccidentalComputer for clients who want pure Ligerito proofs.

## Answering Your Question

> "okay and is ligerito being used now?"

**Partially**:

1. **PolkaVM Verifier** ✅ - YES, uses `ligerito::verify()` (fully functional)
2. **AccidentalComputer** ❌ - NO, uses ZODA directly (by design!)
3. **Light Client** ❌ - NO, proof extraction is placeholder
4. **Circuit Prover** ❌ - Code exists but not called

**In Production**:
- Full nodes: Use ZODA (AccidentalComputer pattern) ← No explicit Ligerito
- Light clients: Not implemented yet ← Would use Ligerito

**The Gap**: We have the verifier working, but not the full proof generation pipeline for light clients.

## Recommendation

### Option A: Just Use AccidentalComputer (Simpler)

**Approach**: Don't worry about explicit Ligerito proofs, just use ZODA everywhere.

**Pros**:
- ✅ Already working
- ✅ Simpler architecture
- ✅ Follows AccidentalComputer philosophy

**Cons**:
- ❌ Light clients need to download full ZODA shards (~MB)
- ❌ No succinct proof option

### Option B: Complete Ligerito Integration (More Flexible)

**Approach**: Wire up proof extraction to generate real Ligerito proofs.

**Pros**:
- ✅ Light clients get succinct proofs (~KB)
- ✅ Both proof types available
- ✅ More flexible

**Cons**:
- ⚠️ More complexity
- ⚠️ Need to implement proof extraction
- ⚠️ Additional testing needed

### Recommended: Option B (Complete Integration)

**Rationale**: You already have the PolkaVM verifier working! Just need to wire up proof generation.

**Estimated Time**: 2-3 days

**Tasks**:
1. Implement ZODA data recovery (1 day)
2. Implement polynomial reconstruction (1 day)
3. Wire up `ligerito::prover()` call (2 hours)
4. Testing and integration (1 day)

## Code Status Summary

```
Ligerito Crate:
  ├─ ligerito::verify()    ✅ Used in PolkaVM verifier
  ├─ ligerito::prover()    ⚠️ Exists but not called
  └─ hardcoded_config_*()  ✅ Used in PolkaVM verifier

Our Code:
  ├─ prove_transfer()         ⚠️ Exists but not called
  ├─ prove_with_accidental()  ✅ USED (primary path)
  ├─ verify_accidental()      ✅ USED (primary path)
  ├─ extract_succinct_proof() ❌ PLACEHOLDER
  └─ PolkaVM verifier guest   ✅ WORKS
```

## Conclusion

**Short Answer**: Ligerito is being used in the **PolkaVM verifier** but **NOT** in the proof generation pipeline yet.

**Current State**: Full nodes use AccidentalComputer (ZODA only), which is the intended primary design.

**Missing**: Light client proof extraction that would actually use `ligerito::prover()` to generate succinct proofs.

**Next Step**: Complete the proof extraction implementation to enable true light client support with succinct Ligerito proofs.
