# Ligerito Design Philosophy: AccidentalComputer First

**Date**: 2025-11-12

## You're Right - I Had It Backwards!

**The Actual Design**: Ligerito was **designed from the ground up** to work with the "AccidentalComputer" pattern. It's not an add-on or alternative approach - it's the **core design**.

## What Ligerito Actually Is

From the paper (https://angeris.github.io/papers/ligerito.pdf):

### The Key Insight (Section 5)

**Most polynomial commitment schemes require two separate encodings**:
1. Data availability encoding (Reed-Solomon for DA)
2. Polynomial commitment encoding (separate PCS for ZK proofs)

**Ligerito's innovation**: Use the **same encoding** for both!

```
Traditional approach:
Data → Reed-Solomon (for DA) → Merkle tree
Data → Separate PCS (for ZK) → Proof

Ligerito approach:
Data → Reed-Solomon (for DA) → Merkle tree
       ↑
       This IS the polynomial commitment! (AccidentalComputer)
```

### Why "AccidentalComputer"?

The Reed-Solomon encoding you were **already doing for data availability** "accidentally" gives you a polynomial commitment scheme for free.

**It's not an optimization - it's the design.**

## How We're Actually Using It

### ✅ Correct: AccidentalComputer (Full Nodes)

```rust
// circuit/src/accidental_computer.rs
pub fn prove_with_accidental_computer(
    config: &AccidentalComputerConfig,
    instance: &TransferInstance,
) -> Result<AccidentalComputerProof> {
    // Serialize transfer data
    let data = serialize_transfer_instance(instance)?;
    
    // ZODA encode (Reed-Solomon)
    // This creates BOTH:
    // 1. Data availability encoding
    // 2. Polynomial commitment for Ligerito!
    let (commitment, shards) = Zoda::<Sha256>::encode(&config, data)?;
    
    Ok(AccidentalComputerProof {
        zoda_commitment: commitment,  // This IS our Ligerito commitment!
        shards,
        // ...
    })
}

pub fn verify_accidental_computer(
    config: &AccidentalComputerConfig,
    proof: &AccidentalComputerProof,
) -> Result<bool> {
    // Verify using Ligerito + ZODA commitment
    // The ZODA commitment serves as the polynomial commitment
    let valid = Zoda::<Sha256>::verify(
        &config,
        &proof.zoda_commitment,
        &proof.shards
    )?;
    
    Ok(valid)
}
```

**This is the primary use case Ligerito was designed for!**

### ⚠️ Incorrect: Standalone Prover/Verifier

```rust
// circuit/src/prover.rs - This is WRONG for the design
use ligerito::{hardcoded_config_20, prover};

pub fn prove_transfer(instance: &TransferInstance) -> Result<StateTransitionProof> {
    let poly = build_constraint_polynomial(instance)?;
    
    // ❌ This creates a SEPARATE polynomial commitment
    // Not reusing ZODA encoding!
    let pcs_proof = prover(&config, &poly)?;
    
    Ok(StateTransitionProof { pcs_proof, ... })
}
```

**This defeats the purpose!** We're doing double work:
1. ZODA encoding (for DA)
2. Separate Ligerito proof (for ZK)

We should only do #1 and reuse it for #2.

## The Three-Tier Network (Corrected)

### Validators
```
Execute transactions
    ↓
Create ZODA encoding (Reed-Solomon for DA)
    ↓
The ZODA commitment IS the Ligerito commitment
    ↓
Broadcast block + ZODA shards
```

### Full Nodes
```
Download block + ZODA shards
    ↓
Re-execute transactions
    ↓
Verify using AccidentalComputer:
  - ZODA commitment proves correctness
  - Reed-Solomon encoding enables recovery
  - No separate PCS needed!
```

### Light Clients
```
Download block header + Ligerito proof
    ↓
For light clients, we CAN'T use AccidentalComputer because:
  - They don't have full ZODA shards (too big)
  - They can't re-execute (no state)
    ↓
Instead: Verify succinct Ligerito proof in PolkaVM
  - Extract commitments from ZODA encoding
  - Verify polynomial evaluation proofs
  - Much smaller proof size
```

## PolkaVM's Role (Clarified)

PolkaVM is **still useful**, but for a **different reason**:

### For Light Clients
Light clients need **succinct proofs** (small, ~few KB). They can't use AccidentalComputer directly because:
- ZODA shards are large (full encoding)
- Requires re-execution

**Solution**: Extract a **succinct proof** from the ZODA commitment and verify it in PolkaVM.

```rust
// Validator creates ZODA encoding
let (zoda_commitment, shards) = Zoda::encode(&data)?;

// Extract succinct proof for light clients
let succinct_proof = extract_succinct_ligerito_proof(&zoda_commitment)?;

// Light client verifies in PolkaVM
let polkavm = PolkaVM::new("ligerito_verifier.polkavm")?;
let valid = polkavm.verify(&succinct_proof)?;  // Fast, small proof
```

### For On-Chain Verification
If you want to verify Ligerito proofs **on another blockchain** (e.g., Polkadot parachain), you use PolkaVM because:
- On-chain storage is expensive (need succinct proofs)
- Deterministic execution required
- Cross-platform compatibility

## What I Got Wrong

### Mistake 1: Treating AccidentalComputer as Optional
I said: "AccidentalComputer OR standalone prover"

**Reality**: AccidentalComputer is the **primary design**. Standalone proving is for special cases (light clients, on-chain verification).

### Mistake 2: Separate Prover/Verifier in Circuit
The `circuit/prover.rs` and `circuit/verifier.rs` that use `ligerito::{prover, verifier}` directly are doing **redundant work**.

**Should be**:
- Validators → Create ZODA encoding (AccidentalComputer)
- Full nodes → Verify ZODA encoding (AccidentalComputer)
- Light clients → Verify succinct extract (PolkaVM)

### Mistake 3: Not Emphasizing ZODA Integration
Ligerito's whole point is: **If you're already doing ZODA for DA, use it for ZK proofs too!**

## Corrected Architecture

```
┌──────────────────────────────────────────────────────┐
│ VALIDATORS                                           │
│  1. Execute transactions                             │
│  2. Create ZODA encoding (Reed-Solomon)              │
│     - Data availability ✓                            │
│     - Polynomial commitment ✓ (same encoding!)       │
│  3. Broadcast: block + ZODA shards                   │
└──────────────────────────────────────────────────────┘
                    │
        ┌───────────┴───────────┐
        │                       │
        ▼                       ▼
┌────────────────────┐  ┌────────────────────┐
│ FULL NODES         │  │ LIGHT CLIENTS      │
│ - Download shards  │  │ - Download header  │
│ - Re-execute       │  │ - Download succinct│
│ - Verify via ZODA  │  │   Ligerito proof   │
│   (AccidentalComp) │  │ - Verify in PolkaVM│
└────────────────────┘  └────────────────────┘
```

## Why PolkaVM Still Matters

Even though AccidentalComputer is primary, PolkaVM is still critical for:

1. **Light clients** - Need succinct proofs, can't use full ZODA shards
2. **On-chain verification** - Cross-chain bridges, parachains
3. **Constrained environments** - Mobile, browsers, IoT
4. **Deterministic verification** - Same results everywhere

## What We Should Actually Build

### Phase 1: AccidentalComputer Integration (Primary)
```rust
// blockchain/src/engine.rs
pub fn produce_block(&mut self, txs: Vec<Transaction>) -> Result<Block> {
    // Execute
    let state_transition = self.execute_transactions(&txs)?;
    
    // ZODA encode (this IS our Ligerito proof!)
    let (zoda_commitment, shards) = 
        accidental_computer::prove(&state_transition)?;
    
    Ok(Block {
        transactions: txs,
        zoda_commitment,  // DA + ZK in one!
        zoda_shards: shards,
    })
}

pub fn apply_block(&mut self, block: &Block) -> Result<()> {
    // Re-execute
    let computed = self.execute_transactions(&block.transactions)?;
    
    // Verify via AccidentalComputer
    let valid = accidental_computer::verify(
        &block.zoda_commitment,
        &block.zoda_shards,
        &computed
    )?;
    
    if !valid { bail!("Invalid proof"); }
    self.state.commit(computed)
}
```

### Phase 2: Light Client Proofs (Secondary)
```rust
// Extract succinct proof from ZODA commitment
pub fn extract_light_client_proof(
    zoda_commitment: &[u8],
    block_header: &BlockHeader,
) -> Result<LigeritoProof> {
    // Extract just the polynomial evaluations needed
    // Much smaller than full ZODA shards
    ligerito::extract_succinct_proof(zoda_commitment, block_header)
}

// Light client verifies in PolkaVM
pub fn verify_light_client(
    header: &BlockHeader,
    proof: &LigeritoProof,
) -> Result<bool> {
    let polkavm = PolkaVM::new("ligerito_verifier.polkavm")?;
    polkavm.verify(header, proof)
}
```

## Summary: The Actual Ligerito Philosophy

**Ligerito is designed to work WITH data availability encoding, not separate from it.**

The "AccidentalComputer" pattern isn't an optimization or alternative - it's **the core design principle**:

> "If you're already doing Reed-Solomon encoding for data availability (which every blockchain needs), you get polynomial commitments for zero-knowledge proofs FOR FREE."

**Primary use**: AccidentalComputer (full nodes + validators)  
**Secondary use**: PolkaVM (light clients + on-chain verification)

The standalone `prover/verifier` API exists for cases where you **can't** use ZODA (light clients, cross-chain), but it's not the primary design.

## Next Steps (Corrected)

1. ✅ Keep AccidentalComputer implementation (`circuit/accidental_computer.rs`)
2. ⚠️ Remove/deprecate standalone `prover.rs`/`verifier.rs` (or mark as "light client only")
3. ❌ Integrate AccidentalComputer into block production
4. ❌ Add light client proof extraction from ZODA commitments
5. ✅ Keep PolkaVM for light client verification

**The design is: ZODA encoding → AccidentalComputer → Succinct extract (optional, for light clients)**

Not: Separate Ligerito proving → ZODA encoding (two separate steps)
