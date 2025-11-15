# ligerito-reed-solomon

reed-solomon erasure coding over binary fields using fft-based encoding.

## features

- `std` (default): standard library support
- `parallel` (default): multi-threaded fft with rayon
- `hardware-accel` (default): simd acceleration for field operations

## usage

```rust
use ligerito_reed_solomon::encode;
use ligerito_binary_fields::BinaryElem32;

let data: Vec<BinaryElem32> = vec![/* your polynomial coefficients */];
let encoded = encode(&data, rate);
```

## reference

part of the [ligerito](https://crates.io/crates/ligerito) polynomial commitment scheme implementation.

## license

mit / apache-2.0
