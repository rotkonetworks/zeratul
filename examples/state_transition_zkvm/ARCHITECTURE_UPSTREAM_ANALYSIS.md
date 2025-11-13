# Architecture: Upstream Dependencies Analysis

**Date**: 2025-11-12
**Critical Question**: How do Commonware and Polkadot SDK fit together?

## The Problem

We're currently mixing **THREE major upstreams**:

1. **Commonware** - Full blockchain framework
2. **Polkadot SDK** - Runtime + consensus framework
3. **Penumbra** - Cryptography primitives

**Risk**: These are competing frameworks - we might be creating an architectural mess!

## Current Dependency Map

```toml
[dependencies]
# Commonware (Full Stack)
commonware-consensus      # ← BFT consensus
commonware-broadcast      # ← Gossip protocol
commonware-p2p            # ← Networking
commonware-runtime        # ← Async runtime
commonware-storage        # ← Storage layer
commonware-cryptography   # ← BLS12-381, Ed25519
commonware-codec          # ← Serialization
commonware-coding         # ← ZODA (Reed-Solomon)

# Polkadot SDK (Partial)
sp-core                   # ← Bandersnatch Ring VRF only
parity-scale-codec        # ← Codec compatibility

# Penumbra (Crypto Only)
decaf377-frost            # ← FROST threshold sigs
decaf377-rdsa             # ← Schnorr signatures
decaf377                  # ← Decaf377 curve

# NOMT (Storage)
nomt                      # ← Authenticated state
```

## The Confusion: Three Frameworks

### Framework 1: Commonware (Primary)

**What it is**: Complete blockchain framework (like Cosmos SDK)

**What we use**:
```
Networking Layer:
  commonware-p2p          → TCP/QUIC networking
  commonware-broadcast    → Gossip protocol
  commonware-resolver     → Peer discovery

Consensus Layer:
  commonware-consensus    → BFT consensus (Simplex)
  commonware-cryptography → BLS12-381 threshold sigs

Data Availability:
  commonware-coding       → ZODA (Reed-Solomon)
  commonware-codec        → Serialization

Runtime:
  commonware-runtime      → Async task execution
  commonware-storage      → Key-value storage
```

**Our Usage**: **PRIMARY FRAMEWORK** - This is our base!

### Framework 2: Polkadot SDK (Selective)

**What it is**: Complete parachain framework (Runtime + Consensus)

**What we use**:
```
Cryptography ONLY:
  sp-core                 → Bandersnatch Ring VRF
  parity-scale-codec      → SCALE codec (for Bandersnatch types)

NOT USED:
  ❌ frame-system         → (We use Commonware instead)
  ❌ frame-support        → (We use Commonware instead)
  ❌ pallet-*             → (We use Commonware instead)
  ❌ sc-consensus         → (We use Commonware consensus)
  ❌ sc-network           → (We use Commonware p2p)
```

**Our Usage**: **CRYPTO LIBRARY ONLY** - Just for Bandersnatch!

### Framework 3: Penumbra (Selective)

**What it is**: Privacy-focused blockchain

**What we use**:
```
Cryptography ONLY:
  decaf377-frost          → FROST threshold signatures
  decaf377-rdsa           → Schnorr signatures
  decaf377                → Decaf377 curve primitives

NOT USED:
  ❌ penumbra-tct         → (We use NOMT instead)
  ❌ penumbra-dex         → (Not needed)
  ❌ penumbra-chain       → (We use Commonware instead)
```

**Our Usage**: **CRYPTO LIBRARY ONLY** - Just for FROST!

## The Reality: NOT Mixed Frameworks

### What We're Actually Building

