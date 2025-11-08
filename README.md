# zeratul

rust implementation of [ligerito](https://angeris.github.io/papers/ligerito.pdf) polynomial commitment scheme over binary extension fields.

## structure

- `binary-fields/` - gf(2^n) arithmetic with simd operations
- `reed-solomon/` - parallel fft-based encoding over binary fields
- `merkle-tree/` - sha256 commitment trees
- `ligerito/` - sumcheck-based polynomial commitments

## performance

benchmarked on amd ryzen 9 7945hx (8 cores, performance governor, turbo disabled):

### julia vs zeratul comparison

all benchmarks use identical parameters (sha256 transcript, 8 cores):

| size | elements | julia proving | julia verify | zeratul proving | zeratul verify | proving ratio | verify ratio |
|------|----------|---------------|--------------|-----------------|----------------|---------------|--------------|
| 2^20 | 1.05M    | TBD ms | TBD ms | TBD ms | TBD ms | TBD x | TBD x |
| 2^24 | 16.8M    | TBD ms | TBD ms | TBD ms | TBD ms | TBD x | TBD x |
| 2^28 | 268.4M   | TBD s  | TBD ms | TBD s  | TBD ms | TBD x | TBD x |
| 2^30 | 1.07B    | TBD s  | TBD ms | TBD s  | TBD ms | TBD x | TBD x |

**note**: benchmarks run with cpu governor tuning (performance mode, turbo boost disabled, pinned to cores 1-8). run `./benchmarks/compare_julia_rust.sh` to reproduce.

### optimizations

**monomorphic simd fft (35% faster):**
- specialized gf(2^32) fft using direct sse pclmulqdq calls
- eliminated generic dispatch overhead via typeid-based specialization
- 2x parallel carryless multiplication in butterfly operations
- before: 91.9ms → after: 61.4ms (2^20, 20 threads)

**threading improvements:**
- eliminated nested parallelization (sequential fft within parallel column encoding)
- reduced rayon task spawning overhead (min_parallel_size: 16384 elements)
- tuned thread count for minimal work-stealing coordination

**single-threaded comparison:**
- rust: 367.7ms (simd fft)
- julia: 393.9ms (jit-compiled)
- **rust is 7% faster single-threaded**

**why julia scales better:**

profiling revealed 61% of rust multi-threaded runtime is rayon coordination overhead (crossbeam-epoch, work-stealing deques, task migration). julia's green threads are 10x cheaper to create (~5-20ns vs 50-200ns) and stay on the same thread (better cache locality).

the remaining gap is architectural:
- rayon: os threads + work-stealing (heavy)
- julia: m:n green threads (lightweight, cooperative scheduling)

implementing green threads in rust would require forking rayon or switching to async (out of scope). current performance is excellent given rust's safety guarantees without gc.

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

configure passwordless sudo for cpu governor tuning:
```bash
./benchmarks/setup_sudo.sh
```

this allows the benchmark scripts to set performance governor and disable turbo boost for consistent measurements.

### run benchmarks

run tuned benchmarks (8 cores, performance governor, turbo disabled):
```bash
./benchmarks/run_tuned_benchmarks.sh
```

compare with julia:
```bash
./benchmarks/compare_julia_rust.sh
```

## testing

```bash
cargo test --release
```

## license

mit / apache-2.0
