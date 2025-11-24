# Ligerito Architecture & Feature Design

This document describes the modular architecture and feature flags for the Ligerito polynomial commitment scheme implementation, designed for maximum portability and flexibility.

## Overview

The `ligerito` crate is designed to be:
- **Portable**: Works in std, no_std, WASM, PolkaVM, and embedded environments
- **Modular**: Feature flags allow including only what you need
- **Flexible**: Supports custom configurations (BYOC - Bring Your Own Config)
- **Efficient**: Optional SIMD/parallel optimizations

## Feature Flags

### Environment

- **`std`** (default): Enable standard library support
  - Required for: Multi-threading, file I/O, dynamic allocation with std types
  - Enables: `serde/std`, `thiserror` error types

- **`no_std`**: Disable by using `--no-default-features`
  - Uses `core` and `alloc` only
  - Custom error types without `Display` impl details
  - No file I/O, no threading

### Functionality

- **`prover`** (default): Include proving functionality
  - Adds dependencies: `reed-solomon`, `rand`, `rand_chacha`
  - Enables `prove()`, `prove_sha256()`, `prove_with_transcript()`

- **`verifier-only`**: Minimal verifier-only build
  - Excludes all prover code and dependencies
  - Only `verify()` functions available
  - Smallest binary size

### Performance

- **`parallel`** (default): Multi-threaded proving with rayon
  - Requires `std`
  - 4-5x speedup on multi-core systems
  - Disable for single-threaded or no_std environments

- **`hardware-accel`** (default): SIMD acceleration
  - x86: pclmulqdq for GF(2^n) multiplication
  - Portable SIMD fallback (nightly)
  - ~2x speedup for FFT operations

### Transcript Implementations

- **`transcript-sha256`**: SHA-256 based Fiat-Shamir (always available)
  - Works in no_std
  - Default for verifier-only builds

- **`transcript-merlin`**: Merlin transcript protocol
  - Requires `std`
  - More robust domain separation

- **`transcript-blake3`**: BLAKE3 transcript (fastest)
  - Optional, requires `blake3` crate

### CLI & Tooling

- **`cli`**: Build the `ligerito` CLI binary
  - Requires `std` and `prover`
  - Adds: `clap`, `anyhow`, `bincode`, `hex`, `serde_json`
  - Enables prove/verify via stdin/stdout

## Build Configurations

### Full-Featured (default)

```bash
cargo build --release
```

Features: `std`, `prover`, `parallel`, `hardware-accel`

**Use for**: Development, benchmarking, native proving

### Verifier-Only (std)

```bash
cargo build --release --no-default-features --features="std,verifier-only"
```

**Use for**: PolkaVM, on-chain verification, resource-constrained environments

Binary size: ~50% smaller than full build

### Verifier-Only (no_std)

```bash
cargo build --release --no-default-features --features="verifier-only"
```

**Use for**: WASM, embedded systems, bare-metal environments

Requires: `alloc` support in target environment

### CLI Binary

```bash
cargo build --release --features=cli
# Or
cargo install --path ligerito --features=cli
```

**Use for**: Scripting, proof generation pipelines, testing

## CLI Usage

### Prove

```bash
# Generate proof from polynomial data
cat polynomial.bin | ligerito prove --size 20 > proof.bin

# With hex output
cat polynomial.bin | ligerito prove --size 20 --format hex > proof.hex
```

### Verify

```bash
# Verify proof (exit code 0 = valid, 1 = invalid)
cat proof.bin | ligerito verify --size 20

# Verbose output
cat proof.bin | ligerito verify --size 20 --verbose

# From hex
cat proof.hex | ligerito verify --size 20 --format hex
```

### Roundtrip

```bash
# Prove and verify in one pipeline
cat data.bin | ligerito prove --size 24 | ligerito verify --size 24
```

### Configuration

```bash
# Show config for a size
ligerito config --size 20

# TODO: Generate custom config (BYOC)
ligerito config --size 20 --generate > my_config.json
```

## BYOC (Bring Your Own Config)

The library supports custom configurations for advanced use cases:

### Current State

**Implemented:**
- Feature flags for verifier-only builds
- Hardcoded configs for sizes: 12, 16, 20, 24, 28, 30
- CLI framework with BYOC placeholders

**TODO (Future Work):**
1. Make `ProverConfig` and `VerifierConfig` fully serializable
2. Add config validation functions
3. Implement config loading in CLI:
   ```bash
   ligerito prove --config my_config.json < data.bin
   ligerito verify --config my_config.json < proof.bin
   ```
4. Add config generation:
   ```bash
   ligerito config --generate --recursive-steps 3 --dims "20,18,16" > custom.json
   ```

