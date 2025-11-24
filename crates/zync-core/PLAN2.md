# ZYNC: Zero-knowledge sYNChronization for Zcash

## Project: zidecar (ZYNC Sidecar Server)

**Target:** Zypherpunk 2025 Hackathon - Tachyon Track ($35k+)

---

## TL;DR

Replace hours of mobile wallet scanning with seconds of proof verification.
Ligerito commits to execution trace of wallet sync, client verifies in ~80ms/epoch.

```
Current:  Wallet ──scan──> 2M blocks ──decrypt──> hours
ZYNC:     Wallet ──verify──> 600 proofs ──decrypt──> seconds
```

---

## 1. Architecture Overview

```
┌─────────────────────────────────────────────────────────────────┐
│                         ZYNC SYSTEM                             │
├─────────────────────────────────────────────────────────────────┤
│                                                                 │
│  ┌─────────────┐     ┌──────────────────────────────────────┐  │
│  │ Zebra/zcashd│     │           zidecar                    │  │
│  │             │────▶│                                      │  │
│  │  RPC:8232   │     │  ┌────────────────────────────────┐  │  │
│  └─────────────┘     │  │     Block Processor            │  │  │
│                      │  │  - fetch compact blocks        │  │  │
│                      │  │  - trial decrypt per wallet    │  │  │
│                      │  └────────────┬───────────────────┘  │  │
│                      │               │                      │  │
│                      │               ▼                      │  │
│                      │  ┌────────────────────────────────┐  │  │
│                      │  │     Trace Builder              │  │  │
│                      │  │  - encode as multilinear poly  │  │  │
│                      │  │  - 2^22 coefficients/epoch     │  │  │
│                      │  └────────────┬───────────────────┘  │  │
│                      │               │                      │  │
│                      │               ▼                      │  │
│                      │  ┌────────────────────────────────┐  │  │
│                      │  │     Ligerito Prover            │  │  │
│                      │  │  - commit to trace poly        │  │  │
│                      │  │  - ~1.5s proving time          │  │  │
│                      │  │  - ~200KB proof                │  │  │
│                      │  └────────────┬───────────────────┘  │  │
│                      │               │                      │  │
│                      │               ▼                      │  │
│                      │  ┌────────────────────────────────┐  │  │
│                      │  │     gRPC Server                │  │  │
│                      │  │  - lightwalletd compat         │  │  │
│                      │  │  - ZYNC extensions             │  │  │
│                      │  └────────────────────────────────┘  │  │
│                      │               │                      │  │
│                      └───────────────┼──────────────────────┘  │
│                                      │                         │
└──────────────────────────────────────┼─────────────────────────┘
                                       │
                                       ▼
                              ┌─────────────────┐
                              │  Mobile Wallet  │
                              │                 │
                              │  - verify proof │
                              │  - ~80ms/epoch  │
                              │  - decrypt notes│
                              └─────────────────┘
```

---

## 2. How We Use Ligerito

### 2.1 The Core Insight

Ligerito proves: "I correctly computed inner product ⟨w, polynomial⟩"

We use this to prove: "I correctly processed all blocks and found all your notes"

```
Execution Trace (polynomial):
┌─────────────────────────────────────────────────────────────┐
│ coefficients[block_idx << 12 | action_idx << 3 | field]    │
├─────────────────────────────────────────────────────────────┤
│ field 0: cmx_low        (note commitment low bits)         │
│ field 1: cmx_high       (note commitment high bits)        │
│ field 2: epk_low        (ephemeral key low bits)           │
│ field 3: epk_high       (ephemeral key high bits)          │
│ field 4: decrypt_flag   (1 if decryption succeeded)        │
│ field 5: note_value     (value if decrypted, else 0)       │
│ field 6: nullifier_hit  (1 if nullifier matches our note)  │
│ field 7: state_delta    (encoded SMT update)               │
└─────────────────────────────────────────────────────────────┘

Size: 2^10 blocks × 2^9 actions × 2^3 fields = 2^22 elements
Field: BinaryElem32 (first round), BinaryElem128 (subsequent)
```

### 2.2 Constraint Verification via Sumcheck

Ligerito's sumcheck proves constraints on the trace:

```rust
// constraint: if decrypt_flag[i] = 1, note was correctly decrypted
// encoded as: Σ_i eq(r,i) * decrypt_flag[i] * (expected[i] - actual[i]) = 0

// constraint: all block hashes chain correctly  
// encoded as: Σ_i eq(r,i) * (prev_hash[i] - hash[i-1]) = 0

// constraint: state updates follow SMT rules
// encoded as: Σ_i eq(r,i) * (claimed_root[i] - computed_root[i]) = 0
```

