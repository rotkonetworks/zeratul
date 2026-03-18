# WIM (Witness Indistinguishable Machine)

PolkaVM execution proofs via Ligerito polynomial commitments.

## overview

WIM proves correct execution of PolkaVM (RISC-V) programs by:
1. generating algebraic constraints from instruction traces
2. batching constraints via Schwartz-Zippel (4 challenges, soundness n^4/2^128)
3. proving via Ligerito polynomial commitments over GF(2^32)

state continuity constraints enable windowed proving for continuous execution.

## architecture

```
PolkaVM Execution → Trace → Constraints → Polynomial → Ligerito Proof
                                                         (~101 KB, constant size)
```

### core modules (no PolkaVM dependency)

- `trace.rs` - register-only execution traces
- `arithmetization.rs` - polynomial encoding with Rescue-Prime program hashing
- `constraints.rs` - basic constraint system
- `evaluation_proof.rs` - column evaluation verification against ligerito yr output
- `rescue.rs` - Rescue-Prime hash (x^(-1) sbox, SHAKE-256 round constants, verified MDS)
- `merkle128.rs` - Merkle tree using Rescue-Prime over GF(2^128)
- `unified_memory128.rs` - authenticated memory with 128-bit merkle proofs
- `memory.rs` - memory abstraction
- `sumcheck.rs` - sumcheck protocol
- `trace_opening.rs` - trace point opening proofs

### PolkaVM integration (requires `polkavm-integration` feature)

- `polkavm_adapter.rs` - PolkaVM state representation
- `polkavm_tracer.rs` - trace generation from PolkaVM execution
- `polkavm_constraints.rs` - complete constraint system
- `polkavm_arithmetization.rs` - constraint batching

### deprecated (insecure, do not use)

- `poseidon.rs` - x^5 sbox is not a permutation in binary fields
- `memory_merkle.rs` - uses insecure poseidon
- `unified_memory.rs` - uses insecure memory_merkle

## security

all hashing and constraint accumulation operates in GF(2^128) for 64-bit collision resistance. trace values are embedded from GF(2^32).

batched Schwartz-Zippel uses 4 independent challenges in GF(2^32), giving soundness error ≤ (n/2^32)^4 ≈ n^4/2^128.

## dependencies

all from crates.io:

- `ligerito` 0.6 - polynomial commitment proving/verification
- `ligerito-binary-fields` 0.6 - GF(2^32) and GF(2^128) arithmetic
- `ligerito-merkle` 0.6 - merkle tree commitments
- `polkavm` (optional) - PolkaVM execution engine

## usage

```rust
use wim::{RegisterOnlyTrace, arithmetize_register_trace, execute_and_trace};

// execute program and generate trace
let trace = execute_and_trace(&program, initial_regs);

// convert to polynomial with Rescue-Prime program hash
let result = arithmetize_register_trace(&trace, &program, challenges);

// prove with ligerito
let proof = ligerito::prove(&result.polynomial, &config, transcript)?;
```

## building

```bash
cargo build --release
cargo build --release --features polkavm-integration
cargo test
```

## license

MIT OR Apache-2.0
