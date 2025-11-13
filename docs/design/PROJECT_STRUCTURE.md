# Zeratul Project Structure

Complete overview of the Ligerito implementation codebase.

## Directory Layout

```
zeratul/
├── binary-fields/          # Binary extension field arithmetic (GF(2^n))
├── merkle-tree/            # SHA256-based Merkle trees
├── reed-solomon/           # Reed-Solomon encoding for proofs
├── ligerito/               # Main Ligerito polynomial commitment library
├── examples/               # Example applications and benchmarks
├── benchmarks/             # Performance benchmarking scripts
├── tests/                  # Integration tests
└── docs/                   # Documentation (*.md files in root)
```

## Core Libraries

### binary-fields/
Binary extension field arithmetic implementation.

**Purpose**: Implements GF(2^16), GF(2^32), GF(2^64), GF(2^128) fields
**Features**:
- `std` (default): Standard library support
- `serde`: Serialization support
- `simd`: Portable SIMD acceleration (nightly)
- `hardware-accel`: x86-specific optimizations (pclmulqdq, AVX-512)

**Key Files**:
- `src/elem.rs` - Field element implementations
- `src/poly.rs` - Polynomial arithmetic
- `src/simd.rs` - SIMD-accelerated operations

**Dependencies**: None (no_std compatible)

### merkle-tree/
Merkle tree implementation for commitments.

**Purpose**: SHA256-based Merkle trees with batched proofs
**Features**:
- `std` (default): Standard library support
- `serde`: Serialization support

**Key Files**:
- `src/lib.rs` - Complete and batched Merkle tree implementations

**Dependencies**: sha2, bytemuck, rayon

### reed-solomon/
Reed-Solomon error correction codes.

**Purpose**: FFT-based RS encoding for Ligero commitments
**Features**:
- Prover-only (not needed for verification)

**Key Files**:
- `src/lib.rs` - RS encoding with additive FFT

**Dependencies**: binary-fields, rayon

### ligerito/
Main Ligerito polynomial commitment scheme.

**Purpose**: Complete prove/verify implementation
**Features**:
- `std` (default): Standard library support
- `prover` (default): Include proving functionality
- `verifier-only`: Exclude prover (minimal build)
- `parallel` (default): Parallel processing with rayon
- `hardware-accel` (default): SIMD acceleration
- `transcript-merlin`: Merlin transcript (default)
- `transcript-sha256`: SHA256 transcript (always available)
- `cli`: CLI binary

**Key Files**:
```
ligerito/src/
├── lib.rs                  # Public API and exports
├── configs.rs              # Hardcoded configurations (2^12 to 2^30)
├── data_structures.rs      # Core types (Config, Proof, etc.)
├── transcript.rs           # Fiat-Shamir transcript implementations
├── utils.rs                # Utility functions (Lagrange basis, etc.)
├── sumcheck_polys.rs       # Sumcheck polynomial operations
├── sumcheck_verifier.rs    # Sumcheck verification logic
├── verifier.rs             # Main verification functions
├── ligero.rs               # Ligero commit/open (prover-only)
├── prover.rs               # Main proving functions (prover-only)
└── bin/
    └── ligerito.rs         # CLI binary
```

**Dependencies**:
- Core: binary-fields, merkle-tree, sha2, serde
- Prover: reed-solomon, rand, rand_chacha
- Parallel: rayon
- Transcript: merlin (optional), sha2
- CLI: clap, anyhow, bincode, hex, serde_json

## Examples

### examples/ (in workspace)
Performance benchmarks and integration examples.

**Benchmarks**:
- `bench_20.rs` - 2^20 polynomial benchmark
- `bench_28_30.rs` - Large proof benchmarks
- `bench_standardized_*.rs` - Standardized benchmarks for comparison
- `detailed_timing.rs` - Detailed timing breakdown
- `profile_*.rs` - Profiling examples

**Integration**:
- `prove_verify.rs` - Basic prove/verify example
- `fast_prove_verify.rs` - Optimized version
- `tiny_debug.rs` - Minimal debug example

**Blockchain**:
- `onchain_verifier_pallet.rs` - Substrate pallet (v1)
- `onchain_verifier_pallet_v2.rs` - Substrate pallet (v2)
- `polkadot_monitor*.rs` - Polkadot monitoring examples

### examples/http_verifier_server/
**Purpose**: HTTP REST API for proof verification
**Type**: Standalone package (not in workspace)
**Features**: Verifier-only build, production-ready

**Structure**:
```
http_verifier_server/
├── Cargo.toml          # Standalone workspace
├── src/
│   └── main.rs         # Axum HTTP server
├── README.md           # API documentation
└── test_client.sh      # Test script
```

**API Endpoints**:
- `POST /verify` - Verify proofs
- `GET /health` - Health check
- `GET /config` - Server configuration

**Dependencies**: axum, tokio, tower-http, serde, tracing

### examples/polkavm_verifier/
**Purpose**: PolkaVM/CoreVM verifier deployment
**Type**: Standalone example
**Features**: RISC-V target, minimal dependencies

**Structure**:
```
polkavm_verifier/
├── main.rs             # PolkaVM FFI interface
├── Makefile            # Build with polkaports
└── README.md           # PolkaVM deployment guide
```

## Benchmarks