### 2.3 Ligerito API Usage

```rust
use ligerito::{
    prove, verify,
    hardcoded_config_22, hardcoded_config_22_verifier,
    FinalizedLigeritoProof, ProverConfig, VerifierConfig,
};
use ligerito_binary_fields::{BinaryElem32, BinaryElem128};

// prover side (zidecar server)
fn generate_epoch_proof(trace: &SyncTrace) -> Result {
    let config: ProverConfig = 
        hardcoded_config_22(PhantomData, PhantomData);
    
    let ligerito_proof = prove(&config, &trace.coefficients)?;
    
    // proof contains:
    // - merkle commitments to encoded matrices
    // - sumcheck polynomials
    // - final evaluation + opening
    
    Ok(EpochProof {
        ligerito_proof,
        // ... other fields
    })
}

// verifier side (mobile wallet)
fn verify_epoch_proof(proof: &EpochProof, ivk: &Ivk) -> Result {
    let config = hardcoded_config_22_verifier();
    
    // verify ligerito commitment + sumcheck
    if !verify(&config, &proof.ligerito_proof)? {
        return Ok(false);
    }
    
    // verify constraint binding (state transition correctness)
    verify_state_binding(proof, ivk)?;
    
    Ok(true)
}
```

### 2.4 Why Ligerito Fits Perfectly

| Requirement | Ligerito Capability |
|-------------|---------------------|
| Fast mobile verification | ~50ms verify for 2^22 poly |
| Reasonable proof size | ~200KB per epoch |
| Fast server proving | ~1.5s per epoch on M1 |
| Post-quantum security | Hash-based, no pairings |
| No trusted setup | Transparent |
| Flexible constraints | Sumcheck handles arbitrary inner products |

---

## 3. Project Structure

```
zeratul/crates/
├── ligerito/                    # [EXISTING] main PCS implementation
├── ligerito-binary-fields/      # [EXISTING] GF(2^32), GF(2^128)
├── ligerito-merkle/             # [EXISTING] merkle tree
├── ligerito-reed-solomon/       # [EXISTING] RS encoding
│
└── zidecar/                     # [NEW] ZYNC sidecar server
    ├── Cargo.toml
    ├── PLAN.md                  # this file
    ├── README.md
    │
    ├── zync-core/               # core types and logic
    │   ├── Cargo.toml
    │   └── src/
    │       ├── lib.rs
    │       ├── state.rs         # WalletState, commitments
    │       ├── trace.rs         # SyncTrace polynomial encoding
    │       ├── proof.rs         # EpochProof structure
    │       ├── constraints.rs   # sumcheck constraint definitions
    │       ├── transition.rs    # state transition function
    │       └── crypto.rs        # trial decrypt, nullifier check
    │
    ├── zync-server/             # gRPC server
    │   ├── Cargo.toml
    │   └── src/
    │       ├── main.rs
    │       ├── service.rs       # gRPC handlers
    │       ├── prover.rs        # epoch proof generation
    │       ├── cache.rs         # proof caching
    │       └── zebra.rs         # zebra RPC client
    │
    ├── zync-client/             # client library + CLI
    │   ├── Cargo.toml
    │   └── src/
    │       ├── lib.rs           # client library
    │       ├── main.rs          # CLI binary
    │       ├── verifier.rs      # proof verification
    │       └── wallet.rs        # note management
    │
    └── zync-proto/              # protobuf definitions
        ├── Cargo.toml
        ├── build.rs
        └── proto/
            ├── compact_formats.proto  # lightwalletd compat
            └── zync.proto             # ZYNC extensions
```

---

## 4. Implementation Plan

### Phase 1: Core Types (Days 1-3)

```
zync-core/src/state.rs
─────────────────────
□ WalletState struct
  □ nullifier_set_root: [u8; 32]
  □ owned_notes_root: [u8; 32]  
  □ note_tree_frontier: Frontier
  □ block_height: u32
  □ block_hash: [u8; 32]

□ WalletStateCommitment
  □ commit() -> [u8; 32]
  □ genesis() -> WalletState

□ SparseMerkleTree operations
  □ insert(root, key, value) -> new_root
  □ verify_membership(root, key, value, proof) -> bool
```

```
zync-core/src/trace.rs
──────────────────────
□ SyncTrace struct
  □ coefficients: Vec<BinaryElem32>
  □ epoch: u32
  □ block_count: usize

□ TraceField enum (8 variants)
  □ CmxLow, CmxHigh, EpkLow, EpkHigh
  □ DecryptFlag, NoteValue, NullifierHit, StateDelta

□ SyncTrace::from_blocks()
  □ iterate blocks
  □ iterate actions
  □ encode fields
  □ pad to 2^22

□ SyncTrace::get_field(block, action, field) -> BinaryElem32
```

