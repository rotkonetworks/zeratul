# HTTP Verifier Server Example

A minimal HTTP server for verifying Ligerito proofs. This example demonstrates how to build a verifier-only deployment suitable for constrained environments like PolkaVM.

## Features

- **Verifier-Only Build**: Uses `verifier-only` feature flag to exclude prover dependencies
- **Minimal Dependencies**: No rand, reed-solomon, or other prover-only crates
- **REST API**: Simple HTTP endpoints for proof verification
- **Multiple Proof Sizes**: Supports 2^12, 2^16, 2^20, and 2^24 polynomial sizes

## Building

```bash
cargo build --manifest-path examples/http_verifier_server/Cargo.toml --release
```

The binary will be in `target/release/http-verifier-server`.

## Running

```bash
cargo run --manifest-path examples/http_verifier_server/Cargo.toml
```

Or with custom logging:

```bash
RUST_LOG=debug cargo run --manifest-path examples/http_verifier_server/Cargo.toml
```

## API Endpoints

### POST /verify

Verify a Ligerito proof.

**Request Body** (JSON):
```json
{
  "proof_size": 20,
  "proof_bytes": [/* bincode-encoded proof as byte array */]
}
```

**Response** (JSON):
```json
{
  "valid": true,
  "proof_size": 20
}
```

Or on error:
```json
{
  "valid": false,
  "proof_size": 20,
  "error": "Verification failed"
}
```

### GET /health

Health check endpoint.

**Response**:
```json
{
  "status": "ok",
  "version": "0.1.0"
}
```

### GET /config

Get server configuration.

**Response**:
```json
{
  "supported_sizes": [12, 16, 20, 24],
  "verifier_only": true
}
```

## Example Usage

### Using curl

```bash
# Health check
curl http://localhost:3000/health

# Get config
curl http://localhost:3000/config

# Verify a proof (assuming you have a proof file)
curl -X POST http://localhost:3000/verify \
  -H "Content-Type: application/json" \
  -d @proof_request.json
```

### Using the Ligerito CLI

```bash
# Generate a proof with the CLI
echo "your_data_here" | cargo run --manifest-path ligerito/Cargo.toml --bin ligerito -- prove --size 20 > proof.bin

# Convert to JSON request format (you'll need to implement this helper)
# Then send to server
curl -X POST http://localhost:3000/verify \
  -H "Content-Type: application/json" \
  -d "{\"proof_size\": 20, \"proof_bytes\": $(cat proof.bin | base64 -w 0)}"
```

## Deployment

### Docker

```dockerfile
FROM rust:1.75 as builder
WORKDIR /app
COPY . .
RUN cargo build --manifest-path examples/http_verifier_server/Cargo.toml --release

FROM debian:bookworm-slim
RUN apt-get update && apt-get install -y ca-certificates && rm -rf /var/lib/apt/lists/*
COPY --from=builder /app/target/release/http-verifier-server /usr/local/bin/
EXPOSE 3000
CMD ["http-verifier-server"]
```

### PolkaVM

This example can be adapted for PolkaVM by:
1. Building with `--target=riscv64gc-unknown-linux-gnu`
2. Using the PolkaVM runtime instead of tokio
3. Implementing host functions for HTTP I/O

See `../polkavm_verifier/` for a PolkaVM-specific example.

## Performance

The verifier-only build is significantly smaller than a full build:

| Build Type | Binary Size | Dependencies |
|-----------|-------------|--------------|
| Full (prover + verifier) | ~8 MB | rand, reed-solomon, rayon, etc. |
| Verifier-only | ~3 MB | SHA256, merkle-tree only |

Memory usage during verification:
- 2^12: ~500 KB
- 2^16: ~2 MB
- 2^20: ~8 MB
- 2^24: ~32 MB

## Security Considerations

1. **Rate Limiting**: Add rate limiting to prevent DoS attacks
2. **Input Validation**: Validate proof size and length before deserialization
3. **Timeouts**: Set verification timeouts to prevent resource exhaustion
4. **HTTPS**: Use HTTPS in production (add TLS layer)
5. **Authentication**: Add API key or JWT authentication if needed

## License

Same as the parent Ligerito project.
