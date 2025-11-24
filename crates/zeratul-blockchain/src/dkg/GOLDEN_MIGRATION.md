# Migration to golden_decaf377

This document outlines the plan to port golden-rs from BLS12-381 to decaf377.

## Why golden_decaf377?

**Network Efficiency:**
- FROST: 3 rounds, O(n²) messages (30k for 100 validators)
- Golden: 1 round, O(n) messages (100 for 100 validators)
- **300x reduction** in network traffic for large validator sets

**Same Security:**
- Both use threshold Schnorr signatures
- Both achieve 2f+1 Byzantine fault tolerance
- Golden adds EVRF for randomness

## Porting Strategy

### Phase 1: Understand golden-rs internals

**Key components to port:**
1. `EVRF` - Efficient Verifiable Random Function
2. `Participant` - Validator state machine
3. `BroadcastMsg` - Single-round DKG message
4. `KeyGenOutput` - Secret shares + group key

**Current (BLS12-381):**
```rust
// golden-rs/src/lib.rs
use bls12_381::{G1Projective, G2Projective, Scalar};

pub struct EVRF {
    beta: G2Projective,
    proof: G1Projective,
}

pub struct Participant {
    secret: Scalar,
    public: G1Projective,
}
```

**Target (decaf377):**
```rust
// golden_decaf377/src/lib.rs
use decaf377::{Element, Fr};

pub struct EVRF {
    beta: Element,
    proof: Element,
}

pub struct Participant {
    secret: Fr,
    public: Element,
}
```

### Phase 2: Curve operations mapping

| BLS12-381 Operation | decaf377 Equivalent |
|---------------------|---------------------|
| `G1Projective` | `decaf377::Element` |
| `G2Projective` | `decaf377::Element` (no G2!) |
| `Scalar` | `decaf377::Fr` |
| `pairing()` | ❌ Not needed (Schnorr, not BLS) |
| `hash_to_curve()` | `Element::map_to_group()` |

**Key difference:** No pairings! decaf377 is simpler.

### Phase 3: Implement golden_decaf377

**Directory structure:**
```
crates/golden_decaf377/
├── Cargo.toml
├── src/
│   ├── lib.rs           # Public API
│   ├── evrf.rs          # EVRF implementation
│   ├── participant.rs   # Participant state
│   ├── keygen.rs        # Single-round DKG
│   └── sign.rs          # Threshold signing
└── tests/
    └── integration.rs   # 4-validator test
```

**Cargo.toml:**
```toml
[package]
name = "golden_decaf377"
version = "0.1.0"
edition = "2021"

[dependencies]
decaf377 = "0.10"
rand_core = "0.6"
sha3 = "0.10"
serde = { version = "1.0", features = ["derive"] }
```

### Phase 4: Integrate into Zeratul

**Create `GoldenProvider`:**
```rust
// crates/zeratul-blockchain/src/dkg/golden_provider.rs
use golden_decaf377 as golden;

pub struct GoldenProvider {
    ceremonies: HashMap<EpochIndex, golden::Participant>,
}

impl DKGProvider for GoldenProvider {
    type Message = golden::BroadcastMsg;
    type SecretShare = golden::SecretShare;
    type GroupPublicKey = golden::GroupPublicKey;

    fn start_ceremony(...) -> Result<Self::Message> {
        // Single broadcast message!
        let participant = golden::Participant::new(...);
        let msg = participant.generate_broadcast()?;
        Ok(msg)
    }

    fn handle_message(...) -> Result<Option<Self::Message>> {
        // Process single broadcast
        // Return None (no more rounds!)
        participant.process_broadcast(from, message)?;
        if participant.is_complete() {
            // DKG done in 1 round!
        }
        Ok(None)
    }
}
```

**Feature flag in Cargo.toml:**
```toml
[features]
default = ["frost"]
frost = []
golden = ["golden_decaf377"]

[dependencies]
decaf377-frost = "0.9"
golden_decaf377 = { path = "../golden_decaf377", optional = true }
```

**Runtime selection:**
```rust
// main.rs
#[cfg(feature = "frost")]
let provider = FrostProvider::new();

#[cfg(feature = "golden")]
let provider = GoldenProvider::new();

let coordinator = DKGCoordinator::new(provider);
```

### Phase 5: Benchmarking

**Test network load:**
```bash
# FROST (baseline)
cargo run --release --features frost -- --validators 100

# Golden (optimized)
cargo run --release --features golden -- --validators 100

# Compare:
# - Message count
# - Bandwidth
# - Latency
# - CPU usage
```

## Timeline

**MVP (Current):**
- ✅ Use decaf377-frost
- ✅ 4-validator testnet
- Network load: negligible (~50 messages)

**Production (3-6 months):**
- Port golden to decaf377
- Benchmark at scale (100+ validators)
- Deploy golden_decaf377 if network becomes bottleneck

**Long-term (Upstream):**
- Contribute golden_decaf377 back to golden-rs
- Or maintain as separate crate for Ristretto-based chains

## Notes

**Why not port now?**
- FROST works fine for 4-validator MVP
- Focus on getting blockchain running first
- Port golden when we have real network data

**Why port at all?**
- 100+ validators = golden is **required**
- FROST won't scale past ~50 validators (network congestion)
- golden's 1-round design is fundamentally better

**Maintenance:**
- golden-rs is actively maintained by rotkonetworks
- Porting to decaf377 is straightforward (simpler curve!)
- Can track upstream golden-rs for improvements