```
┌──────────────────────────────────────────────────┐
│           Zeratul Blockchain                     │
│         (Commonware-based)                       │
├──────────────────────────────────────────────────┤
│                                                  │
│  Application Layer                               │
│    ├─ State Transition Circuit (Ligerito)       │
│    ├─ AccidentalComputer (ZODA)                 │
│    └─ NOMT (Authenticated State)                │
│                                                  │
│  Consensus Layer (JAM-inspired)                  │
│    ├─ Safrole (Commonware BFT)                  │
│    ├─ Bandersnatch Ring VRF (sp-core)           │ ← Polkadot SDK
│    └─ FROST (decaf377-frost)                    │ ← Penumbra
│                                                  │
│  Network Layer                                   │
│    ├─ commonware-p2p (TCP/QUIC)                 │
│    ├─ commonware-broadcast (Gossip)             │
│    └─ commonware-resolver (Discovery)           │
│                                                  │
│  Storage Layer                                   │
│    ├─ NOMT (State)                              │
│    └─ commonware-storage (Metadata)             │
│                                                  │
│  Runtime                                         │
│    └─ commonware-runtime (Async)                │
│                                                  │
└──────────────────────────────────────────────────┘

         Commonware: Primary Framework ✅
         Polkadot SDK: Crypto Library ONLY ✅
         Penumbra: Crypto Library ONLY ✅
```

## Clear Separation of Concerns

### Layer 1: Networking & Consensus (Commonware)

**Primary Framework**: Commonware

```rust
// We use Commonware's consensus
use commonware_consensus::{
    simplex::{Automaton, Finalized},
    Actor as ConsensusActor,
};

// We use Commonware's networking
use commonware_p2p::{Receiver, Recipients, Sender};
use commonware_broadcast::{Broadcast, Processor};

// This is our PRIMARY infrastructure
```

**Why Commonware?**
- ✅ Production-grade BFT consensus
- ✅ QUIC networking (modern)
- ✅ Async runtime (Tokio-compatible)
- ✅ Modular design
- ✅ Well-tested

### Layer 2: Cryptography (Mixed Sources)

**Problem**: No single framework has ALL the crypto we need

**Solution**: Cherry-pick best-in-class crypto:

```rust
// Bandersnatch Ring VRF (Polkadot SDK)
use sp_core::bandersnatch::{
    ring_vrf::{RingContext, RingVrfSignature},
};
// Why? Only production implementation of Bandersnatch Ring VRF

// FROST Threshold Signatures (Penumbra)
use decaf377_frost as frost;
// Why? Best Decaf377-based FROST implementation

// BLS12-381 (Commonware)
use commonware_cryptography::bls12381;
// Why? Needed for BEEFY (future)

// Ed25519 (Commonware)
use commonware_cryptography::ed25519;
// Why? Node identity
```

**This is FINE**: Crypto libraries are independent!

### Layer 3: Application Logic (Custom)

**Our Code**: Built on top of Commonware

```rust
// Our application layer
pub struct Application {
    // Commonware components
    consensus: Arc<Consensus>,
    network: Arc<Network>,
    storage: Arc<Storage>,

    // Our crypto (mixed sources - that's OK!)
    frost_state: FrostState,              // Penumbra
    safrole_state: SafroleState,          // Uses sp-core

    // Our ZK system
    zk_config: AccidentalComputerConfig,  // Ligerito
    nomt: Arc<Mutex<Session>>,            // NOMT
}
```

## The PolkaVM Confusion

### What We Proposed (On-Chain Verifier)

```rust
// Use pallet_revive for on-chain verification
use pallet_revive::{Pallet, Call};

pub fn verify_on_chain(proof: &LigeritoSuccinctProof) -> DispatchResult {
    <pallet_revive::Pallet<T>>::bare_call(
        verifier_address,
        proof_bytes,
        // ...
    )
}
```

**Problem**: This requires **Substrate runtime**!

**Reality**: We're using **Commonware**, not **Substrate**!

### The Architectural Mismatch

```
What pallet_revive needs:
  ┌─────────────────────────────────┐
  │   Substrate Runtime (FRAME)     │
  │                                 │
  │   frame-system                  │
  │   frame-support                 │
  │   pallet-balances               │
  │   pallet-revive ← We want this  │
  └─────────────────────────────────┘

What we actually have:
  ┌─────────────────────────────────┐
  │   Commonware Framework          │
  │                                 │
  │   commonware-consensus          │
  │   commonware-storage            │
  │   Our Application Logic         │
  │   ??? Where does pallet_revive go? │
  └─────────────────────────────────┘
```

**THE MISMATCH**: `pallet_revive` is a **Substrate pallet**, but we're building a **Commonware blockchain**!

## Three Possible Solutions

### Option 1: Abandon On-Chain Verification (Keep It Simple)

**Approach**: Stick with Commonware + crypto libraries only

