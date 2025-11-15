# PolkaVM Polynomial Commitment Verification (polkavm-pcvm)

Constraint system and proving infrastructure for PolkaVM execution traces using Ligerito polynomial commitments.

## Overview

This crate enables **cryptographic proving of PolkaVM execution** by:
1. Generating algebraic constraints from RISC-V instruction traces
2. Batching constraints via Schwartz-Zippel lemma
3. Proving via Ligerito polynomial commitments over GF(2³²)

**Key Innovation**: State continuity constraints that enable windowed proving for continuous execution.

## Architecture

```
PolkaVM Execution → Trace → Constraints → Polynomial → Ligerito Proof
                                                          (~101 KB, constant size!)
```

### Components

**Core Modules** (no PolkaVM dependency):
- `trace.rs` - Register-only execution traces
- `arithmetization.rs` - Polynomial encoding
- `constraints.rs` - Basic constraint system
- `poseidon.rs` - Cryptographic hash over GF(2³²)
- `memory.rs` - Memory abstraction
- `memory_merkle.rs` - Merkle tree for memory authentication

**PolkaVM Integration** (requires `polkavm-integration` feature):
- `polkavm_adapter.rs` - PolkaVM state representation
- `polkavm_tracer.rs` - Trace generation from PolkaVM execution
- `polkavm_constraints_v2.rs` - Complete constraint system
- `polkavm_arithmetization.rs` - Constraint batching
- `polkavm_prover.rs` - End-to-end proving pipeline

## Features

### Default Features
- `std` - Standard library support

### Optional Features
- `polkavm-integration` - Full PolkaVM constraint system

## Usage

### Basic (Register-Only)

```rust
use polkavm_pcvm::{RegisterOnlyTrace, arithmetize_register_trace};

// Execute program and generate trace
let trace = execute_and_trace(&program, initial_regs);

// Convert to polynomial
let polynomial = arithmetize_register_trace(&trace, &program, challenge);

// Prove with Ligerito
let proof = ligerito::prove(&polynomial, &config, transcript)?;
```

### Full PolkaVM Integration

```rust
use polkavm_pcvm::polkavm_prover::prove_polkavm_execution;
use polkavm_pcvm::polkavm_constraints_v2::ProvenTransition;

// Enable feature: polkavm-integration

// Execute PolkaVM and generate trace
let trace: Vec<(ProvenTransition, Instruction)> = ...;

// Generate proof
let proof = prove_polkavm_execution(
    &trace,
    program_commitment,
    batching_challenge,
    &prover_config,
    transcript,
)?;

// Proof is ~101 KB regardless of trace length!
```

## Constraint System

### Constraint Types

1. **Instruction Constraints** (per step):
   - Instruction decode (opcode in program Merkle tree)
   - ALU correctness (result matches operation)
   - Register consistency (unchanged registers stay same)

2. **State Continuity Constraints** (between steps):
   ```rust
   step[i].next_pc == step[i+1].pc
   step[i].regs_after == step[i+1].regs_before
   step[i].memory_root_after == step[i+1].memory_root_before
   ```

3. **Memory Constraints** (if accessed):
   - Merkle proof validity (read/write)
   - Root update correctness

### Batching

All ~29N constraints (for N steps) are batched into a single accumulator check via Schwartz-Zippel:

```rust
accumulator = Σ(r^i × constraint_i)

// If accumulator == 0: All constraints satisfied ✓
// If accumulator != 0: At least one failed ✗
```

## Performance

From Game of Life interactive demo (42 generations, 2688 steps):

```
Proving time:        340ms (constant!)
Verification time:   951μs (<1ms)
Proof size:          101 KB (constant!)
Constraint accumulator: 0 (all satisfied!)
```

## Continuous Execution

The constraint system supports **windowed proving** for continuous execution:

```rust
pub struct ContinuousExecutionProof {
    pub window_proof: PolkaVMProof,
    pub initial_state: StateCommitment,
    pub final_state: StateCommitment,
}

// Verify chain of windows
for i in 0..(proofs.len() - 1) {
    assert_eq!(proofs[i].final_state, proofs[i+1].initial_state);
}
```

This enables proving arbitrary-length executions by chaining proof windows.

## Dependencies

- `ligerito` - Polynomial commitment proving/verification
- `ligerito-binary-fields` - GF(2³²) arithmetic
- `ligerito-merkle` - Merkle tree commitments
- `polkavm` (optional) - PolkaVM execution engine
- `polkavm-common` (optional) - PolkaVM types

## Examples

See the main repository for complete examples:
- `examples/game-of-life/` - Interactive Game of Life with continuous execution
- `crates/ligerito/tests/game_of_life_interactive.rs` - Full windowed proving demo

## Development

### Building

```bash
# Basic build (register-only)
cargo build --release

# With PolkaVM integration
cargo build --release --features polkavm-integration

# Run tests
cargo test --features polkavm-integration
```

### Testing

The crate includes comprehensive tests:
- Unit tests for each module
- Integration tests for end-to-end proving
- Game of Life demo (42 generations, validated!)

## Architecture Document

For complete system architecture, see:
- `AGENTIC_PCVM_ARCHITECTURE.md` - Unified architecture document
- `BLOCKCHAIN_SPEC_1S.md` - 1-second block time design
- `examples/game-of-life/INTERACTIVE.md` - Demo walkthrough

## License

MIT OR Apache-2.0
