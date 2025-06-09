# zeratul

## Ligerito Rust Implementation

This is a Rust implementation of the Ligerito polynomial commitment scheme
described in [this paper](https://angeris.github.io/papers/ligerito.pdf) by
Andrija Novakovic and Guillermo Angeris.

**⚠️ WARNING: This code is vibecoded and has not been audited, so likely
worse than rolling your own crypto.**

## Current Status

**🚧 WORK IN PROGRESS 🚧**

The implementation is incomplete. Major missing components:
- Binary field FFT for Reed-Solomon encoding is not implemented
- Field element conversions need proper irreducible polynomial reduction
- Polynomial evaluation functions need to be completed

Currently failing tests indicate these missing pieces.

## Features

- Binary extension fields GF(2^n) with SIMD acceleration
- FFT-based Reed-Solomon encoding with O(n log n) complexity (NOT YET IMPLEMENTED)
- Batched Merkle tree openings
- Recursive Ligero with sumcheck protocol
- Multi-threaded proving and verification

## Architecture

The implementation is organized as a Rust workspace with the following crates:

- `binary-fields`: Binary extension field arithmetic with SIMD optimizations
- `reed-solomon`: FFT-based Reed-Solomon encoding over binary fields
- `merkle-tree`: Merkle trees with batched opening support
- `ligerito`: Main Ligerito polynomial commitment implementation

## Requirements

- Rust 1.70 or later
- CPU with SIMD support (x86_64 with PCLMULQDQ or ARM with PMULL)
- Multiple CPU cores recommended for optimal performance

## Building

```bash 
# Clone the repository
git clone https://github.com/rotkonetworks/zeratul
cd zeratul

# Build in release mode
cargo build --release

# Run tests (currently failing due to incomplete implementation)
cargo test

# Run benchmarks
cargo bench
```

## Usage

### Running the Example

```bash
# Run with default settings
cargo run --release --example prove_verify

# Run with multiple threads
cargo run --release --example prove_verify -- --threads 8
```

### Basic API

```rust
use ligerito::{prover, verifier, hardcoded_config_24, hardcoded_config_24_verifier};
use binary_fields::{BinaryElem32, BinaryElem128};

// Create configuration
let config = hardcoded_config_24(
    std::marker::PhantomData::<BinaryElem32>,
    std::marker::PhantomData::<BinaryElem128>,
);

// Your polynomial
let poly: Vec<BinaryElem32> = vec![/* your data */];

// Generate proof
let proof = prover(&config, &poly)?;

// Verify proof
let verifier_config = hardcoded_config_24_verifier();
let is_valid = verifier(&verifier_config, &proof)?;
```

## Performance

TODO (pending complete implementation)

## Configuration

We provide several pre-configured parameter sets:

- `hardcoded_config_20`: For 2^20 polynomials
- `hardcoded_config_24`: For 2^24 polynomials
- `hardcoded_config_28`: For 2^28 polynomials
- `hardcoded_config_30`: For 2^30 polynomials

Each configuration has a corresponding verifier configuration.

## Implementation Notes

### Differences from Julia Implementation

- Uses Rust's type system for compile-time safety
- SIMD operations use platform intrinsics directly
- Parallelization via `rayon` instead of Julia's `@threads`
- Fiat-Shamir uses `merlin` or `sha2` for proper domain separation

### TODO

The following components need completion:

1. **Binary Fields**:
   - [x] Field addition and multiplication
   - [x] Field inversion via Fermat's little theorem
   - [ ] Field reduction with proper irreducible polynomials
   - [ ] Field embedding (beta root computation)
   - [x] Software fallback for carryless multiplication

2. **Reed-Solomon**:
   - [ ] Binary field FFT implementation
   - [ ] Non-systematic encoding implementation
   - [ ] Evaluation point generation (beta values)
   - [ ] Short twiddle extraction from long twiddles

3. **Ligero Verification**:
   - [ ] Proper polynomial evaluation at Reed-Solomon points
   - [ ] Consistency check implementation
   - [ ] Integration with FFT evaluations

4. **Sumcheck**:
   - [x] Multilinear polynomial partial evaluation
   - [x] Sumcheck prover/verifier instances
   - [x] Polynomial folding operations

5. **Utilities**:
   - [ ] s_k polynomial evaluation (proper implementation)
   - [ ] Scaled basis evaluation for Reed-Solomon

## Known Issues

- Tests are currently failing due to missing Reed-Solomon FFT implementation
- The verification always fails with error value 2017229001 because encoding is not happening
- Field element conversions may not properly handle irreducible polynomial reduction

## Contributing

This is a research implementation. Contributions should focus on:

1. Completing the TODO items (especially FFT implementation)
2. Improving performance
3. Adding comprehensive tests
4. Documenting the algorithms

## License

MIT License - see LICENSE file for details

## References

- [Ligerito: Lightweight Sublinear Arguments Without a Trusted Setup](https://angeris.github.io/papers/ligerito.pdf)
- [Original Julia Implementation](https://github.com/bcc-research/ligerito-impl.git)

## Acknowledgments

Thanks to Andrija Novakovic and Guillermo Angeris for the Ligerito construction
and the reference Julia implementation.