```
zync-core/src/proof.rs
──────────────────────
□ EpochProof struct
  □ epoch_number: u32
  □ prev_epoch_hash: [u8; 32]
  □ state_before: WalletStateCommitment
  □ state_after: WalletStateCommitment
  □ ligerito_proof: FinalizedLigeritoProof
  □ discovered_notes_enc: Vec<EncryptedNote>
  □ spent_nullifiers_enc: Vec<EncryptedNullifier>
  □ ivk_commitment: [u8; 32]

□ EpochProof::hash() -> [u8; 32]
□ EpochProof::serialized_size() -> usize
□ Serialization (serde + custom binary)
```

```
zync-core/src/constraints.rs
────────────────────────────
□ verify_state_binding()
  □ check ligerito proof binds to claimed states
  □ verify auxiliary sumcheck data

□ Constraint definitions (for documentation)
  □ block_chain_constraint
  □ decryption_constraint  
  □ completeness_constraint
  □ state_update_constraint
```

### Phase 2: Server (Days 4-7)

```
zync-proto/proto/zync.proto
───────────────────────────
□ Import compact_formats.proto (lightwalletd)
□ ZyncService definition
  □ RegisterWallet(RegisterRequest) -> RegisterResponse
  □ GetSyncProof(SyncRequest) -> SyncResponse
  □ GetEpochInfo(EpochInfoRequest) -> EpochInfoResponse

□ Message types
  □ RegisterRequest, RegisterResponse
  □ SyncRequest, SyncResponse
  □ EpochProof (proto version)
  □ WalletState (proto version)
  □ EncryptedNote, EncryptedNullifier
```

```
zync-server/src/zebra.rs
────────────────────────
□ ZebraClient struct
  □ connect(endpoint) -> Self
  □ get_block(height) -> CompactBlock
  □ get_block_range(start..end) -> Vec<CompactBlock>
  □ get_tip_height() -> u32

□ CompactBlock parsing
  □ extract orchard actions
  □ extract nullifiers
```

```
zync-server/src/prover.rs
─────────────────────────
□ EpochProver struct
  □ zebra: ZebraClient
  □ ligerito_config: ProverConfig

□ generate_epoch_proof(wallet, epoch) -> EpochProof
  □ fetch blocks from zebra
  □ build SyncTrace
  □ call ligerito::prove()
  □ compute state transition
  □ encrypt discovered notes
  □ assemble EpochProof
```

```
zync-server/src/service.rs
──────────────────────────
□ ZyncServer struct
  □ zebra: ZebraClient
  □ wallets: DashMap<WalletId, WalletEntry>
  □ cache: ProofCache

□ impl ZyncService for ZyncServer
  □ register_wallet()
  □ get_sync_proof()
  □ get_epoch_info()

□ lightwalletd compatibility RPCs
  □ get_latest_block()
  □ get_block()
  □ get_block_range()
```

```
zync-server/src/cache.rs
────────────────────────
□ ProofCache struct
  □ LRU cache keyed by (wallet_id, epoch)
  □ max_size configuration
  □ get(), insert(), evict()
```

### Phase 3: Client (Days 8-11)

```
zync-client/src/lib.rs
──────────────────────
□ ZyncClient struct
  □ connect(server) -> Self
  □ register(ivk) -> (WalletId, WalletState)
  □ sync(from_epoch) -> SyncResult

□ SyncResult struct
  □ epoch_proofs: Vec<EpochProof>
  □ final_state: WalletState
  □ discovered_notes: Vec<Note>
  □ spent_nullifiers: Vec<Nullifier>
```

```
zync-client/src/verifier.rs
───────────────────────────
□ verify_epoch_proof(proof, ivk, prev_hash) -> Result<()>
  □ check epoch chain linkage
  □ check ivk commitment
  □ call ligerito::verify()
  □ verify state binding

□ verify_sync_response(response, ivk) -> Result<WalletState>
  □ verify all epoch proofs in sequence
  □ check state continuity
```

```
zync-client/src/wallet.rs
─────────────────────────
□ LocalWallet struct
  □ ivk: IncomingViewingKey
  □ state: WalletState
  □ notes: BTreeMap<Commitment, OwnedNote>
  □ epoch_tip: [u8; 32]

□ apply_sync(response) -> SyncSummary
  □ verify proofs
  □ decrypt notes
  □ update state
  □ return summary
```

