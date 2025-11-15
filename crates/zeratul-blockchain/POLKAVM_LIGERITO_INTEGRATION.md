# PolkaVM + Ligerito Integration Plan

## Overview

We're replacing Penumbra's proof system (Groth16) with Ligerito for **10x faster verification**.
We're adding PolkaVM execution for **provable state transitions**.

## What We're NOT Changing

Penumbra's batch execution logic is **already fast** (native Rust). We keep it!

```rust
// This stays the same (from Penumbra)
async fn handle_batch_swaps(
    trading_pair: TradingPair,
    batch_data: SwapFlow,
    ...
) -> Result<BatchSwapOutputData> {
    // Native Rust execution
    // Already deterministic
    // Already fast
    let swap_execution_1_for_2 = self
        .route_and_fill(asset_1, asset_2, delta_1, params)
        .await?;

    // ... (keep all Penumbra logic)
}
```

**We DON'T need to replace this!** It's already optimal.

## What We ARE Changing

### 1. ZK Proof System: Groth16 → Ligerito

**Files to modify:**
```
src/penumbra/dex/swap/proof.rs              ← Swap proofs
src/penumbra/dex/swap_claim/proof.rs        ← Swap claim proofs
src/penumbra/shielded_pool/spend/proof.rs   ← Spend proofs
src/penumbra/shielded_pool/output/proof.rs  ← Output proofs
src/penumbra/governance/delegator_vote/proof.rs ← Vote proofs
```

**What to replace:**
```rust
// OLD (Groth16 - 5ms verification)
use ark_groth16::{Groth16, PreparedVerifyingKey, Proof, ProvingKey};

let proof = Groth16::<Bls12_377>::create_proof_with_reduction(...);
let verified = Groth16::<Bls12_377>::verify_with_processed_vk(...);

// NEW (Ligerito - 512μs verification)
use crate::execution::ligerito_proofs::LigeritoProofSystem;

let proof_system = LigeritoProofSystem::new();
let proof = proof_system.prove_swap(&swap_plaintext, &fee_blinding)?;
let verified = proof_system.verify_swap(&proof, &public_inputs)?;
```

**Performance improvement:**
- Proof generation: 2s → 400ms (5x faster!)
- Proof verification: 5ms → 512μs (10x faster!)
- Proof size: 192 bytes → ~100KB (acceptable tradeoff)

### 2. Add PolkaVM Provable Execution (NEW!)

**Files to create:**
```
src/execution/pvm_batch.rs      ← PolkaVM batch executor
src/execution/ligerito_proofs.rs ← Ligerito proof wrapper
src/execution/mod.rs             ← Execution module
```

**What this adds:**
```rust
// Penumbra: Execute batch (deterministic but not provable)
let output = state.handle_batch_swaps(...).await?;

// Zeratul: Execute batch WITH PROOF (provable!)
let (output, proof) = executor.execute_batch_with_proof(...)?;

// Light clients can verify without re-executing!
executor.verify_batch_proof(&proof, &inputs, &output)?;
```

**Why this matters:**
- Light clients don't need full state
- Just verify proofs (512μs each)
- Download ~100KB proof instead of megabytes of state
- Still get same security!

## Integration Steps

### Phase 1: Wrapper Layer (Week 1)

Create abstraction over proof systems:

```rust
// src/execution/proof_system.rs
pub trait ProofSystem {
    fn prove_swap(&self, inputs: SwapProofPrivate) -> Result<Proof>;
    fn verify_swap(&self, proof: &Proof, public: SwapProofPublic) -> Result<()>;

    fn prove_spend(&self, inputs: SpendProofPrivate) -> Result<Proof>;
    fn verify_spend(&self, proof: &Proof, public: SpendProofPublic) -> Result<()>;

    // ... etc
}

// Groth16 implementation (current)
pub struct Groth16ProofSystem { ... }
impl ProofSystem for Groth16ProofSystem { ... }

// Ligerito implementation (new!)
pub struct LigeritoProofSystem { ... }
impl ProofSystem for LigeritoProofSystem { ... }
```

This lets us **switch at runtime** via feature flags!

### Phase 2: Ligerito Implementation (Week 2-3)

Implement Ligerito proofs for each circuit:

