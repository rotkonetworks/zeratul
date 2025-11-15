# ligerito-binary-fields

binary extension field arithmetic (GF(2^n)) for polynomial commitment schemes.

## features

- `std` (default): standard library support
- `hardware-accel` (default): simd acceleration (x86 pclmulqdq, wasm simd128)
- `serde`: serialization support

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