```
zync-client/src/main.rs (CLI)
─────────────────────────────
□ Commands
  □ sync --server <url> --viewing-key <key>
  □ verify --proof-file <path> --viewing-key <key>
  □ bench --server <url> --epochs <n>
  □ info --server <url>

□ Output formatting
  □ progress bars
  □ sync summary
  □ benchmark results
```

### Phase 4: Integration & Demo (Days 12-14)

```
Integration Tests
─────────────────
□ tests/integration.rs
  □ full round-trip: register -> sync -> verify
  □ multi-epoch sync
  □ proof size verification
  □ timing benchmarks

□ tests/compatibility.rs
  □ lightwalletd gRPC compatibility
  □ verify existing wallets can connect
```

```
Demo Script
───────────
□ demo/run.sh
  □ start local zebra (testnet)
  □ start zidecar
  □ run sync benchmark
  □ compare with lightwalletd baseline

□ demo/README.md
  □ setup instructions
  □ expected output
  □ troubleshooting
```

```
Documentation
─────────────
□ README.md (project root)
  □ what is ZYNC
  □ how it works
  □ quick start

□ docs/ARCHITECTURE.md
  □ detailed design
  □ security analysis
  □ trust model

□ docs/BENCHMARKS.md
  □ performance numbers
  □ comparison charts
  □ methodology
```

---

## 5. Crate Dependencies

```toml
# zync-core/Cargo.toml
[dependencies]
ligerito = { path = "../../ligerito" }
ligerito-binary-fields = { path = "../../ligerito-binary-fields" }
ligerito-merkle = { path = "../../ligerito-merkle" }

blake2 = "0.10"
sha2 = "0.10"
serde = { version = "1.0", features = ["derive"] }
thiserror = "1.0"
bytemuck = { version = "1.14", features = ["derive"] }

# zcash crypto (viewing keys, trial decrypt)
orchard = "0.10"
zcash_primitives = "0.19"
zcash_note_encryption = "0.5"
```

```toml
# zync-server/Cargo.toml
[dependencies]
zync-core = { path = "../zync-core" }
zync-proto = { path = "../zync-proto" }

tokio = { version = "1", features = ["full"] }
tonic = "0.12"
dashmap = "6.0"
lru = "0.12"
tracing = "0.1"
tracing-subscriber = "0.3"

# zebra RPC client
jsonrpsee = { version = "0.24", features = ["http-client"] }
```

```toml
# zync-client/Cargo.toml
[dependencies]
zync-core = { path = "../zync-core" }
zync-proto = { path = "../zync-proto" }
ligerito = { path = "../../ligerito", default-features = false, features = ["verifier-only"] }

tokio = { version = "1", features = ["rt-multi-thread"] }
tonic = "0.12"
clap = { version = "4", features = ["derive"] }
indicatif = "0.17"  # progress bars
```

---

## 6. Config Constants

```rust
// zync-core/src/lib.rs

/// blocks per epoch (~2.5 days at 75s/block)
pub const EPOCH_SIZE: u32 = 1024;

/// max orchard actions per block (protocol limit)
pub const MAX_ACTIONS_PER_BLOCK: usize = 512;

/// fields encoded per action
pub const FIELDS_PER_ACTION: usize = 8;

/// polynomial size: 2^10 * 2^9 * 2^3 = 2^22
pub const TRACE_LOG_SIZE: usize = 22;
pub const TRACE_SIZE: usize = 1 << TRACE_LOG_SIZE;

/// ligerito config for trace polynomial
/// 
/// existing configs: 12, 16, 20, 24, 28, 30
/// for 2^22 trace, use config_24 (pads to 2^24, ~4x overhead but works)
/// or add hardcoded_config_22 to ligerito/src/configs.rs
pub fn prover_config() -> ProverConfig {
    // option A: use existing config_24 (2^24 > 2^22, works with padding)
    hardcoded_config_24(PhantomData, PhantomData)
    
    // option B: add config_22 to ligerito for optimal performance
    // hardcoded_config_22(PhantomData, PhantomData)
}

pub fn verifier_config() -> VerifierConfig {
    hardcoded_config_24_verifier()
    // or: hardcoded_config_22_verifier()
}

/// security parameter (bits)
pub const SECURITY_BITS: usize = 100;

/// orchard activation height (mainnet)
pub const ORCHARD_ACTIVATION_HEIGHT: u32 = 1_687_104;
```

---

## 7. Testing Strategy

