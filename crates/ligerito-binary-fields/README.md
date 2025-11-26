# ligerito-binary-fields

binary extension field arithmetic (GF(2^n)) for polynomial commitment schemes.

## features

- `std` (default): standard library support
- `hardware-accel`: tiered simd acceleration with runtime detection
- `serde`: serialization support

## simd tiers

with `hardware-accel` feature enabled, automatically selects best available:

### x86_64

| tier | instruction | elements/iter | speedup |
|------|-------------|---------------|---------|
| AVX-512 | VPCLMULQDQ (512-bit) | 8 | 1.9x |
| AVX2 | VPCLMULQDQ (256-bit) | 4 | 1.5x |
| SSE | PCLMULQDQ (128-bit) | 2 | baseline |
| scalar | lookup table + karatsuba | 1 | fallback |

runtime detection via `is_x86_feature_detected!` - no recompilation needed.

### wasm32-simd128

| technique | description |
|-----------|-------------|
| `i8x16_swizzle` | parallel 16-way table lookups for carryless multiply |
| `v128_xor` | SIMD XOR for GF(2) additions |
| 4-bit LUT | 256-byte lookup table with Karatsuba decomposition |

compile with: `RUSTFLAGS='-C target-feature=+simd128' cargo build --target wasm32-unknown-unknown`

**note:** disable SMT for accurate benchmarks (hyperthreading causes cache contention).

## usage

```rust
use ligerito_binary_fields::{BinaryElem32, BinaryElem128, BinaryFieldElement};

let a = BinaryElem32::from(42);
let b = BinaryElem32::from(17);
let c = a.mul(&b);
```

## reference

part of the [ligerito](https://crates.io/crates/ligerito) polynomial commitment scheme implementation.

## license

mit / apache-2.0
