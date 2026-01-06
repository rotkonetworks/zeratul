# vault nodes

vault nodes store encrypted key shares for users. they cannot access user keys without the user's PIN.

## architecture

```
vault node components:

  ┌─────────────────────────────────────────────────────────────┐
  │                      VAULT NODE                              │
  │  ┌─────────────────────────────────────────────────────────┐│
  │  │  API server (HTTP/TLS)                                  ││
  │  │  - register: store encrypted share                      ││
  │  │  - recover: return share if unlock_tag matches          ││
  │  │  - health: availability check                           ││
  │  └─────────────────────────────────────────────────────────┘│
  │                          │                                   │
  │                          ▼                                   │
  │  ┌─────────────────────────────────────────────────────────┐│
  │  │  PVM guest (polkavm sandbox)                            ││
  │  │  - cryptographic operations                              ││
  │  │  - rate limiting logic                                   ││
  │  │  - share verification                                    ││
  │  └─────────────────────────────────────────────────────────┘│
  │                          │                                   │
  │                          ▼                                   │
  │  ┌─────────────────────────────────────────────────────────┐│
  │  │  TPM (trusted platform module)                          ││
  │  │  - sealed storage                                        ││
  │  │  - attestation                                           ││
  │  └─────────────────────────────────────────────────────────┘│
  └─────────────────────────────────────────────────────────────┘
```

## API endpoints

```
POST /register
  input:
    - user_id: string (email hash)
    - unlock_tag: [u8; 16]
    - encrypted_share: [u8; 64]

  behavior:
    - verify share format
    - check user_id not already registered
    - seal to TPM
    - return signature confirming storage

  response:
    - ok: bool
    - signature: string (vault's signature)


POST /recover
  input:
    - user_id: string
    - unlock_tag: [u8; 16]

  behavior:
    - lookup user_id
    - compare unlock_tag (constant time)
    - if match: return encrypted share
    - if no match: increment failure counter
    - if failures >= {{PIN_GUESSES_FREE}}: delete share

  response:
    - ok: bool
    - encrypted_share: Option<[u8; 64]>
    - remaining_attempts: Option<u8>


GET /health
  response:
    - status: "ok" | "degraded"
    - version: string
```

## storage format

```rust
struct StoredShare {
    /// user identifier (hash of email)
    user_id: [u8; 32],
    /// unlock verification tag
    unlock_tag: [u8; 16],
    /// encrypted VSS share
    encrypted_share: [u8; 64],
    /// wrong guess counter
    failure_count: u8,
    /// creation timestamp
    created_at: u64,
    /// last access timestamp
    last_accessed: u64,
}
```

## TPM sealing

shares protected by TPM:

```
sealing process:
  1. vault generates sealing key bound to TPM
  2. share encrypted with sealing key
  3. sealing key only released to authorized code
  4. even vault operator can't extract shares

unsealing requirements:
  - correct TPM state (no tampering)
  - authorized software running
  - valid request from PVM guest
```

## rate limiting

protect against brute force:

```
per-user limits:
  - {{PIN_GUESSES_FREE}} wrong PINs (free tier)
  - {{PIN_GUESSES_PREMIUM}} wrong PINs (premium)
  - share deleted after limit reached

global limits:
  - 100 requests/minute per IP
  - 1000 requests/hour per IP
  - prevents mass enumeration
```

## deployment

recommended setup with {{VSS_TOTAL_SHARES}} nodes:

```
geographic distribution:
  vault 1: US East (primary operator)
  vault 2: EU West (partner operator)
  vault 3: Asia Pacific (third party)

independence:
  - different operators
  - different jurisdictions
  - different infrastructure providers
  - no single point of compromise
```

## trust model

```
vault operators can:
  - see encrypted share blobs
  - see unlock_tag hashes
  - see access patterns
  - delete shares (data loss, not theft)

vault operators cannot:
  - decrypt shares (need user's PIN)
  - recover signing keys
  - impersonate users
  - forge signatures

collusion resistance:
  - {{VSS_THRESHOLD}} vaults must cooperate
  - all {{VSS_THRESHOLD}} operators must collude
  - still need user's PIN
  - practically infeasible
```

## monitoring

vault operators should monitor:

```
availability metrics:
  - uptime percentage
  - response latency p50/p99
  - error rates

security metrics:
  - failed recovery attempts (potential brute force)
  - unusual access patterns
  - rate limit triggers

capacity metrics:
  - storage utilization
  - request volume trends
```

## software mode

for development/testing without TPM:

```
software mode:
  - shares stored encrypted on disk
  - encryption key in environment variable
  - NOT secure for production
  - useful for local testing

enable with:
  --mode software
  --encryption-key <32-byte-hex>
```