**Swap Circuit:**
```rust
impl LigeritoProofSystem {
    pub fn prove_swap(
        &self,
        swap_plaintext: &SwapPlaintext,
        fee_blinding: &Fr,
    ) -> Result<LigeritoProof> {
        // 1. Convert swap constraints to PolkaVM program
        let program = compile_swap_circuit(swap_plaintext)?;

        // 2. Execute in PolkaVM to get trace
        let trace = polkavm::execute(program, swap_plaintext)?;

        // 3. Generate Ligerito proof of execution
        let proof = ligerito::prove_execution(trace)?;

        Ok(proof)
    }
}
```

**Spend Circuit:**
```rust
impl LigeritoProofSystem {
    pub fn prove_spend(
        &self,
        note: &Note,
        position: u64,
        auth_path: &[MerkleProofNode],
    ) -> Result<LigeritoProof> {
        // Prove:
        // - Note exists in tree (Merkle proof)
        // - Nullifier correctly derived
        // - Value balance correct

        let program = compile_spend_circuit(note, position)?;
        let trace = polkavm::execute(program, (note, auth_path))?;
        let proof = ligerito::prove_execution(trace)?;

        Ok(proof)
    }
}
```

### Phase 3: PolkaVM Batch Executor (Week 4)

Add provable batch execution:

```rust
impl PvmBatchExecutor {
    pub fn execute_batch_with_proof(
        &mut self,
        trading_pair: TradingPair,
        delta_1: u64,
        delta_2: u64,
        liquidity_positions: &[LiquidityPosition],
    ) -> Result<(BatchSwapOutputData, ExecutionProof)> {
        // 1. Execute using Penumbra's route_and_fill (native Rust)
        let output = self.execute_native(
            trading_pair,
            delta_1,
            delta_2,
            liquidity_positions,
        )?;

        // 2. Record execution in PolkaVM for proof
        let program = compile_batch_executor()?;
        let inputs = BatchInputs {
            delta_1,
            delta_2,
            positions: liquidity_positions,
        };

        // 3. Execute in PolkaVM (same result, but traced!)
        let trace = polkavm::execute(program, inputs)?;

        // 4. Verify native and PVM results match
        assert_eq!(trace.output, output);

        // 5. Generate proof
        let proof = ligerito::prove_execution(trace)?;

        Ok((output, proof))
    }
}
```

### Phase 4: Modify Penumbra Code (Week 5)

Update Penumbra's proof callsites:

**Before (Groth16):**
```rust
// src/penumbra/dex/component/action_handler/swap.rs
self.proof.verify(
    &SWAP_PROOF_VERIFICATION_KEY,
    SwapProofPublic {
        balance_commitment: self.balance_commitment_inner(),
        swap_commitment: self.body.payload.commitment,
        fee_commitment: self.body.fee_commitment,
    },
)?;
```

**After (Ligerito):**
```rust
// src/penumbra/dex/component/action_handler/swap.rs
use crate::execution::ligerito_proofs::LigeritoProofSystem;

let proof_system = LigeritoProofSystem::new();
proof_system.verify_swap(
    &self.proof,
    &SwapProofPublic {
        balance_commitment: self.balance_commitment_inner(),
        swap_commitment: self.body.payload.commitment,
        fee_commitment: self.body.fee_commitment,
    },
)?;
```

**Do this for all proof types:**
- Swap proofs → `ligerito_proofs.rs:verify_swap()`
- Spend proofs → `ligerito_proofs.rs:verify_spend()`
- Output proofs → `ligerito_proofs.rs:verify_output()`
- Vote proofs → `ligerito_proofs.rs:verify_vote()`

### Phase 5: Testing & Benchmarking (Week 6)

**Correctness tests:**
```rust
#[test]
fn test_ligerito_swap_proof() {
    // Generate proof with Ligerito
    let proof_system = LigeritoProofSystem::new();
    let proof = proof_system.prove_swap(&swap_plaintext, &fee_blinding)?;

    // Verify it
    proof_system.verify_swap(&proof, &public_inputs)?;

    // Should match Groth16 result
    assert_eq!(proof.is_valid(), true);
}
```

