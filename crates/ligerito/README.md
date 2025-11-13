# Ligerito

Rust implementation of the [Ligerito polynomial commitment scheme](https://angeris.github.io/papers/ligerito.pdf) over binary extension fields.

[![License: MIT](https://img.shields.io/badge/License-MIT-yellow.svg)](https://opensource.org/licenses/MIT)
[![License: Apache 2.0](https://img.shields.io/badge/License-Apache%202.0-blue.svg)](https://opensource.org/licenses/Apache-2.0)

## Features

- 🚀 **Fast**: SIMD-accelerated GF(2^n) arithmetic and parallel FFT
- 📦 **Modular**: Feature flags for std/no_std, prover/verifier-only
- 🔧 **Flexible**: Multiple transcript implementations (Merlin, SHA256, BLAKE3)
- 🎯 **Portable**: Works on std, PolkaVM, WASM, and embedded targets
- 🛠️ **Tooling**: CLI for prove/verify workflows

## Quick Start

```rust
use ligerito::{prove, verify, hardcoded_config_20, hardcoded_config_20_verifier};
use binary_fields::{BinaryElem32, BinaryElem128};
use std::marker::PhantomData;

// Create prover config for 2^20 elements
let prover_config = hardcoded_config_20(
    PhantomData::<BinaryElem32>,
    PhantomData::<BinaryElem128>,
);

// Your polynomial (must be 2^20 elements)
let polynomial: Vec<BinaryElem32> = vec![BinaryElem32::from(42); 1 << 20];

// Generate proof
let proof = prove(&prover_config, &polynomial).unwrap();

// Verify proof
let verifier_config = hardcoded_config_20_verifier();
let is_valid = verify(&verifier_config, &proof).unwrap();
assert!(is_valid);
```

## Installation

Add to your `Cargo.toml`:

```toml
[dependencies]
ligerito = "0.1"
binary-fields = "0.1"
```

## Feature Flags

### Environment
- **`std`** (default): Enable standard library
- **`no_std`**: Use `--no-default-features` for no_std environments

### Functionality
- **`prover`** (default): Include proving functionality
- **`verifier-only`**: Minimal verifier-only build (~50% smaller)

### Performance
- **`parallel`** (default): Multi-threaded proving with rayon
- **`hardware-accel`** (default): SIMD acceleration for GF(2^n)

### Transcripts
- **`transcript-sha256`**: SHA256-based Fiat-Shamir (always available)
- **`transcript-merlin`**: Merlin transcript protocol (requires std)
- **`transcript-blake3`**: BLAKE3 transcript (fastest)

### Tooling
- **`cli`**: Build CLI binary for prove/verify workflows

## Build Configurations

### Full-Featured (Default)

```bash
cargo build --release
```

Includes: prover, verifier, parallelism, SIMD

### Verifier-Only

```bash
cargo build --release --no-default-features --features="std,verifier-only"
```

Perfect for: PolkaVM, on-chain verification, constrained environments

### CLI Tool

```bash
cargo install --path . --features=cli
```

Usage:
```bash
# Generate proof
cat polynomial.bin | ligerito prove --size 20 > proof.bin

# Verify proof
cat proof.bin | ligerito verify --size 20
```

### no_std (Embedded/WASM)

```bash
cargo build --release --no-default-features --features=verifier-only
```

Requires: `alloc` in target environment

## Supported Polynomial Sizes

- 2^12 (4,096 elements)
- 2^16 (65,536 elements)
- 2^20 (1,048,576 elements) **← Recommended**
- 2^24 (16,777,216 elements)
- 2^28 (268,435,456 elements)
- 2^30 (1,073,741,824 elements)

## Performance

Benchmarked on AMD Ryzen 9 7945HX (8 cores, SMT disabled):

| Size | Elements | Proving | Verification |
|------|----------|---------|--------------|
| 2^20 | 1.05M | 68ms | 22ms |
| 2^24 | 16.8M | 1.24s | 470ms |
| 2^28 | 268.4M | 25.1s | 8.5s |

See [benchmarks](../BENCHMARKS.md) for detailed results.

## Documentation

- **[Quick Start Guide](../QUICKSTART.md)** - Get started in 5 minutes
- **[Architecture](../ARCHITECTURE.md)** - Design and feature flags
- **[Implementation Summary](../IMPLEMENTATION_SUMMARY.md)** - Technical details
- **[API Docs](https://docs.rs/ligerito)** - Full API documentation

## Examples

### Library Usage

```rust
// Different transcript types
use ligerito::{prove_sha256, verify_sha256};
let proof = prove_sha256(&config, &poly).unwrap();
let valid = verify_sha256(&verifier_config, &proof).unwrap();

// Custom configuration (coming soon)
let config = VerifierConfig {
    recursive_steps: 2,
    initial_dim: 20,
    // ...
};
```

### CLI Usage

```bash
# Show configuration
ligerito config --size 20

# Roundtrip test
dd if=/dev/urandom of=test.bin bs=4 count=$((1 << 12))
cat test.bin | ligerito prove --size 12 | ligerito verify --size 12
```

### PolkaVM Deployment

```bash
# Build verifier for PolkaVM
cd examples/polkavm_verifier
make
```

See [PolkaVM example](../examples/polkavm_verifier/) for details.

## Use Cases

- **On-chain verification** - Verify proofs in Substrate pallets
- **Light clients** - Succinct polynomial commitments
- **Data availability** - Efficient Reed-Solomon encoding verification
- **Zero-knowledge proofs** - Building block for ZK systems
- **Research** - Experimentation with binary field commitments

## Project Structure

```
ligerito/
├── src/
│   ├── lib.rs          # Main library interface
│   ├── prover.rs       # Proving logic
│   ├── verifier.rs     # Verification logic
│   ├── transcript.rs   # Fiat-Shamir transcripts
│   ├── configs.rs      # Hardcoded configurations
│   └── bin/
│       └── ligerito.rs # CLI binary
├── examples/
│   ├── polkavm_verifier/ # PolkaVM deployment
│   └── ...
└── tests/
    └── ...
```

## Dependencies

- `binary-fields` - GF(2^n) arithmetic with SIMD
- `reed-solomon` - FFT-based encoding (prover only)
- `merkle-tree` - SHA256 Merkle trees
- `sha2` - SHA256 hashing
- `serde` - Serialization

Optional:
- `rayon` - Parallelism (with `parallel` feature)
- `merlin` - Merlin transcript (with `transcript-merlin` feature)
- `clap`, `anyhow` - CLI (with `cli` feature)

## Contributing

Contributions welcome! Please:

1. Check [issues](https://github.com/your-org/zeratul/issues) for ideas
2. Open an issue before major changes
3. Add tests for new features
4. Update documentation
5. Follow Rust style guidelines

## Testing

```bash
# Run all tests
cargo test --release

# Verifier-only tests
cargo test --no-default-features --features="std,verifier-only"

# Benchmarks
cargo bench
```

## Roadmap

### v0.2 (In Progress)
- [ ] Complete BYOC (custom configuration loading)
- [ ] Full no_std support (update dependencies)
- [ ] WASM example and benchmarks
- [ ] Batch verification

### v0.3 (Planned)
- [ ] Compressed proof format
- [ ] Alternative field sizes (GF(2^64), GF(2^128))
- [ ] Recursive composition
- [ ] Formal verification

## License

Licensed under either of:

- Apache License, Version 2.0 ([LICENSE-APACHE](../LICENSE-APACHE) or http://www.apache.org/licenses/LICENSE-2.0)
- MIT license ([LICENSE-MIT](../LICENSE-MIT) or http://opensource.org/licenses/MIT)

at your option.

## References

- [Ligerito Paper](https://angeris.github.io/papers/ligerito.pdf) by Andrija Novakovic and Guillermo Angeris
- [Binary Field Arithmetic](https://en.wikipedia.org/wiki/Finite_field_arithmetic)
- [Ligero Protocol](https://eprint.iacr.org/2017/872) (predecessor)

## Acknowledgments

- Original Ligerito design by Andrija Novakovic and Guillermo Angeris
- Inspired by [ashutosh1206's implementation](https://github.com/ashutosh1206/ligerito-bounty)
- Built with ❤️ by the Rotko Networks team