### Why BYOC?

- **Research**: Test different parameters
- **Optimization**: Tune for specific hardware
- **Integration**: Match configs from other implementations
- **Flexibility**: Support non-standard polynomial sizes

## PolkaVM Deployment

### Prerequisites

```bash
# Build polkaports SDK
cd ../polkaports
env CC=clang CXX=clang++ LLD=lld ./setup.sh corevm
. ./activate.sh corevm
```

### Build for PolkaVM

```bash
cd examples/polkavm_verifier
make

# Or manually:
cargo build --manifest-path ../../ligerito/Cargo.toml \
    --release \
    --features="std,verifier-only" \
    --example polkavm_verifier
```

### Deployment Options

**Option 1: Standalone Binary**
- Build verifier as RISC-V binary
- Deploy to PolkaVM runtime
- Read proof from stdin/memory
- Return verification result via exit code

**Option 2: FFI Library**
- Build as `cdylib` with C FFI
- Export `ligerito_verify(proof_ptr, proof_len, config_size)`
- Link with C/C++ PolkaVM applications

**Option 3: Embedded in Pallet**
- Include verifier code in Substrate pallet
- Call `verify()` from on-chain runtime
- Store proofs in blockchain storage

## Dependencies for no_std

To make the entire stack no_std compatible:

### Binary Fields (`binary-fields/`)

Status: ✅ Ready (already supports no_std)
- Remove `std` usage
- Use `core::` and `alloc::`
- SIMD: Conditional on target features

### Merkle Tree (`merkle-tree/`)

Status: ⚠️ Needs Work
- Replace `Vec` with `alloc::vec::Vec`
- Make `sha2` dependency no_std: `sha2 = { version = "0.10", default-features = false }`
- Remove `rayon` in no_std builds

### Reed-Solomon (`reed-solomon/`)

Status: N/A (verifier-only doesn't need this)

## Performance Characteristics

### Proving (with default features)

| Size | Elements | Time (8-core) | Memory |
|------|----------|---------------|--------|
| 2^20 | 1.05M | 68ms | ~50 MB |
| 2^24 | 16.8M | 1.24s | ~800 MB |
| 2^28 | 268.4M | 25.1s | ~12 GB |

### Verification (verifier-only, single-threaded)

| Size | Elements | Time | Memory |
|------|----------|------|--------|
| 2^20 | 1.05M | 22ms | ~20 MB |
| 2^24 | 16.8M | 470ms | ~150 MB |
| 2^28 | 268.4M | 8.5s | ~2 GB |

Verification is ~3-4x faster than proving due to:
- No Reed-Solomon encoding
- No Merkle tree construction
- Sequential operation (no parallelism overhead)

## Binary Sizes

Approximate compiled binary sizes (release, stripped):

| Configuration | Size | Notes |
|--------------|------|-------|
| Full (default) | ~15 MB | Prover + verifier, all optimizations |
| Verifier-only (std) | ~7 MB | 50% smaller, verifier only |
| Verifier-only (no_std) | ~4 MB | Minimal, no threading |
| CLI | ~16 MB | Full + CLI interface |

Sizes can be further reduced with:
- `opt-level = "z"` (optimize for size)
- `lto = true` (link-time optimization)
- `strip = true` (remove debug symbols)
- `panic = "abort"` (smaller panic handler)

## Testing

### Unit Tests

```bash
# All tests (requires prover feature)
cargo test

# Verifier-only tests
cargo test --no-default-features --features="std,verifier-only"
```

### Integration Tests

```bash
# CLI roundtrip test
dd if=/dev/urandom of=/tmp/test.bin bs=4 count=$((1 << 12))
cargo run --features=cli -- prove --size 12 < /tmp/test.bin | \
    cargo run --features=cli -- verify --size 12
```

### Benchmarks

```bash
# Full benchmark suite
cargo bench

# Verifier-only benchmarks
cargo bench --no-default-features --features="std,verifier-only"
```

## Future Enhancements

### Short Term
1. Complete BYOC implementation (config loading/generation)
2. Make dependencies fully no_std compatible
3. Add WASM example
4. Optimize verifier for smaller binary size

### Medium Term
1. Alternative field implementations (GF(2^64), GF(2^128))
2. Batch verification
3. Parallel verification (multi-proof batches)
4. Compressed proof format

### Long Term
1. STARK backend integration
2. Recursive composition
3. Hardware acceleration (GPU, FPGA)
4. Formal verification of core algorithms

## Contributing

When adding features:
1. Use feature gates appropriately
2. Test all feature combinations
3. Update this document
4. Add examples for new functionality
5. Ensure no_std compatibility where applicable

## License

MIT OR Apache-2.0
