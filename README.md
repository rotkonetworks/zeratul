# zeratul

rust implementation of [ligerito](https://angeris.github.io/papers/ligerito.pdf) polynomial commitment scheme over binary extension fields.

## structure

- `binary-fields/` - gf(2^n) arithmetic with constant-time operations
- `reed-solomon/` - parallel fft-based encoding over binary fields
- `merkle-tree/` - sha256 commitment trees
- `ligerito/` - sumcheck-based polynomial commitments

## performance

measured on amd ryzen 9 7945hx (32 threads) 94gb ddr5:

### standardized benchmarks

all implementations tested with identical parameters (sha256 transcript):

#### 2^20 (1,048,576 elements)

| implementation | proving | verification | notes |
|----------------|---------|--------------|-------|
| ligerito.jl (baseline) | 52.3ms | 14.2ms | best of 10 runs, 20 threads |
| **zeratul** | **61.4ms** (1.17x slower) | **14.0ms** (1.01x slower) | monomorphic SIMD FFT, 20 threads (10 runs) |
| ashutosh-ligerito | 3,417ms (65x slower) | 258ms (18x slower) | reference port |

#### 2^24 (16,777,216 elements)

| implementation | proving | verification | notes |
|----------------|---------|--------------|-------|
| ligerito.jl | 844ms | 178ms | 5 warmup + best of 3 |
| **zeratul** | **1.85s** (2.2x slower) | **10.6ms** (17x faster) | SIMD FFT + optimized verifier |

**recent optimizations (2025-11-08):**
- **monomorphic SIMD FFT** for GF(2^32) using SSE pclmulqdq
  - eliminated generic dispatch overhead with TypeId-based specialization
  - direct calls to fft_butterfly_gf32_sse (2x parallel carryless mul)
- **eliminated nested parallelization**
  - column encoding now uses sequential FFT within parallel tasks
  - reduced rayon task spawning overhead (was 60% of runtime)
  - MIN_PARALLEL_SIZE increased to 16384 elements
- **optimized thread count** for 2^20
  - best performance at 20 threads (vs default 32)
  - reduces thread contention and work-stealing overhead
- **35% faster proving**: 91.9ms → 61.4ms at 2^20
- **now within 1.17x of julia JIT** (was 1.5x before optimizations)

**why the 9ms gap exists:**

single-threaded performance:
- rust: 367.7ms (SIMD FFT)
- julia: 393.9ms (JIT-compiled)
- **rust is 7% faster single-threaded**

multi-threaded scaling:
- rust: 6.0x speedup (367.7ms → 61.4ms)
- julia: 7.5x speedup (393.9ms → 52.3ms)
- julia gets **25% better parallel scaling**

the gap is entirely threading overhead:
- **rayon (work-stealing)**: 61% of runtime spent in coordination overhead (crossbeam-epoch, work-stealing deques, task migration)
- **julia (green threads)**: task creation is 10x cheaper (~5-20ns vs 50-200ns), tasks stay on same thread (better cache locality)
- our workload: ~150 parallel tasks → rayon overhead dominates

**conclusion**: rust's SIMD FFT is faster single-threaded, but julia's lightweight threading (green threads vs OS threads) wins for parallel scaling. implementing green threads in rust (forking rayon or using async) is out of scope. **61.4ms (within 17% of julia) is excellent** given rust's safety guarantees without GC.

#### larger sizes (comprehensive benchmark results)

all benchmarks run with RAYON_NUM_THREADS=20 on AMD Ryzen 9 7945HX:

| size | elements | proving (min/avg) | verification (min/avg) | runs |
|------|----------|-------------------|------------------------|------|
| 2^20 | 1.05M    | **61.4ms** / 63.4ms | **14.0ms** / 15.3ms | 10 |
| 2^24 | 16.8M    | **954ms** / 984ms | **261ms** / 273ms | 10 |
| 2^28 | 268.4M   | **17.6s** / 17.8s | **5.29s** / 5.40s | 3 |
| 2^30 | 1.07B    | **56.3s** / 56.7s | **8.16s** / 8.46s | 3 |

**note**: benchmarks run with optimized thread count (20 threads). default 32 threads shows ~15% slower due to rayon work-stealing overhead. see [threading overhead analysis](docs/threading_overhead_analysis.md) for details.

### prover timing breakdown (2^20)

detailed profiling shows where proving time is spent:

| component | time (ms) | % of total |
|-----------|-----------|------------|
| **FFT (reed-solomon encode)** | **35.1** | **79%** |
| poly to matrix | 2.9 | 6% |
| merkle tree construction | 1.9 | 4% |
| partial evaluation | 1.5 | 3% |
| recursive commitment | 2.2 | 5% |
| SHA256 row hashing | 0.7 | 2% |
| sumcheck (induce + rounds) | 0.3 | 1% |
| **total** | **~45ms** | **100%** |

**optimization status:** FFT butterfly operations now use SIMD (pclmulqdq for 2x parallel carryless multiplication). AVX-512 tested but provides no additional benefit - memory bandwidth is the bottleneck, not compute throughput.

run detailed profiling:
```bash
cargo run --release --example detailed_timing
```

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
