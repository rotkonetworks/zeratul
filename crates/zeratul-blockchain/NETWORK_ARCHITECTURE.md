# Zeratul Network Architecture

Complete MVP implementation with decaf377-frost and migration path to golden_decaf377.

## 🎯 Design Goals

1. **Unified Cryptography**: All on decaf377 (privacy + consensus)
2. **Network Efficiency**: FROST for MVP, golden for production
3. **Native Bridges**: Threshold account on Penumbra (no IBC!)
4. **Easy Migration**: Swap DKG implementation via trait abstraction

## 📐 Architecture Overview

```
┌─────────────────────────────────────────────────────────────┐
│                    Zeratul Blockchain                        │
│                                                              │
│  ┌──────────────┐  ┌──────────────┐  ┌──────────────┐     │
│  │  Consensus   │  │  Execution   │  │   Privacy    │     │
│  │  (Safrole)   │  │  (PolkaVM)   │  │  (Ligerito)  │     │
│  └──────┬───────┘  └──────┬───────┘  └──────┬───────┘     │
│         │                  │                  │             │
│         └──────────────────┴──────────────────┘             │
│                            │                                │
│                   ┌────────▼────────┐                       │
│                   │  DKG Layer      │                       │
│                   │  (decaf377)     │                       │
│                   └────────┬────────┘                       │
│                            │                                │
│                   ┌────────▼────────┐                       │
│                   │  DKGCoordinator │                       │
│                   │  (abstraction)  │                       │
│                   └────────┬────────┘                       │
│                            │                                │
│         ┌──────────────────┴──────────────────┐            │
│         │                                      │            │
│  ┌──────▼───────┐                  ┌──────────▼─────┐     │
│  │ FrostProvider│ (MVP)            │ GoldenProvider │     │
│  │ 3 rounds     │                  │ 1 round        │     │
│  │ O(n²) msgs   │                  │ O(n) msgs      │     │
│  └──────┬───────┘                  └──────┬─────────┘     │
│         │                                 │               │
│         └─────────────┬───────────────────┘               │
│                       │                                   │
│              ┌────────▼────────┐                          │
│              │ NetworkService  │                          │
│              │ (litep2p + TCP) │                          │
│              └────────┬────────┘                          │
│                       │                                   │
└───────────────────────┼───────────────────────────────────┘
                        │
              ┌─────────▼─────────┐
              │   Validator P2P    │
              │  (4 nodes → 100+)  │
              └───────────────────┘
```

## 🔑 Cryptography Stack (All decaf377!)

### 1. Privacy Layer
```rust
use decaf377::Element;

// Ligerito PCS commitments
pub struct Commitment(Element);

// Accidental computer proofs
pub struct ACProof {
    commitments: Vec<Element>,
    // ...
}
```

### 2. Consensus Layer
```rust
use decaf377_frost as frost;

// Threshold signatures for blocks
pub type ValidatorKey = frost::keys::PublicKeyPackage;
pub type Signature = frost::Signature;
```

### 3. Bridge Layer
```rust
// Penumbra threshold account
pub struct ThresholdAccount {
    address: decaf377::Element,  // = DKG group key
    validators: Vec<ValidatorId>,
}

// No IBC needed - native Penumbra transactions!
```

## 🌐 Network Layer

### Transport (litep2p + TCP)

**Current:** TCP (universal support)
**Future:** QUIC (when litep2p adds it)

```rust
// src/network/quic.rs
pub struct NetworkService {
    litep2p: Litep2p,
    peers: HashMap<PeerId, ValidatorEndpoint>,
    // ...
}

impl NetworkService {
    pub async fn dial(&mut self, addr: SocketAddr) -> Result<()>
    pub async fn broadcast_dkg(&self, msg: DKGBroadcast) -> Result<()>
    pub async fn run(self) -> Result<()>  // Event loop
}
```

### Protocols

**DKG Broadcast:** Notification protocol
**DKG Request/Response:** Request-response protocol
**Block Sync:** (TODO) Request-response protocol

## 🔄 DKG Abstraction Layer

### Trait Design

```rust
// src/dkg/mod.rs
pub trait DKGProvider: Send + Sync {
    type Message;
    type SecretShare;
    type GroupPublicKey;

    fn start_ceremony(...) -> Result<Self::Message>;
    fn handle_message(...) -> Result<Option<Self::Message>>;
    fn is_complete(&self, epoch: EpochIndex) -> bool;
    fn threshold_sign(&self, message: &[u8]) -> Result<Vec<u8>>;
}
```

### MVP Implementation (FROST)

```rust
// src/dkg/frost_provider.rs
pub struct FrostProvider {
    ceremonies: HashMap<EpochIndex, FrostEpochState>,
}

impl DKGProvider for FrostProvider {
    // 3-round DKG
    // Round 1: Commitments
    // Round 2: Shares
    // Round 3: Verification
}
```

**Network Load (4 validators):**
- Round 1: 4 broadcasts → 16 messages
- Round 2: 4 × 4 point-to-point → 16 messages
- Round 3: 4 broadcasts → 16 messages
- **Total: ~48 messages** (negligible)

### Future Implementation (Golden)

```rust
// src/dkg/golden_provider.rs (future)
pub struct GoldenProvider {
    ceremonies: HashMap<EpochIndex, golden_decaf377::Participant>,
}

impl DKGProvider for GoldenProvider {
    // 1-round DKG with EVRF
    // Single broadcast per validator
}
```

**Network Load (100 validators):**
- FROST: 3 rounds × 100² = **30,000 messages** 😱
- Golden: 1 round × 100 = **100 messages** ✅