```
Architecture:
  Commonware Framework (Primary)
    ├─ Consensus: Commonware BFT
    ├─ Network: Commonware p2p
    ├─ Storage: NOMT + Commonware storage
    └─ Crypto Libraries:
        ├─ Bandersnatch (sp-core)       ← Just the crypto!
        ├─ FROST (decaf377-frost)       ← Just the crypto!
        └─ BLS12-381 (commonware-crypto)

  Verification:
    ├─ Native (AccidentalComputer)      ← Off-chain
    └─ Light Client (PolkaVM)           ← Off-chain, client-side

  ❌ NO on-chain verification
```

**Pros**:
- ✅ Clean architecture (one framework)
- ✅ No dependency confusion
- ✅ Simpler to maintain
- ✅ Faster development

**Cons**:
- ❌ No consensus-guaranteed verification
- ❌ No fraud proofs possible
- ❌ Off-chain verification risk

### Option 2: Add Substrate Runtime Layer (Hybrid)

**Approach**: Embed a minimal Substrate runtime JUST for verification

```
Architecture:
  ┌────────────────────────────────────────┐
  │  Commonware Layer (Primary)            │
  │    ├─ Consensus (BFT)                  │
  │    ├─ Network (p2p)                    │
  │    └─ Storage (NOMT)                   │
  └────────────────┬───────────────────────┘
                   │
                   ▼
  ┌────────────────────────────────────────┐
  │  Embedded Substrate Runtime (Minimal)  │
  │    └─ pallet_revive ONLY               │
  │       (for PolkaVM execution)          │
  └────────────────────────────────────────┘
```

**Implementation**:
```rust
// Minimal Substrate runtime
pub struct MinimalRuntime;

impl frame_system::Config for MinimalRuntime {
    type AccountId = [u8; 32];
    type Block = MinimalBlock;
    // ... minimal config
}

impl pallet_revive::Config for MinimalRuntime {
    // Just enough to run PolkaVM
}

// Call from Commonware application
pub fn verify_on_chain(proof: &LigeritoSuccinctProof) -> Result<bool> {
    // Convert to Substrate call
    let call = RuntimeCall::Revive(
        pallet_revive::Call::call {
            dest: verifier_address,
            value: 0,
            gas_limit: 10_000_000,
            data: proof.encode(),
        }
    );

    // Execute in embedded runtime
    call.dispatch(RawOrigin::Signed([0u8; 32]))?;

    Ok(true)
}
```

**Pros**:
- ✅ Can use pallet_revive (on-chain verification)
- ✅ Consensus-guaranteed
- ✅ Fraud proofs possible

**Cons**:
- ❌ Complex architecture (two frameworks)
- ❌ Need to bridge Commonware ↔ Substrate
- ❌ More dependencies
- ❌ Harder to maintain

### Option 3: Use PolkaVM Directly (No Substrate)

**Approach**: Use `polkavm` crate directly, without Substrate runtime

```
Architecture:
  Commonware Framework (Primary)
    └─ PolkaVM Executor (Direct)
        ├─ polkavm::Engine
        ├─ polkavm::Module
        └─ polkavm::Instance

  Implementation:
    ├─ Load verifier binary
    ├─ Execute in Commonware consensus
    └─ All nodes run same PolkaVM code
```

**Implementation**:
```rust
// Direct PolkaVM usage (no Substrate)
use polkavm::{Config, Engine, Linker, Module, ProgramBlob};

pub struct PolkaVMVerifier {
    engine: Engine,
    module: Module,
}

impl PolkaVMVerifier {
    pub fn new(verifier_binary: &[u8]) -> Result<Self> {
        let blob = ProgramBlob::parse(verifier_binary)?;
        let config = Config::default();
        let engine = Engine::new(&config)?;
        let linker = Linker::new();
        let module = Module::from_blob(&engine, &ModuleConfig::default(), blob)?;

        Ok(Self { engine, module })
    }

    pub fn verify_in_consensus(&self, proof: &LigeritoSuccinctProof) -> Result<bool> {
        // This runs INSIDE Commonware consensus
        // All nodes execute the same PolkaVM code
        let mut instance = self.module.instantiate()?;

        let input = self.serialize_proof(proof);
        let result = instance.call_typed(&mut (), "main", &input)?;

        Ok(result == 0)  // 0 = valid
    }
}

// In Commonware consensus
impl Automaton for SafroleAutomaton {
    fn propose(&mut self) -> Vec<AccidentalComputerProof> {
        // Native verification for block production
        self.mempool.drain_verified_proofs()
    }

    fn verify(&mut self, proofs: &[AccidentalComputerProof]) -> bool {
        // PolkaVM verification in consensus
        for proof in proofs {
            let succinct = extract_succinct_proof(proof, 24)?;

            // All nodes run same PolkaVM code (consensus!)
            if !self.polkavm_verifier.verify_in_consensus(&succinct)? {
                return false;
            }
        }
        true
    }
}
```

