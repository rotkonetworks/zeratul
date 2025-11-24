# Light Client Authentication Strategy

## Problem

**JAMNP-S authentication doesn't work for browser light clients:**
- JAMNP-S uses mutual TLS with Ed25519 certificates
- Browsers can't present custom client certificates in WebTransport/WebSocket
- Browsers can't verify custom server certificates (must use standard CA chain)

## Solution: Dual Authentication

### Validator ↔ Validator (JAMNP-S)
✅ **Mutual TLS with Ed25519 certificates**
- Both peers present self-signed Ed25519 certs
- Certificate alternative name = base32(Ed25519 pubkey)
- TLS 1.3 handshake signs transcript hash
- Byzantine fault tolerant (validators are known)

```
Validator A                    Validator B
    │                               │
    │──── TLS Handshake ────────────►│
    │  (present Ed25519 cert)        │
    │◄──── TLS Handshake ────────────│
    │  (verify Ed25519 cert)         │
    │                               │
    │◄──── Encrypted QUIC ──────────►│
```

### Validator ↔ Light Client (WebTransport/WebSocket)
✅ **Application-layer signatures**
- Server uses standard HTTPS certificate (Let's Encrypt, etc.)
- No client certificate (browser limitation)
- Light client authenticates via signed requests
- Validator doesn't need to authenticate to light client (light client verifies proofs)

```
Light Client (Browser)         Validator
    │                               │
    │──── HTTPS/WebTransport ───────►│
    │  (standard server cert)        │
    │                               │
    │──── Signed Request ───────────►│
    │  Sign(query, client_key)       │
    │◄──── Response + Proof ─────────│
    │  (verify proof locally)        │
```

## Authentication Models

### Validator Network (BFT)
**Trust Model**: Known validator set with Byzantine fault tolerance
- Validators are on-chain (published Ed25519 keys)
- 2f+1 threshold for consensus
- Malicious validators are slashed

**Authentication**:
- Mutual TLS (both sides verify)
- Connection-level authentication
- Prevents impersonation attacks

### Light Client Network (Trust-Minimized)
**Trust Model**: Cryptographic proof verification
- Light clients don't trust validators
- Verify Ligerito proofs locally (512μs)
- Connect to multiple validators for redundancy

**Authentication**:
- Optional: Light client signs queries (prevents DoS, enables rate limiting)
- Not required for security (proofs are verifiable)
- Server uses standard TLS (validators already have domain names)

## Implementation

### Validator TLS Config
```rust
// Self-signed Ed25519 certificate
let cert = generate_ed25519_cert(validator_keypair);
let alt_name = base32_encode(validator_pubkey); // "eabcd..."

// Mutual TLS
let tls_config = TlsConfig::new()
    .with_client_cert_required()
    .with_custom_verifier(verify_ed25519_cert);

// ALPN
let alpn = format!("jamnp-s/0/{}", genesis_hash);
```

### Light Client HTTP/WebTransport
```rust
// Standard HTTPS (Let's Encrypt)
let cert = load_lets_encrypt_cert();

// No client cert required
let server_config = ServerConfig::new(cert);

// Optional: Verify signed queries
fn handle_query(req: SignedRequest) -> Response {
    if !verify_signature(&req) {
        return RateLimited; // DoS protection
    }

    let proof = generate_proof(&req.query);
    Response { data, proof }
}
```

### Light Client (Browser)
```typescript
// WebTransport (HTTP/3 over QUIC)
const transport = new WebTransport("https://validator.example.com:443");

// Sign query (optional, for rate limiting)
const query = { block_hash: "0xabc..." };
const signature = sign(query, client_private_key);

await transport.datagrams.writable.getWriter().write({
    query,
    signature, // Optional
});

const response = await transport.datagrams.readable.getReader().read();

// Verify proof locally (trustless)
const valid = verify_ligerito_proof(response.proof, response.data);
```

## Security Properties

### Validator Network
✅ **Prevents impersonation**: Mutual TLS with known keys
✅ **Prevents MITM**: TLS 1.3 encryption
✅ **Byzantine tolerance**: 2f+1 threshold consensus
✅ **Slashing**: Malicious validators lose stake

### Light Client Access
✅ **Trust-minimized**: Verify proofs locally (no trust in validators)
✅ **Censorship resistant**: Connect to multiple validators
✅ **DoS protected**: Optional signed requests for rate limiting
✅ **Privacy preserving**: No identity required (proofs are public)

## Comparison to Other Chains

### Ethereum
- **Validators**: LibP2P with noise protocol (not mutual TLS)
- **Light clients**: JSON-RPC over HTTPS (no signatures)
- **Auth**: Trust-based (no proof verification for most queries)

### Polkadot
- **Validators**: LibP2P with noise protocol
- **Light clients**: JSON-RPC + Substrate Connect (WASM light client in browser)
- **Auth**: Proofs for finality, trust for state

### Cosmos (Tendermint)
- **Validators**: P2P with ed25519 node keys (custom protocol)
- **Light clients**: gRPC/REST (trust-based) or light client protocol (proof-based)
- **Auth**: IBC light client verification

### Zeratul (JAM-inspired)
- **Validators**: QUIC + TLS 1.3 + Ed25519 certs (JAMNP-S)
- **Light clients**: WebTransport/WebSocket with Ligerito proofs
- **Auth**: Mutual TLS for validators, proof verification for light clients
- **Advantage**: Fastest proof verification (512μs Ligerito)

## Why This Works

1. **Validators need mutual auth**: They participate in consensus (BFT)
2. **Light clients need proofs**: They don't participate in consensus
3. **Browser limitations**: Can't do custom TLS, so use application-layer signatures
4. **Best of both worlds**:
   - Validators: Strong authentication + low latency (QUIC)
   - Light clients: Universal access + trustless (proof verification)

## Open Questions

1. **Should light client queries be signed?**
   - Pro: DoS protection, rate limiting, analytics
   - Con: Requires key management for users
   - Decision: Make it optional (unsigned for anonymous queries, signed for higher limits)

2. **Should we expose both WebTransport AND WebSocket?**
   - Pro: WebSocket is universal (older browsers)
   - Pro: WebTransport is faster (QUIC multiplexing)
   - Decision: Support both, let client choose

3. **Should validators charge for light client queries?**
   - Pro: Covers bandwidth/computation costs
   - Con: Reduces accessibility
   - Decision: Free for basic queries, paid for high-volume APIs
