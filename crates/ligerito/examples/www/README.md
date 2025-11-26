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
ligerito --version  # should show v0.2.3 or later
ligerito generate --size 20 | ligerito prove --size 20 2>&1 | grep "SIMD"
# output should show: [release SIMD] for optimal performance
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

benchmarked on amd ryzen 9 7945hx (8 physical cores, SMT off, turbo off):

| size | elements | proving | proof size |
|------|----------|---------|------------|
| 2^20 | 1.05m | **50ms** | 149 KB |
| 2^24 | 16.8m | 650ms | 2.4 MB |
| 2^28 | 268.4m | 10s | 38 MB |

**simd tier comparison (fft butterfly, 2^20 elements):**

| tier | elements/iter | time | speedup |
|------|---------------|------|---------|
| AVX-512 | 8 | 0.96ms | 1.9x |
| AVX2 | 4 | 1.25ms | 1.5x |
| SSE | 2 | 1.86ms | baseline |

**benchmarking setup:**
```bash
# disable SMT (hyperthreading causes cache contention)
echo "off" | sudo tee /sys/devices/system/cpu/smt/control

# run benchmark
RAYON_NUM_THREADS=8 taskset -c 0-7 cargo run --release --example quick_bench

# restore SMT
echo "on" | sudo tee /sys/devices/system/cpu/smt/control
```

## reference

[ligerito paper](https://angeris.github.io/papers/ligerito.pdf) by andrija novakovic and guillermo angeris

## license

mit / apache-2.0
