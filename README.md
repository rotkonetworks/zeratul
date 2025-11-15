# Zeratul

Zero-overhead blockchain using AccidentalComputer pattern with Ligerito for "ZK"
proofs.

## performance

benchmarked on amd ryzen 9 7945hx (8 physical cores, smt disabled, turbo disabled, performance governor):

### julia vs zeratul comparison

| size | elements | julia proving | julia verify | zeratul proving | zeratul verify | proving ratio | verify ratio |
|------|----------|---------------|--------------|-----------------|----------------|---------------|--------------|
| 2^20 | 1.05M    | 90.65ms | 16.55ms | **68.31ms** | 22.48ms | **0.75x** ✓ | 1.35x |
| 2^24 | 16.8M    | 1173.71ms | 127.24ms | **1238.83ms** | 470.45ms | **1.05x** | 3.69x |
| 2^28 | 268.4M   | 18.08s | 2.07s | 25.09s | 8.50s | 1.38x | 4.10x |
| 2^30 | 1.07B    | 71.11s | 4.14s | 77.34s | 14.46s | 1.08x | 3.49x |

**results:**
- 2^20: rust 25% faster (68.31ms vs 90.65ms)
- 2^24: roughly equal (1238.83ms vs 1173.71ms, 5% slower)
- 2^28/2^30: julia faster (8-38% slower)

single-threaded baseline (2^20):
- rust simd fft: 334.79ms
- julia jit: 401.0ms
- rust 20% faster single-threaded

multi-threaded scaling with 8 physical cores (smt disabled):
- rust: 334.79ms → 68.31ms (4.9x speedup)
- julia: 401.0ms → 90.65ms (4.42x speedup)

rust's monomorphic simd fft (direct sse pclmulqdq) is faster than julia's jit
both single and multi-threaded at 2^20, but julia wins at larger inputs(green
threads goated?).

### optimization highlights

**monomorphic simd fft:**
- specialized gf(2^32) fft using direct sse pclmulqdq calls
- eliminated generic dispatch overhead via typeid-based specialization
- 2x parallel carryless multiplication in butterfly operations

**threading:**
- eliminated nested parallelization (sequential fft within parallel column encoding)
- tuned for 8 physical cores without smt
- min_parallel_size: 16384 elements to reduce task spawning overhead

**critical: smt (hyperthreading) must be disabled**
- with smt on: 2^20 proving = 138.64ms (terrible - cache/resource contention)
- with smt off: 2^20 proving = 68.31ms (proper scaling!)
- **smt doubles latency due to execution unit/cache sharing**

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

// polynomial to commit
let poly: Vec<BinaryElem32> = (0..1048576)
    .map(|i| BinaryElem32::from(i as u32))
    .collect();

// generate proof
let proof = prove(&config, &poly).unwrap();

// verify
let verifier_config = hardcoded_config_20_verifier();
let verified = verify(&verifier_config, &proof).unwrap();
assert!(verified);
```

## benchmarking

### setup (one-time)

configure passwordless sudo for cpu tuning:
```bash
./benchmarks/setup_sudo.sh
```

this allows benchmarks to disable smt, set performance governor, and disable turbo boost.

### run benchmarks

**important:** benchmarks require smt disabled for accurate results:

```bash
# complete benchmark suite (all sizes)
./benchmarks/run_proper_tuned.sh

# compare with julia
./benchmarks/compare_proper_tuned.sh
```

benchmarks automatically:
- disable smt (hyperthreading)
- disable turbo boost (consistent clocks)
- set performance governor
- pin to physical cores 0-7
- restore original state on exit

## testing

```bash
cargo test --release
```

## license

mit / apache-2.0
