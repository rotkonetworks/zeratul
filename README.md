# zeratul

rust implementation of [ligerito](https://angeris.github.io/papers/ligerito.pdf) polynomial commitment scheme over binary extension fields.

## structure

- `binary-fields/` - gf(2^n) arithmetic with constant-time operations
- `reed-solomon/` - parallel fft-based encoding over binary fields
- `merkle-tree/` - sha256 commitment trees
- `ligerito/` - sumcheck-based polynomial commitments

## performance

measured on amd ryzen 9 7945hx (32 threads) 96gb ddr5:

### standardized benchmarks

all implementations tested with identical parameters (sha256 transcript):

#### 2^20 (1,048,576 elements)

| implementation | proving | verification | speedup |
|----------------|---------|--------------|---------|
| **ligerito.jl** | **60ms** | **42ms** | baseline |
| zeratul | 175ms | 96ms | 2.9x slower (was 3.1x) |
| ashutosh-ligerito | 3,417ms | 258ms | 57x slower |

#### 2^24 (16,777,216 elements)

| implementation | proving | verification | speedup |
|----------------|---------|--------------|---------|
| **ligerito.jl** | **708ms** | **162ms** | baseline |
| zeratul | 3,465ms | 1,199ms | **4.9x slower** (was 5.0x) |

⚠️ **performance degrades at scale**: gap increases from 2.9x at 2^20 to 4.9x at 2^24, suggesting bottlenecks in fft/reed-solomon or memory allocation patterns that don't scale well.

**recent optimizations (committed separately):**
- enabled hardware-accel feature with target-cpu=native (pclmulqdq always-on)
- removed runtime feature detection overhead in hot paths
- improved fft parallelization with better work distribution
- 7% faster proving at 2^20, 1.5% at 2^24 (38% faster verification at 2^20)

note: julia benchmarks exclude jit compilation time via warmup runs. zeratul uses simd (pclmulqdq) for gf(2^128) multiplication + parallel sumcheck (rayon).

#### larger sizes (zeratul only)

| size | elements | proving | verification | proof size |
|------|----------|---------|--------------|------------|
| 2^20 | 1.05M | 184ms | 131ms | 147 KB |
| 2^24 | 16.8M | 4.3s | 2.4s | 241 KB |
| 2^28 | 268.4M | 109s | 26.6s | 364 KB |
| 2^30 | 1.07B | 318s | 41.7s | 423 KB |

### reproducing

```bash
git submodule update --init --recursive
./benchmarks/run_standardized_benchmarks.sh
```

see `benchmarks/RESULTS.md` for detailed methodology and analysis.

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
- verifier kept single-threaded for now

```sh
curl -sL https://githem.com/rotkonetworks/zeratul | claude -p "analyze features"
```

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

## notes on llm-assisted development

this code vibes hard and was built with llm assistance. what actually worked:

1. **channel the best** - pointed at isis lovecruft's style for commit
   messages, looked at how top cryptographers write code instead of generic
"best practices"

2. **meditate on failure** - when things broke or benchmarks sucked, asked llm
   to reflect on what went wrong and find the headspace to try differently. very
similar to sport coaching - you don't just say "do better", you work through the
mental blockers.

3. **iterate through pain** - let test failures and compiler errors be the
   teacher. when reduction algorithm for gf(2^128) failed, reverted and learned
instead of forcing it

4. **the julia 1-indexing hell** - major hardship was translating julia
   reference implementation (1-indexed) to rust (0-indexed). off-by-one errors
in polynomial evaluation and fft indexing caused subtle math bugs

5. **minimal vibe-based prompts** - "yes lets keep improving prover and verifier
   times" or "pedal to the metal, we want to be on L1 cache"

6. **benchmark against reality** - added reference implementations as
   submodules, ran same tests on same hardware, no speculation, claude code
   improvements were the final nail in the coffin

the key: treat it like coaching - point at role models, create space to reflect
on failures, require explanations, iterate based on real feedback not theory.

## license

MIT

## support
penumbra13pqjwqyqqd3u7jw86a7ncekc6xg07d7gfk36ywlym2twypqh7xusmjnamz5ketvk2hxklpm8gdn4sdp3m333fvsq0nwly4lhzzzekjf9r9e98mpt0tdn8e6pzg65nx9uv2x4f4

## references

- [ligerito paper](https://angeris.github.io/papers/ligerito.pdf) by novakovic & angeris
- [ligerito.jl](https://github.com/bcc-research/Ligerito.jl) - julia reference implementation
- [ligerito-impl](https://github.com/bcc-research/ligerito-impl) - optimized julia components
- [ashutosh1206/ligerito-rust](https://github.com/ashutosh1206/ligerito-rust) - rust reference port
