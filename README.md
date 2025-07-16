# zeratul

## Ligerito Rust Implementation

This is a Rust implementation of the Ligerito polynomial commitment scheme
described in [this paper](https://angeris.github.io/papers/ligerito.pdf) by
Andrija Novakovic and Guillermo Angeris.

**⚠️ WARNING: This code is vibecoded and has not been audited, so likely
worse than rolling your own crypto.**

## Current Status

**🚧 WORK IN PROGRESS 🚧**

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

## License

MIT License

## References

- [Ligerito: Lightweight Sublinear Arguments Without a Trusted Setup](https://angeris.github.io/papers/ligerito.pdf)
- [Original Julia Implementation](https://github.com/bcc-research/ligerito-impl.git)

## Acknowledgments

Thanks to Andrija Novakovic and Guillermo Angeris for the Ligerito construction
and the reference Julia implementation.