### Unit Tests
```rust
// state.rs
#[test] fn test_wallet_state_commitment() { }
#[test] fn test_smt_insert_verify() { }
#[test] fn test_genesis_state() { }

// trace.rs
#[test] fn test_trace_encoding_roundtrip() { }
#[test] fn test_trace_field_indexing() { }
#[test] fn test_trace_from_empty_blocks() { }

// proof.rs
#[test] fn test_epoch_proof_serialization() { }
#[test] fn test_epoch_proof_hash_determinism() { }
```

### Integration Tests
```rust
// tests/prove_verify.rs
#[test] fn test_prove_verify_single_epoch() { }
#[test] fn test_prove_verify_with_notes() { }
#[test] fn test_invalid_proof_rejected() { }

// tests/sync_flow.rs
#[tokio::test] async fn test_full_sync_flow() { }
#[tokio::test] async fn test_incremental_sync() { }
```

### Benchmarks
```rust
// benches/proving.rs
fn bench_trace_building(c: &mut Criterion) { }
fn bench_ligerito_prove(c: &mut Criterion) { }
fn bench_epoch_proof_generation(c: &mut Criterion) { }

// benches/verification.rs
fn bench_ligerito_verify(c: &mut Criterion) { }
fn bench_epoch_proof_verify(c: &mut Criterion) { }
fn bench_full_sync_verify(c: &mut Criterion) { }
```

---

## 8. Milestones & Deliverables

### Milestone 1: Core Types (Day 3)
- [ ] WalletState + commitment working
- [ ] SyncTrace encoding working
- [ ] EpochProof structure defined
- [ ] Unit tests passing

### Milestone 2: Prover Integration (Day 7)
- [ ] Ligerito proves SyncTrace
- [ ] Full EpochProof generation
- [ ] Proof verification working
- [ ] Server skeleton running

### Milestone 3: gRPC Server (Day 11)
- [ ] Full gRPC service implemented
- [ ] Zebra integration working
- [ ] Client library working
- [ ] CLI tool functional

### Milestone 4: Demo Ready (Day 14)
- [ ] End-to-end demo working
- [ ] Benchmarks documented
- [ ] README complete
- [ ] Video/presentation ready

---

## 9. Success Metrics

| Metric | Target | Stretch |
|--------|--------|---------|
| Epoch proving time | <3s | <1.5s |
| Epoch verify time | <100ms | <50ms |
| Proof size | <500KB | <300KB |
| Full chain sync | <60s | <30s |
| Memory (prover) | <8GB | <4GB |
| Memory (verifier) | <100MB | <50MB |

---

## 10. Risk Mitigation

| Risk | Mitigation |
|------|------------|
| Ligerito 2^22 config missing | Add hardcoded_config_22 to ligerito |
| Zcash crate version conflicts | Pin specific versions, test early |
| Zebra RPC changes | Abstract behind trait, mock for testing |
| Proof too large | Tune epoch size, compression |
| Proving too slow | Profile, parallel optimization |
| Time pressure | MVP first, polish later |

---

## 11. Open Questions

1. **Exact constraint formulation**: Need to finalize how we encode trial decrypt verification in sumcheck. Options:
   - Commit to ivk, verify decryption algebraically
   - Hash-based: include decrypt inputs in trace, verify hash chain

2. **SMT implementation**: Use existing crate or custom?
   - Candidate: `sparse-merkle-tree` crate
   - May need custom for BinaryElem32 compatibility

3. **Epoch boundaries**: Align with Zcash epochs or arbitrary 1024-block chunks?
   - Arbitrary simpler for MVP
   - Protocol epochs useful for caching

4. **Viewing key encryption**: How to protect ivk in transit to server?
   - Server pubkey + ECIES
   - Or: client sends ivk_commitment, server generates proof, client fills in ivk
   - For MVP: assume secure channel (TLS)

---

## 12. References

- [Ligerito Paper](https://angeris.github.io/papers/ligerito.pdf)
- [Tachyon RFP](https://forum.zcashcommunity.com/t/zcash-project-tachyon-request-for-proposals/49659)
- [lightwalletd Protocol](https://github.com/zcash/lightwalletd/blob/master/docs/rtd/index.rst)
- [Orchard Protocol](https://zcash.github.io/orchard/)
- [Zebra RPC Docs](https://doc.zebra.zfnd.org/)

---

## Next Steps

```bash
# 1. create workspace structure
cd zeratul/crates/zidecar
mkdir -p zync-core/src zync-server/src zync-client/src zync-proto/proto

# 2. initialize crates
cargo init zync-core --lib
cargo init zync-server
cargo init zync-client
cargo init zync-proto --lib

# 3. add hardcoded_config_22 to ligerito if missing
# check: grep -r "hardcoded_config_22" ../ligerito/

# 4. start with zync-core/src/state.rs
```
