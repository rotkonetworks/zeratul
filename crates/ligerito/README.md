# Ligerito

polynomial commitment scheme over binary extension fields.

> **⚠️ IMPORTANT:** For optimal performance (3x speedup), install with native CPU optimizations:
> ```bash
> RUSTFLAGS="-C target-cpu=native" cargo install ligerito
> ```
> Without this flag, the prover will be significantly slower. See [installation](#installation) for details.

## what it's good for

- committing to large polynomials with small proofs (~150 KB for 2^20 polynomial)
- fast proving on modern cpus with simd (**50ms for 1M elements** on AVX-512, SMT off)
- tiered simd: AVX-512 → AVX2 → SSE → scalar fallback
- verifier-only builds for constrained environments (polkavm, wasm, embedded)
- transparent setup (no trusted setup required)
- enabling verifiable light client p2p networks

## what it's not good for

- general-purpose zkp (no arbitrary circuits, only polynomial commitments)
- proving without simd (slow without hardware acceleration)
- tiny polynomials (proof overhead significant below 2^12)
- scenarios requiring smallest possible proofs (starkware/plonky2 may be smaller)

## library usage

**add to Cargo.toml:**
```toml
[dependencies]
ligerito = "0.2.3"
```

**⚠️ for development:** clone the workspace to get automatic native optimizations:
```bash
git clone https://github.com/rotkonetworks/zeratul
cd zeratul
cargo build --release -p ligerito
```

**example:**
```rust
use ligerito::{prove, verify, hardcoded_config_20, hardcoded_config_20_verifier};
use ligerito_binary_fields::{BinaryElem32, BinaryElem128};
use std::marker::PhantomData;

// create prover config
let config = hardcoded_config_20(
    PhantomData::<BinaryElem32>,
    PhantomData::<BinaryElem128>,
);

// polynomial to commit (2^20 elements)
let poly: Vec<BinaryElem32> = vec![BinaryElem32::from(42); 1 << 20];

// generate proof
let proof = prove(&config, &poly).unwrap();

// verify proof
let verifier_config = hardcoded_config_20_verifier();
let valid = verify(&verifier_config, &proof).unwrap();
assert!(valid);
```

### transcript backends

```rust
// sha256 (default, no extra deps, works in no_std)
use ligerito::{prove_sha256, verify_sha256};
let proof = prove_sha256(&config, &poly).unwrap();
let valid = verify_sha256(&verifier_config, &proof).unwrap();

// merlin (requires std + transcript-merlin feature)
use ligerito::{prove, verify};
let proof = prove(&config, &poly).unwrap();
let valid = verify(&verifier_config, &proof).unwrap();
```

### supported sizes

configs available: `hardcoded_config_{12,16,20,24,28,30}` for prover and `hardcoded_config_{12,16,20,24,28,30}_verifier` for verifier.

## build configurations

### full-featured (default)

```bash
cargo build --release
```

includes: prover, verifier, parallelism, simd

### verifier-only

```bash
cargo build --release --no-default-features --features="std,verifier-only"
```

~50% smaller binary, perfect for polkavm/on-chain verification

### no_std verifier

```bash
cargo build --release --no-default-features --features="verifier-only"
```

minimal build for wasm/embedded (requires `alloc`)

## cli usage

### installation

**recommended (optimized for your cpu):**
```bash
# from crates.io
RUSTFLAGS="-C target-cpu=native" cargo install ligerito

# or from source
git clone https://github.com/rotkonetworks/zeratul
cd zeratul
cargo install --path crates/ligerito
```

the workspace config automatically enables native cpu optimizations.

**performance by simd tier (8 cores, SMT off):**
```
AVX-512 + VPCLMULQDQ:  ~50ms for 2^20 prove (fastest)
AVX2 + VPCLMULQDQ:     ~65ms for 2^20 prove
SSE + PCLMULQDQ:       ~95ms for 2^20 prove
No SIMD (scalar):      ~220ms for 2^20 prove
```

**important:** disable SMT (hyperthreading) for accurate benchmarks - it causes cache contention.

**without optimizations (not recommended):**
```bash
cargo install ligerito  # will show build warning about missing SIMD
```

**check your build:**
```bash
ligerito --version  # should show v0.2.4 or later
ligerito bench --size 20  # quick performance test (no I/O overhead)
# look for [release SIMD] in prove output for optimal performance
```

### prove and verify

```bash
# generate random test data (2^20 = 1M elements)
ligerito generate --size 20 --pattern random > poly.bin

# generate proof from polynomial data
cat poly.bin | ligerito prove --size 20 > proof.bin

# verify proof
cat proof.bin | ligerito verify --size 20
# output: "VALID" with exit code 0

# roundtrip test
cat poly.bin | ligerito prove --size 20 | ligerito verify --size 20
```

### transcript backends

prover and verifier must use the same transcript backend:

```bash
# sha256 (default)
ligerito prove --size 20 --transcript sha256 < poly.bin > proof.bin
ligerito verify --size 20 --transcript sha256 < proof.bin

# merlin (requires transcript-merlin feature)
ligerito prove --size 20 --transcript merlin < poly.bin > proof.bin
ligerito verify --size 20 --transcript merlin < proof.bin
```

### generate test data

```bash
# random data (default)
ligerito generate --size 20 --pattern random > poly.bin

# all zeros
ligerito generate --size 20 --pattern zeros > poly.bin

# all ones
ligerito generate --size 20 --pattern ones > poly.bin

# sequential (0, 1, 2, ...)
ligerito generate --size 20 --pattern sequential > poly.bin

# save to file
ligerito generate --size 20 --pattern random --output test.bin
```

### benchmark (no I/O overhead)

```bash
# prove only
ligerito bench --size 24

# prove + verify
ligerito bench --size 24 --verify

# multiple iterations
ligerito bench --size 24 --verify --iterations 10

# with specific thread count
RAYON_NUM_THREADS=16 ligerito bench --size 24
```

### show configuration

```bash
ligerito config --size 20
```

### data format

polynomials are binary data: `size * 4` bytes (4 bytes per `BinaryElem32` element).

example for 2^12:
```bash
# 2^12 elements = 4096 elements * 4 bytes = 16384 bytes
dd if=/dev/urandom of=test.bin bs=16384 count=1
cat test.bin | ligerito prove --size 12 | ligerito verify --size 12
```

## features

- `std` (default): standard library support
- `prover` (default): include proving functionality
- `verifier-only`: minimal verifier build
- `parallel` (default): multi-threaded with rayon
- `hardware-accel` (default): simd acceleration
- `transcript-sha256`: sha256 transcript (always available)
- `transcript-merlin`: merlin transcript (requires std)
- `cli`: command-line binary
- `wasm`: browser support with wasm-bindgen
- `wasm-parallel`: multi-threaded WASM via Web Workers (requires SharedArrayBuffer)

## wasm usage

```bash
# build WASM with SIMD128 (recommended)
RUSTFLAGS='-C target-feature=+simd128' \
  cargo build --target wasm32-unknown-unknown --features wasm --no-default-features

# with parallel support (requires SharedArrayBuffer + CORS headers)
RUSTFLAGS='-C target-feature=+simd128,+atomics,+bulk-memory,+mutable-globals' \
  cargo build --target wasm32-unknown-unknown --features wasm-parallel --no-default-features
```

wasm simd128 optimizations:
- `i8x16_swizzle` for parallel 16-way table lookups
- `v128_xor` for SIMD GF(2) additions
- 4-bit lookup table with Karatsuba decomposition

## supported sizes

- 2^12 (4,096 elements, 16 KB)
- 2^16 (65,536 elements, 256 KB)
- 2^20 (1,048,576 elements, 4 MB)
- 2^24 (16,777,216 elements, 64 MB)
- 2^28 (268,435,456 elements, 1 GB)
- 2^30 (1,073,741,824 elements, 4 GB)

## performance

### benchmark results

benchmarked on amd ryzen 9 7945hx (16 cores / 32 threads, AVX-512 + VPCLMULQDQ):

| size | elements | prove time | verify time | proof size | throughput |
|------|----------|------------|-------------|------------|------------|
| 2^20 | 1.05m | **41ms** | 5ms | 149 KB | 25.6m elem/s |
| 2^24 | 16.8m | **570ms** | 29ms | 245 KB | 29.4m elem/s |
| 2^28 | 268.4m | ~9s | ~400ms | ~2.4 MB | 29.8m elem/s |

use the built-in benchmark command (no I/O overhead):
```bash
ligerito bench --size 24 --verify --iterations 5
```

### parallel scaling analysis

the prover is **memory-bandwidth bound**, not compute-bound. scaling efficiency drops significantly beyond 8 cores:

| threads | 2^24 prove | speedup | efficiency |
|---------|------------|---------|------------|
| 1 | 4339ms | 1.0x | 100% |
| 2 | 2301ms | 1.9x | 94% |
| 4 | 1251ms | 3.5x | 87% |
| 8 | 780ms | 5.6x | 70% |
| 16 | 617ms | 7.0x | 44% |
| 32 | 570ms | 7.6x | 24% |

**why scaling stops at ~8 cores:**

1. **memory bandwidth saturation**: 2^24 elements = 64MB polynomial. the FFT reads/writes this data multiple times per level. DDR5 bandwidth (~100GB/s) becomes the bottleneck, not CPU cycles.

2. **L3 cache pressure**: ryzen 7945hx has 64MB L3 (32MB per CCD). the polynomial barely fits in cache. with >8 cores, cross-CCD traffic over infinity fabric adds latency.

3. **amdahl's law**: FFT has log2(n) = 24 levels. only some levels run in parallel. merkle tree construction and transcript hashing have limited parallelism.

4. **diminishing returns**: 7.6x speedup on 32 threads (vs theoretical 32x) is actually good for memory-bound workloads. typical memory-bound algorithms achieve 4-8x on many-core systems.

### simd tier comparison

fft butterfly performance (2^24 elements, single iteration):

| tier | instruction | elements/op | time | vs SSE |
|------|-------------|-------------|------|--------|
| AVX-512 | vpclmulqdq zmm | 8 | 17ms | 2.6x |
| AVX2 | vpclmulqdq ymm | 4 | 25ms | 1.8x |
| SSE | pclmulqdq xmm | 2 | 45ms | baseline |

runtime detection automatically selects the best available tier.

### cli vs library performance

the CLI has ~2x overhead when using pipes due to I/O:

| method | 2^24 prove |
|--------|------------|
| `ligerito bench --size 24` | 570ms |
| `generate \| prove \| verify` | ~1200ms |

the overhead comes from:
- piping 64MB through stdin/stdout
- random number generation in `generate`
- proof serialization/deserialization

for benchmarking, always use `ligerito bench` which measures pure algorithm performance.

### optimal configuration

```bash
# best performance: all threads, SMT on
RAYON_NUM_THREADS=32 ligerito bench --size 24

# consistent benchmarks: physical cores only, SMT off
echo "off" | sudo tee /sys/devices/system/cpu/smt/control
RAYON_NUM_THREADS=16 taskset -c 0-15 ligerito bench --size 24
echo "on" | sudo tee /sys/devices/system/cpu/smt/control

# single CCD (lower latency, less throughput)
RAYON_NUM_THREADS=8 taskset -c 0-7 ligerito bench --size 24
```

### improving performance further

current bottlenecks and potential solutions:

| bottleneck | solution | expected gain |
|------------|----------|---------------|
| memory bandwidth | GPU acceleration (HBM) | 5-10x |
| FFT memory access | cache-blocked FFT | 1.2-1.5x |
| field element size | GF(2^16) instead of GF(2^32) | 1.5-2x |
| cross-CCD latency | NUMA-aware allocation | 1.1-1.2x |

## reference

[ligerito paper](https://angeris.github.io/papers/ligerito.pdf) by andrija novakovic and guillermo angeris

## license

mit / apache-2.0