### benchmarks/
Julia and shell scripts for performance comparison.

**Files**:
- `Ligerito.jl` - Reference Julia implementation
- `julia_large.sh` - Large proof benchmarks
- `compare_proper_tuned.sh` - Comparison script
- `run_proper_tuned.sh` - Tuned benchmark runner
- `setup_sudo.sh` - System configuration for benchmarking

**Results**: See `benchmarks/results/` for benchmark data

## Tests

### tests/integration_test.rs
Integration tests for the workspace.

### ligerito/tests/
Unit and integration tests for ligerito.

**Files**:
- `lib_tests.rs` - Library tests
- Test coverage for all proof sizes

## Documentation

### Root Documentation

**Getting Started**:
- `README.md` - Project overview and quick start
- `QUICKSTART.md` - Step-by-step tutorial
- `STATUS.md` - Current status and roadmap

**Technical**:
- `ARCHITECTURE.md` - System architecture and design
- `IMPLEMENTATION_SUMMARY.md` - Implementation details
- `PROJECT_STRUCTURE.md` - This file

**Library Specific**:
- `ligerito/README.md` - Ligerito library documentation
- `examples/http_verifier_server/README.md` - HTTP server guide

## Build Configurations

### Feature Flag Matrix

| Build Type | Features | Use Case | Binary Size |
|-----------|----------|----------|-------------|
| **Full** | `std, prover, parallel, hardware-accel, transcript-merlin` | Development, proving | ~8 MB |
| **Verifier-only** | `std, verifier-only` | Verification servers, blockchain | ~3 MB |
| **CLI** | `std, prover, parallel, cli, transcript-merlin` | Command-line tools | ~8 MB |
| **no_std** | (none) | Embedded, PolkaVM | ~2 MB |

### Common Build Commands

```bash
# Default build (full features)
cargo build --release

# Verifier-only
cargo build --release --no-default-features --features="std,verifier-only"

# CLI binary
cargo build --release --bin ligerito --features=cli

# HTTP server
cargo build --release --manifest-path examples/http_verifier_server/Cargo.toml

# no_std (future)
cargo build --release --no-default-features --target=riscv64gc-unknown-none-elf
```

## Development Workflow

### 1. Making Changes

```bash
# Run tests
cargo test --workspace

# Run specific tests
cargo test --package ligerito --lib

# Build all targets
cargo build --workspace --all-targets

# Check formatting
cargo fmt --all -- --check

# Run clippy
cargo clippy --workspace -- -D warnings
```

### 2. Benchmarking

```bash
# Run benchmarks
cargo bench --package ligerito

# Run custom benchmarks
cargo run --example bench_20 --release

# Compare with Julia
cd benchmarks && ./compare_proper_tuned.sh
```

### 3. Documentation

```bash
# Generate docs
cargo doc --workspace --no-deps --open

# Check doc links
cargo doc --workspace --no-deps
```

## Deployment Scenarios

### 1. Verification Server
Use `examples/http_verifier_server/` for HTTP API deployment.

**Target Platforms**:
- Cloud (AWS, GCP, Azure)
- Kubernetes
- Docker containers

### 2. Blockchain Integration
Use `examples/onchain_verifier_pallet_v2.rs` as template.

**Supported Chains**:
- Substrate-based chains
- Polkadot parachains

### 3. PolkaVM/CoreVM
Use `examples/polkavm_verifier/` for RISC-V deployment.

**Build Requirements**:
- RISC-V toolchain
- polkaports SDK
- musl/picoalloc

### 4. Embedded Systems
Use no_std build (future work).

**Target Platforms**:
- ARM Cortex-M
- RISC-V embedded
- Custom hardware

## Code Organization Principles

### 1. Separation of Concerns
- **Prover code**: `ligero.rs`, `prover.rs`, `reed-solomon/`
- **Verifier code**: `verifier.rs`, `sumcheck_verifier.rs`
- **Shared code**: `utils.rs`, `transcript.rs`, `data_structures.rs`

### 2. Feature Gating
- Use `#[cfg(feature = "prover")]` for prover-only code
- Use `#[cfg(feature = "parallel")]` for parallel code
- Use `#[cfg(feature = "std")]` for std-dependent code

### 3. No Circular Dependencies
```
binary-fields (base)
    ↓
merkle-tree, reed-solomon
    ↓
ligerito (top)
```

### 4. Minimal Public API
- Export only necessary types and functions
- Use `pub(crate)` for internal APIs
- Document all public items

## Future Organization

### Planned Additions

1. **docs/** directory
   - Move *.md files from root
   - Add diagrams and illustrations
   - Generate API documentation

2. **scripts/** directory
   - Move benchmark scripts
   - Add CI/CD scripts
   - Development utilities

3. **docker/** directory
   - Dockerfiles for various deployments
   - Docker compose files
   - Container documentation

4. **ci/** directory
   - GitHub Actions workflows
   - GitLab CI configuration
   - Test scripts

## Contributing

When adding new code, follow this structure:

1. **New libraries**: Add as workspace member in root `Cargo.toml`
2. **New examples**: Add to `examples/` or create standalone in `examples/*/`
3. **New tests**: Add to `tests/` or `ligerito/tests/`
4. **New docs**: Add to root or create `docs/` directory

## License

See LICENSE file in repository root.
