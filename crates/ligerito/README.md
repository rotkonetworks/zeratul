# Ligerito

polynomial commitment scheme over binary extension fields.

## what it's good for

- committing to large polynomials with small proofs (~150 KB for 2^20 polynomial)
- fast proving on modern cpus with simd (300-600ms for 1M elements)
- verifier-only builds for constrained environments (polkavm, wasm, embedded)
- transparent setup (no trusted setup required)
- enabling verifiable light client p2p networks

## what it's not good for

- general-purpose zkp (no arbitrary circuits, only polynomial commitments)
- proving without simd (slow without hardware acceleration)
- tiny polynomials (proof overhead significant below 2^12)
- scenarios requiring smallest possible proofs (starkware/plonky2 may be smaller)

## library usage

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

the workspace config automatically enables native cpu optimizations (SIMD/PCLMULQDQ) for 5-6x speedup.

**without optimizations (not recommended):**
```bash
cargo install ligerito  # will show performance warning
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

## supported sizes

- 2^12 (4,096 elements, 16 KB)
- 2^16 (65,536 elements, 256 KB)
- 2^20 (1,048,576 elements, 4 MB)
- 2^24 (16,777,216 elements, 64 MB)
- 2^28 (268,435,456 elements, 1 GB)
- 2^30 (1,073,741,824 elements, 4 GB)

## performance

benchmarked on amd ryzen 9 7945hx (8 cores, smt disabled):

| size | elements | proving | verification |
|------|----------|---------|--------------|
| 2^20 | 1.05m | 68ms | 22ms |
| 2^24 | 16.8m | 1.24s | 470ms |
| 2^28 | 268.4m | 25.1s | 8.5s |

## reference

[ligerito paper](https://angeris.github.io/papers/ligerito.pdf) by andrija novakovic and guillermo angeris

## license

mit / apache-2.0
