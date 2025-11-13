# PolkaVM Verifier Service

Complete end-to-end verifier service using PolkaVM.

## Architecture

```
┌─────────────────────────────────────────────────────────┐
│ CLIENT (HTTP/WebSocket)                                 │
│   POST /verify { proof_size, proof_bytes }             │
└───────────────────┬─────────────────────────────────────┘
                    │
                    ▼
┌─────────────────────────────────────────────────────────┐
│ HOST SERVICE (Rust + axum)                              │
│  - Receives HTTP requests                               │
│  - Manages PolkaVM instances                            │
│  - Returns JSON responses                               │
│                                                          │
│  ┌────────────────────────────────────────────────────┐ │
│  │ PolkaVM Runtime                                    │ │
│  │  ┌──────────────────────────────────────────────┐  │ │
│  │  │ GUEST (ligerito_verifier.polkavm)            │  │ │
│  │  │  - RISC-V binary                             │  │ │
│  │  │  - Reads proof from stdin                    │  │ │
│  │  │  - Runs verify()                             │  │ │
│  │  │  - Returns exit code: 0=valid, 1=invalid     │  │ │
│  │  └──────────────────────────────────────────────┘  │ │
│  └────────────────────────────────────────────────────┘ │
└─────────────────────────────────────────────────────────┘
```

## Components

### 1. Guest Verifier (`../polkavm_verifier`)
- RISC-V binary built with polkaports
- Runs inside PolkaVM sandbox
- Protocol:
  - Input (stdin): `[config_size: u32][proof_bytes: bincode]`
  - Output: Exit code (0=valid, 1=invalid, 2=error)

### 2. Host Service (this directory)
- HTTP API server using axum
- Manages PolkaVM execution
- Endpoints:
  - `POST /verify` - Verify a proof
  - `GET /health` - Service health
  - `GET /config` - Configuration info

## Building

### Step 1: Build the Guest Verifier

```bash
cd ../polkavm_verifier
. ../../polkaports/activate.sh polkavm
make
```

This creates: `ligerito_verifier.polkavm`

### Step 2: Build the Host Service

```bash
cd ../polkavm_service
cargo build --release
```

## Running

```bash
cd examples/polkavm_service
cargo run --release -- ../polkavm_verifier/ligerito_verifier.polkavm
```

The service will start on `http://0.0.0.0:3000`

## API

### POST /verify

Verify a proof.

**Request:**
```json
{
  "proof_size": 20,
  "proof_bytes": "<hex-encoded-bincode-proof>"
}
```

**Response (valid):**
```json
{
  "valid": true,
  "execution_time_ms": 23
}
```

**Response (invalid):**
```json
{
  "valid": false,
  "error": "Verification failed",
  "execution_time_ms": 22
}
```

### GET /health

Health check.

**Response:**
```json
{
  "status": "ok",
  "polkavm_loaded": true
}
```

### GET /config

Get configuration.

**Response:**
```json
{
  "supported_sizes": [12, 16, 20, 24, 28, 30],
  "max_proof_bytes": 104857600
}
```

## Example Usage

### Using curl:

```bash
# Health check
curl http://localhost:3000/health

# Get config
curl http://localhost:3000/config

# Verify a proof (with hex-encoded proof bytes)
curl -X POST http://localhost:3000/verify \
  -H "Content-Type: application/json" \
  -d '{
    "proof_size": 20,
    "proof_bytes": "0102030405..."
  }'
```

### Using Python:

```python
import requests
import bincode  # pip install bincode

# Generate proof (using ligerito prover)
proof = generate_proof()  # Your proof generation

# Serialize
proof_bytes = bincode.encode(proof)

# Send to verifier
response = requests.post('http://localhost:3000/verify', json={
    'proof_size': 20,
    'proof_bytes': proof_bytes.hex()
})

print(response.json())
# {"valid": true, "execution_time_ms": 23}
```

## Use Cases

### 1. Standalone Verifier Service
Deploy on a server to provide proof verification as a service.

```bash
# Production deployment
cargo build --release
./target/release/polkavm-service ./ligerito_verifier.polkavm
```

### 2. Integration with Blockchain
Use as an off-chain verifier for blockchain proofs.

### 3. Testing Infrastructure
Run in CI/CD to verify proofs as part of testing.

### 4. Development/Debugging
Use the HTTP API to test proof generation and verification.

## Security Considerations

### PolkaVM Sandbox
- Guest runs in isolated PolkaVM environment
- No access to host filesystem (beyond what's explicitly provided)
- Limited syscalls available
- Single-threaded execution

### Resource Limits
- Set appropriate timeouts for verification
- Limit proof size (currently 100 MB max)
- Consider rate limiting in production

### Input Validation
- Proof bytes are validated during deserialization
- Invalid proofs return error (exit code 2)
- Malformed requests return HTTP 400

## Performance

### Expected Performance (2^20 polynomial):
- Verification time: ~20-30ms (sequential)
- Memory usage: ~50-100 MB
- Binary size: ~3-4 MB (guest verifier)

### Scaling:
- Single-threaded guest execution
- Can run multiple PolkaVM instances in parallel (host-level)
- Consider load balancing for high traffic

## Troubleshooting

### Error: "Failed to read PolkaVM binary"
Make sure you built the guest verifier first:
```bash
cd ../polkavm_verifier
. ../../polkaports/activate.sh polkavm
make
```

### Error: "Failed to parse PolkaVM binary"
The binary might be corrupt or not a valid PolkaVM format. Rebuild:
```bash
cd ../polkavm_verifier
make clean && make
```

### Error: "Execution failed"
Check the guest verifier logs. The error details will be in the response:
```json
{
  "valid": false,
  "error": "Verification error: ...",
  "execution_time_ms": 5
}
```

## Development

### Running with debug logging:
```bash
RUST_LOG=debug cargo run -- ../polkavm_verifier/ligerito_verifier.polkavm
```

### Testing locally:
```bash
# Terminal 1: Start service
cargo run

# Terminal 2: Send test request
curl http://localhost:3000/health
```

## Future Enhancements

- [ ] WebSocket support for streaming verification
- [ ] Batch verification (multiple proofs)
- [ ] Proof caching
- [ ] Metrics and monitoring (Prometheus)
- [ ] Rate limiting
- [ ] Authentication/API keys
- [ ] Docker deployment
- [ ] Kubernetes manifests

## File Structure

```
polkavm_service/
├── Cargo.toml                   # Host service dependencies
├── src/
│   ├── main.rs                  # HTTP server + API
│   └── polkavm_runner.rs        # PolkaVM execution wrapper
└── README.md                    # This file
```

## Related

- Guest verifier: `../polkavm_verifier/`
- HTTP server (standalone): `../http_verifier_server/`
- Library: `../../ligerito/`