**Pros**:
- ✅ Clean architecture (stays with Commonware)
- ✅ On-chain verification (in consensus)
- ✅ Deterministic (PolkaVM sandbox)
- ✅ No Substrate dependency
- ✅ Direct control

**Cons**:
- ⚠️ Need to handle gas metering ourselves
- ⚠️ Need to implement host functions
- ⚠️ More work than using pallet_revive

## Recommended Solution: Option 3 (PolkaVM Direct)

**Why Option 3 is best**:

1. **Clean Architecture**: One framework (Commonware)
2. **Still Gets On-Chain Verification**: PolkaVM in consensus
3. **No Framework Mixing**: Just use `polkavm` crate directly
4. **Full Control**: Not dependent on Substrate abstractions

### How It Works

```
┌───────────────────────────────────────────────────┐
│             Commonware Blockchain                 │
│                                                   │
│  Consensus Layer (Simplex BFT)                   │
│    ├─ Block Production                           │
│    │    └─ Native verification (fast)            │
│    │                                              │
│    └─ Block Validation                           │
│         └─ PolkaVM verification (consensus) ✅   │
│             │                                     │
│             ▼                                     │
│      ┌─────────────────────┐                    │
│      │  polkavm::Engine    │                    │
│      │  (embedded)         │                    │
│      │                     │                    │
│      │  ├─ Load verifier   │                    │
│      │  ├─ Execute         │                    │
│      │  └─ Return result   │                    │
│      └─────────────────────┘                    │
│                                                   │
└───────────────────────────────────────────────────┘

No Substrate runtime needed! ✅
```

### Implementation Path

```rust
// 1. Embed PolkaVM in consensus
pub struct SafroleState {
    // Existing fields
    pub config: SafroleConfig,
    pub validators: ValidatorSet,
    // ...

    // NEW: PolkaVM verifier
    pub polkavm_verifier: Arc<PolkaVMVerifier>,
}

// 2. Use in block validation
impl SafroleState {
    pub fn validate_block(&self, block: &Block) -> Result<bool> {
        for proof in &block.proofs {
            // Extract succinct proof
            let succinct = extract_succinct_proof(proof, 24)?;

            // Verify via PolkaVM (in consensus - all nodes agree!)
            if !self.polkavm_verifier.verify(&succinct)? {
                return Ok(false);
            }
        }

        Ok(true)
    }
}

// 3. Gas metering (simple approach)
impl PolkaVMVerifier {
    pub fn verify_with_timeout(&self, proof: &LigeritoSuccinctProof) -> Result<bool> {
        // Simple timeout-based gas metering
        let timeout = Duration::from_millis(100);  // Max 100ms per proof

        tokio::time::timeout(timeout, async {
            self.verify(proof)
        }).await?
    }
}
```

## Updated Architecture Diagram

### What We Actually Have (Clean!)

```
┌──────────────────────────────────────────────────────────┐
│                 Zeratul Blockchain                       │
│              (Commonware Framework)                      │
├──────────────────────────────────────────────────────────┤
│                                                          │
│  Layer 1: Application                                   │
│    ├─ AccidentalComputer (ZODA + ZK)                   │
│    ├─ Ligerito (Binary Field PCS)                      │
│    └─ NOMT (Authenticated State)                        │
│                                                          │
│  Layer 2: Consensus                                     │
│    ├─ Safrole (Commonware BFT)                         │
│    │   └─ PolkaVM Verifier (embedded) ✅               │
│    ├─ Bandersnatch Ring VRF (sp-core) ✅               │
│    └─ FROST (decaf377-frost) ✅                         │
│                                                          │
│  Layer 3: Network                                       │
│    ├─ commonware-p2p (QUIC)                            │
│    ├─ commonware-broadcast (Gossip)                    │
│    └─ commonware-resolver (Discovery)                  │
│                                                          │
│  Layer 4: Storage                                       │
│    ├─ NOMT (State Tree)                                │
│    └─ commonware-storage (Metadata)                    │
│                                                          │
│  Layer 5: Runtime                                       │
│    └─ commonware-runtime (Tokio)                       │
│                                                          │
└──────────────────────────────────────────────────────────┘

   Legend:
   ✅ = Crypto library (not framework)
   Commonware = Primary framework
```

