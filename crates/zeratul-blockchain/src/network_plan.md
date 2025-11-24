# Zeratul Network Architecture

## Design Principles

1. **Validator Network**: QUIC (JAMNP-S) for low-latency P2P
2. **Light Client Access**: HTTP/WebSocket/WebTransport for browser compatibility
3. **Separation of Concerns**: Validators run full protocol, light clients just query/verify

## Architecture

```
┌─────────────────────────────────────────────────────────────┐
│                     Validator Network                        │
│                    (QUIC - JAMNP-S)                         │
│                                                              │
│  ┌──────────┐   QUIC    ┌──────────┐   QUIC    ┌──────────┐│
│  │Validator │◄─────────►│Validator │◄─────────►│Validator ││
│  │    0     │  Streams  │    1     │  Streams  │    2     ││
│  └────┬─────┘           └────┬─────┘           └────┬─────┘│
│       │                      │                      │       │
└───────┼──────────────────────┼──────────────────────┼───────┘
        │                      │                      │
        │ WebSocket/           │ WebSocket/           │ WebSocket/
        │ WebTransport         │ WebTransport         │ WebTransport
        │                      │                      │
        ▼                      ▼                      ▼
   ┌─────────┐           ┌─────────┐           ┌─────────┐
   │ Light   │           │ Light   │           │ Light   │
   │ Client  │           │ Client  │           │ Client  │
   │(Browser)│           │(Browser)│           │(Browser)│
   └─────────┘           └─────────┘           └─────────┘
```

## Validator Network (QUIC)

### Transport
- **Protocol**: QUIC (UDP-based)
- **Encryption**: TLS 1.3 with Ed25519 certificates
- **Authentication**: Mutual TLS with validator keys
- **ALPN**: `jamnp-s/0/{genesis_hash}` or `jamnp-s/0/{genesis_hash}/builder`

### Streams (as per JAM spec)
**UP (Unique Persistent):**
- UP 0: Block announcements (neighbor gossip)

**CE (Common Ephemeral):**
- CE 128: Block request
- CE 129: State request
- CE 131/132: Safrole ticket distribution
- CE 133: Work-package submission (builder → guarantor)
- CE 134: Work-package sharing (guarantor ↔ guarantor)
- CE 135: Work-report distribution
- CE 136: Work-report request (auditor)
- CE 137: Shard distribution (assurer → guarantor)
- CE 138: Audit shard request
- CE 139/140: Segment shard request
- CE 141: Assurance distribution
- CE 142: Preimage announcement
- CE 143: Preimage request
- CE 144: Audit announcement
- CE 145: Judgment publication
- CE 146: Work-package bundle submission
- CE 147: Bundle request
- CE 148: Segment request

### DKG Protocol (Custom - not in JAM spec)
We need to add our own stream kind for Golden DKG:
- **CE 200**: DKG broadcast (epoch-based threshold key generation)
- **CE 201**: DKG request (missing broadcasts)
- **CE 202**: DKG complete announcement

## Light Client Access

### Why Not P2P for Light Clients?
❌ **QUIC P2P**: Browsers can't do raw QUIC P2P (no UDP socket API)
❌ **WebRTC P2P**: Requires signaling server, complex NAT traversal, high latency
✅ **Client-Server**: Light clients query validators via HTTP/WS/WebTransport

### Transport Options

#### 1. WebSocket (MVP - Start Here)
**Pros:**
- Universal browser support
- Simple implementation
- Works everywhere (including mobile)
- Can reuse HTTP port (upgrade from HTTP)

**Cons:**
- TCP-based (higher latency than QUIC)
- No built-in multiplexing (need to add)

#### 2. WebTransport (Future)
**Pros:**
- HTTP/3 over QUIC (low latency)
- Built-in multiplexing
- Modern protocol

**Cons:**
- Limited browser support (Chrome only as of 2025)
- Requires server cert infrastructure
- Not available in all networks

#### 3. HTTP/2 or HTTP/3 (Simple queries)
**Pros:**
- REST-style API
- Easy to use from any language
- Cacheable

**Cons:**
- Request-response only (no subscriptions)
- Higher latency for streaming

### Light Client API

Light clients need:

```rust
// Block sync
GET /block/{hash}                    // Get block by hash
GET /block/latest                    // Get latest finalized block
GET /block/range/{from}/{to}         // Get block range
GET /header/{hash}                   // Get header only (lighter)

// State queries
GET /state/{block_hash}/{key}        // Get state value
POST /state/proof                    // Get Merkle proof for key
  Body: { block_hash, keys: [...] }

// Proof verification
GET /proof/ligerito/{hash}           // Get Ligerito succinct proof
POST /proof/verify                   // Verify proof (client-side)
  Body: { proof, commitment, ... }

// Subscriptions (WebSocket only)
WS /subscribe/blocks                 // Subscribe to new finalized blocks
WS /subscribe/headers                // Subscribe to new headers only
WS /subscribe/state/{key}            // Subscribe to state changes

// Transaction submission
POST /tx/submit                      // Submit transaction
  Body: { tx: AccidentalComputerProof }
```

### Security Model

**Validators:**
- Mutually authenticated (TLS + Ed25519 validator keys)
- Byzantine fault tolerant (2f+1 threshold)
- Full state validation

**Light Clients:**
- Trust-minimized (verify proofs locally)
- Connect to multiple validators for redundancy
- Don't need validator authentication (just verify cryptographic proofs)
- Can detect malicious validators via proof verification

## Implementation Plan

### Phase 1: Validator Network (MVP)
1. ✅ QUIC transport with litep2p
2. ✅ TLS 1.3 with Ed25519 certificates
3. ✅ UP 0: Block announcements
4. ✅ CE 128/129: Block/state requests
5. ✅ CE 200: DKG broadcast (custom)

### Phase 2: Light Client Support
6. ⬜ HTTP/WebSocket endpoint on validators
7. ⬜ REST API for block/state queries
8. ⬜ WebSocket subscriptions for real-time updates
9. ⬜ Ligerito proof endpoints
10. ⬜ Browser light client library (WASM)

### Phase 3: Advanced Features
11. ⬜ WebTransport support (HTTP/3)
12. ⬜ GraphQL API (optional)
13. ⬜ gRPC endpoint (optional)
14. ⬜ Mobile SDK (Swift/Kotlin)

## Code Structure

```
crates/
├── zeratul-blockchain/
│   ├── src/
│   │   ├── network/
│   │   │   ├── quic.rs           # QUIC transport (JAMNP-S)
│   │   │   ├── streams.rs        # Stream protocols (UP/CE)
│   │   │   ├── dkg.rs            # DKG over QUIC (CE 200-202)
│   │   │   └── mod.rs
│   │   ├── rpc/
│   │   │   ├── http.rs           # HTTP REST API
│   │   │   ├── websocket.rs      # WebSocket subscriptions
│   │   │   ├── webtransport.rs   # WebTransport (future)
│   │   │   └── mod.rs
│   │   └── bin/
│   │       └── validator.rs      # Runs both QUIC + HTTP/WS
├── zeratul-light-client/         # New crate
│   ├── src/
│   │   ├── client.rs             # Light client API
│   │   ├── proof_verifier.rs    # Verify Ligerito proofs
│   │   └── lib.rs
│   └── Cargo.toml
└── zeratul-wasm/                 # New crate (browser)
    ├── src/
    │   └── lib.rs                # WASM bindings
    └── Cargo.toml
```

## Comparison to Other Chains

### Ethereum
- **Validators**: LibP2P (similar to our QUIC)
- **Light Clients**: JSON-RPC over HTTP/WebSocket
- **Approach**: Separate networking layers ✅

### Polkadot
- **Validators**: LibP2P
- **Light Clients**: JSON-RPC + Substrate Connect (WASM in-browser light client)
- **Approach**: Separate layers + advanced browser integration ✅

### Cosmos
- **Validators**: Tendermint P2P
- **Light Clients**: REST API + gRPC + WebSocket
- **Approach**: Separate layers with multiple APIs ✅

### Our Approach (Zeratul)
- **Validators**: QUIC (JAMNP-S spec compliant)
- **Light Clients**: HTTP/WebSocket (trust-minimized via Ligerito proofs)
- **Innovation**: Fastest proof verification (512μs Ligerito) ✅

## Why This Works

1. **Validators need low latency**: QUIC provides 10-50ms P2P with multiplexing
2. **Light clients need accessibility**: HTTP/WS work everywhere (browsers, mobile, IoT)
3. **Security model differs**:
   - Validators: Mutually authenticated BFT consensus
   - Light clients: Cryptographic proof verification (trustless)
4. **JAM compatibility**: QUIC network follows JAM JAMNP-S spec exactly
5. **Progressive enhancement**: Start with WebSocket, add WebTransport when browsers catch up

## Decision: Dual Transport

✅ **For MVP: Implement both**
- QUIC for validator-to-validator (start here)
- HTTP/WebSocket for light clients (add later)

This is industry standard and gives us:
- Fast validator network
- Universal light client support
- Clear separation of concerns
- JAM spec compliance
