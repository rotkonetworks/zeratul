# Zeratul

Zero-overhead blockchain using AccidentalComputer pattern with Ligerito for "ZK"
proofs.

## performance

benchmarked on amd ryzen 9 7945hx (8 physical cores, SMT off, turbo off):

### julia vs zeratul comparison

| size | elements | julia proving | zeratul proving | speedup |
|------|----------|---------------|-----------------|---------|
| 2^20 | 1.05M    | 87ms | **50ms** | **1.7x faster** |
| 2^24 | 16.8M    | 1.17s | 650ms | **1.8x faster** |
| 2^28 | 268.4M   | 18s | 10s | **1.8x faster** |

**simd tier performance (fft butterfly, 2^20):**

| tier | elements/iter | time | vs SSE |
|------|---------------|------|--------|
| AVX-512 | 8 | 0.96ms | 1.9x |
| AVX2 | 4 | 1.25ms | 1.5x |
| SSE | 2 | 1.86ms | baseline |

### optimization highlights

**tiered simd with runtime detection:**
- AVX-512 VPCLMULQDQ: 8 elements/iteration (512-bit carryless multiply)
- AVX2 VPCLMULQDQ: 4 elements/iteration (256-bit carryless multiply)
- SSE PCLMULQDQ: 2 elements/iteration (128-bit carryless multiply)
- lookup table fallback for non-SIMD (4-bit × 4-bit LUT with Karatsuba)

**other optimizations:**
- O(1) index extraction replacing O(n) linear search
- batch row hashing (entire row at once vs element-by-element)
- eliminated nested parallelization overhead

### benchmarking setup

**disable SMT for accurate benchmarks** (hyperthreading causes cache contention):
```bash
# disable SMT
echo "off" | sudo tee /sys/devices/system/cpu/smt/control

# run with 8 physical cores
RAYON_NUM_THREADS=8 taskset -c 0-7 cargo run --release --example quick_bench

# restore SMT
echo "on" | sudo tee /sys/devices/system/cpu/smt/control
```

## building

**critical: for simd performance, you must build with native cpu target:**

```bash
RUSTFLAGS="-C target-cpu=native" cargo build --release
```

or add to `.cargo/config.toml`:
```toml
[build]
rustflags = ["-C", "target-cpu=native"]
```

without this flag, simd instructions (pclmulqdq) won't be used and performance will be significantly worse.

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