### What We DON'T Have (And Don't Need!)

```
❌ Substrate Runtime (frame-system, frame-support)
❌ Substrate Pallets (pallet-balances, pallet-contracts)
❌ Substrate Consensus (sc-consensus)
❌ Substrate Network (sc-network)
❌ Penumbra Chain (penumbra-chain, penumbra-app)
❌ Penumbra TCT (penumbra-tct)
```

## Dependency Audit

### ✅ Legitimate Dependencies

```toml
# PRIMARY FRAMEWORK
commonware-* = { ... }          # Our base framework

# CRYPTO LIBRARIES (Not frameworks!)
sp-core = { ... }               # Just for Bandersnatch
decaf377-frost = { ... }        # Just for FROST
parity-scale-codec = { ... }    # Just for SCALE

# STORAGE
nomt = { ... }                  # Authenticated state

# STANDARD LIBS
tokio, serde, anyhow, ...       # Standard Rust ecosystem
```

### ⚠️ Potential Confusion

```toml
# DO NOT ADD THESE:
❌ frame-system = { ... }       # Would mix frameworks!
❌ frame-support = { ... }      # Would mix frameworks!
❌ pallet-revive = { ... }      # Needs Substrate runtime!
❌ sc-executor-polkavm = { ... }# Client-side only

# USE THIS INSTEAD:
✅ polkavm = { ... }            # Direct PolkaVM usage
```

## Clarified Architecture

### Final Design (Recommended)

```
Zeratul = Commonware Framework
        + Crypto Libraries (sp-core, decaf377-frost)
        + PolkaVM Direct (no Substrate runtime)
        + NOMT Storage
        + Ligerito ZK System

NOT:
  Zeratul ≠ Substrate Parachain
  Zeratul ≠ Cosmos SDK Chain
  Zeratul ≠ Hybrid Monstrosity
```

### Dependency Graph (Simplified)

```
zeratul-blockchain
  ├─ commonware-*           (PRIMARY FRAMEWORK)
  │   ├─ consensus (BFT)
  │   ├─ p2p (network)
  │   ├─ runtime (async)
  │   └─ coding (ZODA)
  │
  ├─ sp-core                (CRYPTO ONLY)
  │   └─ bandersnatch       (Ring VRF)
  │
  ├─ decaf377-frost         (CRYPTO ONLY)
  │   └─ frost              (Threshold sigs)
  │
  ├─ polkavm                (VM ONLY - no Substrate!)
  │   └─ engine             (RISC-V executor)
  │
  ├─ nomt                   (STORAGE)
  │   └─ merkle tree
  │
  └─ ligerito               (ZK SYSTEM)
      └─ binary field PCS
```

## Action Items

### Remove Confusion

1. **Update verifier pallet** to NOT use `pallet_revive`
2. **Use `polkavm` crate directly** in consensus
3. **Document** that sp-core is crypto-only, not framework
4. **Clarify** that we're a Commonware blockchain, not Substrate

### Update Documentation

1. Remove references to `pallet_revive` integration
2. Add PolkaVM direct usage documentation
3. Clarify upstream dependency roles
4. Update architecture diagrams

## Conclusion

**TL;DR**: We're NOT mixing frameworks!

- **Commonware** = Our primary framework ✅
- **sp-core** = Crypto library (Bandersnatch only) ✅
- **decaf377-frost** = Crypto library (FROST only) ✅
- **polkavm** = VM library (no Substrate needed) ✅

**The confusion came from**: Proposing `pallet_revive` (which needs Substrate runtime)

**The solution**: Use `polkavm` crate directly in Commonware consensus!

**Result**: Clean architecture, single framework, best-in-class crypto!