**When to migrate:** When validator count > 50

## 🌉 Penumbra Bridge (No IBC!)

### Architecture

```
Penumbra                          Zeratul
   │                                 │
   │  ┌─────────────────────────┐   │
   │  │  Threshold Account      │   │
   │  │  Address: GroupPubKey   │←──┤ DKG Group Key
   │  └─────────────────────────┘   │
   │                                 │
   │  Deposit                        │
   ├──────────────────────────────→  │ Validators detect
   │  User sends NOTE to addr        │ → Mint on Zeratul
   │                                 │
   │  Withdrawal                     │
   │  ←──────────────────────────────┤ User burns on Zeratul
   │  Threshold signature (2f+1)     │ → Release on Penumbra
   │                                 │
```

### Implementation

```rust
pub struct PenumbraBridge {
    frost: FrostCoordinator,
    penumbra_rpc: PenumbraClient,
    threshold_address: decaf377::Element,
}

impl PenumbraBridge {
    /// Monitor deposits (Penumbra → Zeratul)
    async fn watch_deposits(&self) -> Result<Vec<Deposit>> {
        // Read Penumbra chain for notes to threshold_address
    }

    /// Process withdrawal (Zeratul → Penumbra)
    async fn process_withdrawal(&self, burn: BurnProof) -> Result<()> {
        // 1. Validators agree on withdrawal (consensus)
        // 2. Run FROST signing (2f+1 validators)
        // 3. Submit threshold-signed tx to Penumbra
    }
}
```

### Security Model

**Trust:** Same as Zeratul consensus (2f+1 honest validators)

**vs IBC:**
| Feature | IBC Bridge | Threshold Bridge |
|---------|-----------|------------------|
| Trust assumptions | Light client + relayers | 2f+1 validators |
| Complexity | Very high | Low |
| Maintenance | Continuous | Minimal |
| Latency | High (proofs) | Low (signatures) |
| Native integration | No | Yes (same curve!) |

## 📊 Network Scalability

### Validator Count vs Network Load

| Validators | FROST Messages | Golden Messages | Improvement |
|-----------|---------------|-----------------|-------------|
| 4         | 48            | 4               | 12x         |
| 10        | 300           | 10              | 30x         |
| 50        | 7,500         | 50              | 150x        |
| 100       | 30,000        | 100             | **300x**    |

### Migration Trigger

```rust
const GOLDEN_MIGRATION_THRESHOLD: usize = 50;

let provider = if validator_count > GOLDEN_MIGRATION_THRESHOLD {
    Box::new(GoldenProvider::new()) as Box<dyn DKGProvider>
} else {
    Box::new(FrostProvider::new())
};
```

## 🚀 Roadmap

### Phase 1: MVP (Current) ✅
- [x] litep2p TCP transport
- [x] DKG abstraction layer
- [x] FROST provider (placeholder)
- [x] Network event loop
- [x] DKG message routing

### Phase 2: Testnet (Next 2 weeks)
- [ ] Complete FROST implementation
- [ ] 4-validator local testnet
- [ ] DKG ceremony testing
- [ ] Threshold signature testing

### Phase 3: Mainnet Prep (1-2 months)
- [ ] QUIC transport (when litep2p ready)
- [ ] Block sync protocol
- [ ] Penumbra bridge integration
- [ ] 10-validator testnet

### Phase 4: Production (3-6 months)
- [ ] Port golden to decaf377
- [ ] Benchmark FROST vs Golden
- [ ] Deploy golden_decaf377
- [ ] 100+ validator mainnet

## 🔧 Development

### Build
```bash
cd crates/zeratul-blockchain
cargo build --release
```

### Run Validator
```bash
# With FROST (default)
cargo run --bin validator -- --index 0 --validators 4

# With Golden (future)
cargo run --bin validator --features golden -- --index 0 --validators 4
```

### Test DKG
```bash
cargo test --package zeratul-blockchain dkg
```

## 📚 Key Files

```
crates/zeratul-blockchain/src/
├── dkg/
│   ├── mod.rs                  # DKG abstraction (trait)
│   ├── frost_provider.rs       # FROST implementation (MVP)
│   ├── GOLDEN_MIGRATION.md     # Migration guide
│   └── golden_provider.rs      # Golden implementation (future)
├── network/
│   ├── mod.rs                  # Network exports
│   ├── quic.rs                 # NetworkService (litep2p)
│   ├── types.rs                # ValidatorEndpoint, PeerId
│   ├── dkg.rs                  # DKG protocol messages
│   ├── streams.rs              # Stream protocols
│   └── crypto_compat.rs        # Crypto type wrappers
└── penumbra/
    └── bridge.rs               # Threshold account bridge (future)
```

## 🎓 Why This Design?

**1. Unified Curve**
- One curve (decaf377) for everything
- Simpler, faster, fewer dependencies
- Native Penumbra integration

**2. Progressive Optimization**
- Start simple (FROST)
- Optimize later (Golden)
- Data-driven decisions

**3. No IBC Complexity**
- Threshold account = native Penumbra
- No light clients, no relayers
- Same security as consensus

**4. Easy Migration**
- Trait abstraction = drop-in replacement
- No protocol changes needed
- Feature-flag selection

## 🔒 Security

**Cryptography:** decaf377 (Ristretto, cofactor-safe)
**Threshold:** 2f+1 Byzantine fault tolerance
**Network:** Authenticated Ed25519 transport
**Bridge:** Same trust as consensus (no additional assumptions)

---

**Status:** MVP complete, ready for FROST implementation and testnet deployment!