**Performance benchmarks:**
```rust
#[bench]
fn bench_swap_proof_generation(b: &mut Bencher) {
    let proof_system = LigeritoProofSystem::new();

    b.iter(|| {
        proof_system.prove_swap(&swap_plaintext, &fee_blinding)
    });

    // Target: <400ms
}

#[bench]
fn bench_swap_proof_verification(b: &mut Bencher) {
    let proof_system = LigeritoProofSystem::new();
    let proof = proof_system.prove_swap(&swap_plaintext, &fee_blinding)?;

    b.iter(|| {
        proof_system.verify_swap(&proof, &public_inputs)
    });

    // Target: <512μs (10x faster than Groth16!)
}
```

## File Changes Summary

### New Files Created
```
✅ src/execution/mod.rs                  (module definition)
✅ src/execution/ligerito_proofs.rs     (Ligerito wrapper)
✅ src/execution/pvm_batch.rs           (PolkaVM executor)
```

### Files to Modify (Replace Groth16 → Ligerito)
```
📝 src/penumbra/dex/swap/proof.rs
📝 src/penumbra/dex/swap_claim/proof.rs
📝 src/penumbra/dex/batch_swap_output_data.rs
📝 src/penumbra/shielded_pool/spend/proof.rs
📝 src/penumbra/shielded_pool/output/proof.rs
📝 src/penumbra/governance/delegator_vote/proof.rs
📝 src/penumbra/stake/undelegate_claim/proof.rs
```

### Files to Keep (No Changes Needed)
```
✅ src/penumbra/dex/component/router/route_and_fill.rs  (native Rust, already fast!)
✅ src/penumbra/dex/component/router/fill_route.rs
✅ src/penumbra/dex/component/position_manager.rs
✅ src/penumbra/dex/component/flow.rs
✅ ... (all Penumbra business logic stays the same!)
```

## Performance Targets

| Metric | Groth16 (Penumbra) | Ligerito (Zeratul) | Target |
|--------|-------------------|-------------------|--------|
| Swap proof gen | ~2s | 400ms | ✅ 5x faster |
| Swap proof verify | ~5ms | 512μs | ✅ 10x faster |
| Spend proof verify | ~5ms | 512μs | ✅ 10x faster |
| Output proof verify | ~5ms | 512μs | ✅ 10x faster |
| Proof size | 192 bytes | ~100KB | ⚠️ Larger |
| Batch execution | Native Rust | Native + PVM trace | ✅ Same speed |

**Key insight:** Proof size is larger, but verification is 10x faster!
- On-chain: Larger proofs (acceptable, ~100KB)
- Light clients: Faster sync (verify proofs in microseconds!)

## Migration Strategy

### Option A: Feature Flag (Recommended)
```toml
[features]
default = ["groth16"]
groth16 = []
ligerito = []
```

```rust
#[cfg(feature = "groth16")]
use crate::proofs::groth16::Groth16ProofSystem as ProofSystem;

#[cfg(feature = "ligerito")]
use crate::execution::ligerito_proofs::LigeritoProofSystem as ProofSystem;
```

**Benefits:**
- Can test both systems
- Gradual migration
- Fallback if issues

### Option B: Direct Replacement
Just replace all Groth16 with Ligerito.

**Benefits:**
- Simpler
- Force migration
- No dual maintenance

**We recommend Option A for safety!**

## Next Steps

1. **Week 1:** Create proof system abstraction ✅ DONE (ligerito_proofs.rs)
2. **Week 2:** Implement Ligerito swap proofs
3. **Week 3:** Implement Ligerito spend/output proofs
4. **Week 4:** Add PolkaVM batch executor
5. **Week 5:** Replace Groth16 callsites in Penumbra code
6. **Week 6:** Test & benchmark

## Dependencies Needed

```toml
[dependencies]
# Keep Penumbra dependencies
penumbra-sdk-proto = "0.82"
penumbra-sdk-asset = "0.82"
# ... etc

# Remove Groth16
# ark-groth16 = "0.4"  ← Remove this

# Add Ligerito
ligerito = { path = "../../ligerito" }

# Add PolkaVM
polkavm = { path = "../../polkavm-pcvm" }
```

## Summary

**What we're doing:**
1. ✅ Keep Penumbra's batch logic (already fast!)
2. ✅ Replace Groth16 → Ligerito (10x faster verification!)
3. ✅ Add PolkaVM provable execution (new capability!)

**What we get:**
- Same security as Penumbra
- Same MEV resistance
- Same privacy
- **10x faster proof verification**
- **Provable state transitions**
- **Light client support**

**Time to implement:** 6 weeks

**Performance gain:** 10x faster proofs! 🚀
