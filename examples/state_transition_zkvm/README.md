# State Transition with Ligerito + NOMT

A verifiable computation system using **Ligerito PCS** for zero-knowledge proofs and **NOMT** for authenticated state storage. All heavy computation happens on the client edge - the server just verifies proofs and updates state.

## Architecture

```
┌─────────────────────────────────────────┐
│  Kubernetes Cluster (HA)                │
│                                         │
│  ┌──────────────────────────────────┐  │
│  │ Deployment: verifier-service     │  │
│  │ - Replicas: 3                    │  │
│  │ - Stateless HTTP servers         │  │
│  │ - Verify Binius proofs           │  │
│  │ - Auto-scales 3-20 pods          │  │
│  └──────────────┬───────────────────┘  │
│                 ↓                       │
│  ┌──────────────────────────────────┐  │
│  │ StatefulSet: nomt-storage        │  │
│  │ - Single replica                 │  │
│  │ - Persistent volume (100Gi)      │  │
│  │ - NOMT authenticated state       │  │
│  └──────────────────────────────────┘  │
└─────────────────────────────────────────┘
                 ↑
                 │ HTTP/gRPC
                 │
      ┌──────────┴──────────┐
      │  Clients            │
      │  - Generate proofs  │
      │  - Submit via HTTP  │
      └─────────────────────┘
```

## Key Features

1. **Commitment-Based State**: Hash commitments hide balances while allowing proof verification
2. **Client-Side Proving**: Users generate proofs locally (expensive, ~seconds)
3. **Server-Side Verification**: Stateless verifiers only verify (fast, ~milliseconds)
4. **Transparent Privacy**: Commitments hide balances, but encrypted backup allows compliance/auditing
5. **NOMT Storage**: Efficient authenticated state with compact witnesses
6. **High Availability**: Kubernetes deployment with horizontal pod autoscaling (3-20 replicas)

## Project Structure

```
examples/state_transition_zkvm/
├── ARCHITECTURE.md              # Detailed design documentation
├── README.md                    # This file
├── circuit/                     # Binius circuit implementation
│   ├── Cargo.toml
│   └── src/
│       └── lib.rs              # Transfer circuit logic
├── client/                      # Proof generator (TODO)
│   ├── Cargo.toml
│   └── src/
│       └── main.rs
└── server/                      # Proof verifier (TODO)
    ├── Cargo.toml
    └── src/
        └── main.rs
```

## How It Works

### 1. Commitments (Privacy Model)

Instead of storing plaintext balances, we store **commitments**:

```
commitment = SHA256(account_id || balance || nonce || salt)
```

**Benefits:**
- Balance not publicly visible
- Can prove properties about committed values
- Can decrypt when needed (compliance, audits)

### 2. State Transitions

A transfer proves:

```
"I know secret data that:
  1. Opens to old commitments stored in NOMT
  2. Satisfies balance constraint (sender has enough funds)
  3. Produces new commitments after transfer"
```

**Public inputs** (on-chain):
- Old sender commitment
- New sender commitment
- Old receiver commitment
- New receiver commitment
- Old state root
- New state root

**Private witness** (client-side):
- Sender balance, nonce, salt
- Receiver balance, nonce, salt
- Transfer amount
- NOMT inclusion proofs

### 3. Circuit Constraints

The Binius circuit proves:

```rust
// 1. Old commitments are valid
assert sender_commitment_old == Hash(sender_id, sender_balance, sender_nonce, sender_salt)
assert receiver_commitment_old == Hash(receiver_id, receiver_balance, receiver_nonce, receiver_salt)

// 2. Sufficient balance
assert sender_balance >= amount

// 3. New commitments are correct
let new_sender_balance = sender_balance - amount
let new_sender_nonce = sender_nonce + 1
assert sender_commitment_new == Hash(sender_id, new_sender_balance, new_sender_nonce, new_sender_salt')

let new_receiver_balance = receiver_balance + amount
assert receiver_commitment_new == Hash(receiver_id, new_receiver_balance, receiver_nonce, receiver_salt')

// 4. NOMT witnesses are valid
verify_nomt_inclusion(sender_commitment_old, old_state_root, sender_witness)
verify_nomt_inclusion(receiver_commitment_old, old_state_root, receiver_witness)
```

## Current Status

### ✅ Completed

- [x] Architecture design (ARCHITECTURE.md)
- [x] Circuit logic and commitment scheme (circuit/src/lib.rs)
- [x] Test suite for circuit constraints
- [x] Documentation

### 🚧 In Progress / TODO

