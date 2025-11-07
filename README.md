# ligerito

fast rust implementation of the [ligerito polynomial commitment scheme](https://angeris.github.io/papers/ligerito.pdf)

## performance

**cpu**: AMD Ryzen 9 7945HX (16 cores / 32 threads)

benchmarked on 2^20 (1,048,576) elements:

| implementation | transcript | prove | verify | total | proof size |
|---------------|-----------|-------|--------|-------|------------|
| **ours (rust)** | **sha256** | **190ms** | **122ms** | **312ms** | **145 KB** |
| **ours (rust)** | **merlin** | **198ms** | **123ms** | **322ms** | **145 KB** |
| julia | sha256 | 3,262ms | 383ms | 3,645ms | 147 KB |
| ashutosh (rust) | sha256 | 3,600ms | 279ms | 3,880ms | 105 KB |

**11-19x faster** than other implementations. both transcripts deliver similar performance.

**note on proof sizes**: our proofs are 145 KB vs ashutosh's 105 KB. the 38% difference comes from simpler merkle proof batching (no deduplication). this is an acceptable tradeoff for 12x performance gain. see [PROOF_SIZE_INVESTIGATION.md](PROOF_SIZE_INVESTIGATION.md) for details.

## usage

```rust
use ligerito::{prove, verify, hardcoded_config_20, hardcoded_config_20_verifier};
use binary_fields::{BinaryElem32, BinaryElem128};
use std::marker::PhantomData;

// create config for 2^20 elements
let config = hardcoded_config_20(
    PhantomData::<BinaryElem32>,
    PhantomData::<BinaryElem128>,
);

// generate proof
let poly: Vec<BinaryElem32> = (0..1048576)
    .map(|i| BinaryElem32::from(i as u32))
    .collect();
let proof = prove(&config, &poly)?;

// verify
let verifier_config = hardcoded_config_20_verifier();
let valid = verify(&verifier_config, &proof)?;
```

## build

```bash
cargo build --release
cargo test --release
```

## run

```bash
# performance benchmark
cargo run --release --example performance_benchmark

# basic usage
cargo run --release --example prove_verify
```

## configs

| config | polynomial size | elements |
|--------|----------------|----------|
| `hardcoded_config_12` | 2^12 | 4,096 |
| `hardcoded_config_16` | 2^16 | 65,536 |
| `hardcoded_config_20` | 2^20 | 1,048,576 |
| `hardcoded_config_24` | 2^24 | 16,777,216 |
| `hardcoded_config_28` | 2^28 | 268,435,456 |
| `hardcoded_config_30` | 2^30 | 1,073,741,824 |

## features

- parallel proving via rayon (17x faster than single-threaded)
- parallel verification (3x faster)
- merlin transcript (default) or sha256 (julia-compatible)
- optimized binary field operations with simd
- zero-copy serialization
- comprehensive test suite (36 passing tests)

## transcript types

```rust
// merlin (default, fastest)
let proof = prove(&config, &poly)?;
let valid = verify(&verifier_config, &proof)?;

// sha256 (julia-compatible)
let proof = prove_sha256(&config, &poly)?;
let valid = verify_sha256(&verifier_config, &proof)?;
```

## requirements

- rust 1.70+
- cpu with simd support (x86_64 pclmulqdq or arm pmull)
- multi-core cpu recommended

## license

mit

## references

- [ligerito paper](https://angeris.github.io/papers/ligerito.pdf) by novakovic & angeris
- [julia implementation](https://github.com/bcc-research/ligerito-impl)
