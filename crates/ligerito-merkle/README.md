# ligerito-merkle

sha256 merkle tree implementation for polynomial commitment schemes.

## features

- `std` (default): standard library support
- `parallel` (default): multi-threaded tree construction with rayon
- `serde`: serialization support

## usage

```rust
use ligerito_merkle::MerkleTree;

let data = vec![/* your data */];
let tree = MerkleTree::new(&data);
let root = tree.root();
```

## reference

part of the [ligerito](https://crates.io/crates/ligerito) polynomial commitment scheme implementation.

## license

mit / apache-2.0