- [ ] **Actual Binius Integration**
  - Add Binius dependencies
  - Implement circuit using `CircuitBuilder`
  - Implement witness population with `WitnessFiller`
  - Use Binius `Sha256` gadget

- [ ] **NOMT Integration**
  - Add NOMT dependency
  - Implement witness generation
  - Implement witness verification in circuit
  - Test with actual NOMT database

- [ ] **Client Implementation**
  - CLI for creating transfers
  - Proof generation
  - HTTP client for submitting proofs

- [ ] **Server Implementation**
  - HTTP API endpoints
  - Proof verification
  - NOMT database management
  - Encrypted data storage

- [ ] **End-to-End Testing**
  - Integration tests
  - Performance benchmarks
  - Proof size measurements

## Building (Current)

The circuit module can be built and tested:

```bash
cd circuit
cargo test
```

This runs the constraint verification tests without actual zero-knowledge proofs.

## Building (Future - With Binius)

Once Binius is integrated:

```bash
# Build circuit
cd circuit
cargo build --release

# Build client
cd ../client
cargo build --release

# Build server
cd ../server
cargo build --release
```

## Running (Future)

### 1. Start the Server

```bash
cd server
cargo run --release
```

Server listens on `http://localhost:8080`:
- `POST /api/submit_transaction` - Submit proof + commitments
- `GET /api/state_root` - Get current NOMT root
- `GET /health` - Health check
- `GET /ready` - Readiness probe

### 2. Submit a Transfer

```bash
cd client
cargo run --release -- transfer \
  --sender-id 1 \
  --receiver-id 2 \
  --amount 100 \
  --server http://localhost:3000
```

This will:
1. Fetch current state root from server
2. Fetch NOMT witnesses for both accounts
3. Build Binius circuit
4. Generate proof (uses Ligerito PCS internally)
5. Submit proof + new commitments to server
6. Server verifies proof and updates NOMT

## What's Implemented

### ✅ Circuit Package (`circuit/`)
- **Commitment scheme**: Hash-based commitments hide account balances
- **Transfer circuit**: Proves valid balance updates and nonce increments
- **Ligerito integration**: Uses Ligerito PCS for polynomial commitments
- **Tests**: 5/6 tests passing (1 ignored due to stack usage)

Key file: `circuit/src/lib.rs:1-200`

### ✅ Server Package (`server/`)
- **HTTP API**: Axum-based REST server with proof verification
- **NOMT storage**: Authenticated state with Blake3 hashing
- **Stateless design**: Can scale horizontally in Kubernetes
- **Endpoints**: Health checks, state root queries, transaction submission

Key files:
- `server/src/main.rs:1-161` - HTTP server implementation
- `server/src/nomt_storage.rs:1-96` - NOMT wrapper with proper session management

### ✅ Kubernetes Deployment (`k8s/`)
- **Deployment**: Stateless verifier pods (3-20 replicas with autoscaling)
- **StatefulSet**: NOMT storage with persistent volumes
- **Services**: LoadBalancer for external access
- **HPA**: CPU-based horizontal pod autoscaling

Key file: `k8s/deployment.yaml:1-100`

## Technical Stack

| Component | Technology | Purpose |
|-----------|------------|---------|
| **PCS** | Ligerito | Polynomial commitment scheme for binary fields |
| **Field** | BinaryElem32/128 | Binary field arithmetic (efficient for hashing) |
| **State Storage** | NOMT | Authenticated state with Blake3, compact witnesses |
| **Commitment** | SHA256 hash | Simple, transparent, auditable |
| **Privacy Model** | Transparent commitments | Balance commitments (can decrypt for compliance) |
| **Server** | Axum + Tokio | Async HTTP server, stateless design |
| **Deployment** | Kubernetes | Horizontal scaling (3-20 pods) + StatefulSet for NOMT |

## Why This Approach?

1. **Edge Computation**: All heavy proof generation happens on client machines (distributed workload)
2. **Fast Verification**: Server only verifies proofs (~milliseconds) - can handle many concurrent requests
3. **Ligerito PCS**: Optimized for binary fields, compact proofs
4. **NOMT Storage**: Authenticated state with efficient witnesses
5. **Transparent Privacy**: Commitments hide balances but support compliance/auditing
6. **Cloud Native**: Stateless verifiers scale horizontally in Kubernetes (3-20 pods based on load)

## Resources

- **Binius**: https://github.com/IrreducibleOSS/binius
- **Ligerito**: `../../ligerito/` (this repo)
- **NOMT**: https://github.com/thrumdev/nomt
- **Penumbra**: https://github.com/penumbra-zone/penumbra
- **Architecture**: [ARCHITECTURE.md](ARCHITECTURE.md)

## License

Same as parent project
