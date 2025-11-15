# Ligerito Quick Start Guide

## Installation

```bash
# Clone the repository
git clone https://github.com/your-org/zeratul
cd zeratul

# Build the library (default features)
cargo build --release

# Build the CLI tool
cargo build --release --features=cli

# Install CLI globally
cargo install --path ligerito --features=cli
```

## Basic Usage

### As a Library

```rust
use ligerito::{prove, verify, hardcoded_config_20, hardcoded_config_20_verifier};
use binary_fields::{BinaryElem32, BinaryElem128};
use std::marker::PhantomData;

fn main() {
    // Create prover config for 2^20 elements
    let prover_config = hardcoded_config_20(
        PhantomData::<BinaryElem32>,
        PhantomData::<BinaryElem128>,
    );

    // Your polynomial data (must be 2^20 elements)
    let polynomial: Vec<BinaryElem32> = vec![BinaryElem32::from(42); 1 << 20];

    // Generate proof
    let proof = prove(&prover_config, &polynomial).unwrap();
    println!("Proof generated successfully!");

    // Verify proof
    let verifier_config = hardcoded_config_20_verifier();
    let is_valid = verify(&verifier_config, &proof).unwrap();
    println!("Proof is valid: {}", is_valid);
}
```

### Using the CLI

#### Generate Proof

```bash
# Create test polynomial data (2^12 = 4096 elements, 4 bytes each)
dd if=/dev/urandom of=/tmp/test_poly.bin bs=4 count=4096

# Generate proof
cat /tmp/test_poly.bin | ligerito prove --size 12 > /tmp/proof.bin

# Generate proof with hex output
cat /tmp/test_poly.bin | ligerito prove --size 12 --format hex > /tmp/proof.hex
```

#### Verify Proof

```bash
# Verify proof (exit code 0 = valid, 1 = invalid)
cat /tmp/proof.bin | ligerito verify --size 12
# Output: VALID

# Verify with verbose output
cat /tmp/proof.bin | ligerito verify --size 12 --verbose
# Output:
# Proof size: 12345 bytes
# Proof structure size: 12000 bytes
# VALID

# Verify hex-encoded proof
cat /tmp/proof.hex | ligerito verify --size 12 --format hex
```

#### Roundtrip Test

```bash
# Prove and verify in one pipeline
dd if=/dev/urandom of=/tmp/test.bin bs=4 count=4096
cat /tmp/test.bin | ligerito prove --size 12 | ligerito verify --size 12
# Output: VALID
```

#### View Configuration

```bash
# Show configuration for a specific size
ligerito config --size 20

# Output:
# Ligerito Configuration for 2^20
# ====================================
# Polynomial elements: 2^20 = 1048576
# Recursive steps: 2
# Initial k: 4
# Recursive ks: [8, 8]
# Log dimensions: [18, 16]
#
# Estimated sizes:
#   Polynomial: 4194304 bytes (4.00 MB)
```

## Feature Flags

### Environment Features

```bash
# Standard library build (default)
cargo build --release

# no_std build (for embedded/WASM)
cargo build --release --no-default-features --features="verifier-only"
```

### Functionality Features

```bash
# Full prover + verifier (default)
cargo build --release

# Verifier only (smaller binary)
cargo build --release --no-default-features --features="std,verifier-only"
```

### Performance Features

```bash
# With SIMD acceleration (default)
cargo build --release

# Without SIMD (for compatibility)
cargo build --release --no-default-features --features="std,prover"

# With parallelism (default)
cargo build --release

# Single-threaded
cargo build --release --no-default-features --features="std,prover"
```

## Deploying to PolkaVM

### Build Verifier for PolkaVM

```bash
# Navigate to PolkaVM example
cd examples/polkavm_verifier

# Activate polkaports environment
cd ../../../polkaports
. ./activate.sh corevm
cd -

# Build
make
```

### Use as Library in Rust Code

```rust
// In your PolkaVM application
use ligerito::{verify, hardcoded_config_20_verifier, FinalizedLigeritoProof};
use binary_fields::{BinaryElem32, BinaryElem128};

fn verify_on_polkavm(proof_bytes: &[u8]) -> bool {
    // Deserialize proof
    let proof: FinalizedLigeritoProof<BinaryElem32, BinaryElem128> =
        bincode::deserialize(proof_bytes).unwrap();

    // Verify
    let config = hardcoded_config_20_verifier();
    verify(&config, &proof).unwrap()
}
```

### Build Configuration for PolkaVM

Since PolkaVM supports std Rust, use:

```toml
[dependencies]
ligerito = { path = "../../zeratul/ligerito", default-features = false, features = ["std", "verifier-only"] }
binary-fields = { path = "../../zeratul/binary-fields" }
bincode = "1.3"
```

## Advanced Usage

### Custom Polynomial Sizes

Supported sizes: 12, 16, 20, 24, 28, 30 (log2 of element count)

```rust
use ligerito::{prove, verify};
use ligerito::{hardcoded_config_24, hardcoded_config_24_verifier};

// For 2^24 = 16,777,216 elements
let config = hardcoded_config_24(/* ... */);
let proof = prove(&config, &large_polynomial).unwrap();
```

### Different Transcript Implementations

```rust
use ligerito::{prove_sha256, verify_sha256};

// Use SHA256 transcript (Julia-compatible)
let proof = prove_sha256(&prover_config, &polynomial).unwrap();
let valid = verify_sha256(&verifier_config, &proof).unwrap();
```

### Serialization

```rust
// Serialize proof to bytes
let proof_bytes = bincode::serialize(&proof).unwrap();

// Deserialize from bytes
let proof: FinalizedLigeritoProof<BinaryElem32, BinaryElem128> =
    bincode::deserialize(&proof_bytes).unwrap();

// Save to file
std::fs::write("/tmp/proof.bin", &proof_bytes).unwrap();

// Load from file
let loaded_bytes = std::fs::read("/tmp/proof.bin").unwrap();
let loaded_proof = bincode::deserialize(&loaded_bytes).unwrap();
```

## Troubleshooting

### Build Errors

**Error: "merlin" not found**
- Solution: Enable the `transcript-merlin` feature or use SHA256 transcript

**Error: "rayon" not found**
- Solution: Enable the `parallel` feature or use single-threaded build

**Error: Binary too large**
- Solution: Use `--no-default-features --features="std,verifier-only"` for smaller binary

### Runtime Errors

**Error: "Invalid proof structure"**
- Check that polynomial size matches config (e.g., 2^20 = 1048576 elements)
- Verify proof wasn't corrupted during transmission

**Error: "Verification failed"**
- Proof may be invalid or corrupted
- Ensure same transcript type for prove and verify
- Check that config matches between prover and verifier

### Performance Issues

**Slow proving**
- Enable `parallel` feature for multi-threading
- Enable `hardware-accel` for SIMD
- Disable SMT (hyperthreading) for best performance
- Use release build: `cargo build --release`

**High memory usage**
- Use smaller polynomial sizes
- Consider streaming for very large proofs
- Check for memory leaks in your polynomial generation

## Next Steps

- Read [`ARCHITECTURE.md`](ARCHITECTURE.md) for design details
- See [`examples/`](examples/) for more examples
- Check [`IMPLEMENTATION_SUMMARY.md`](IMPLEMENTATION_SUMMARY.md) for implementation notes
- Explore custom configurations (BYOC - coming soon)

## Support

- GitHub Issues: https://github.com/your-org/zeratul/issues
- Documentation: See [`docs/`](docs/)
- Examples: See [`examples/`](examples/)
