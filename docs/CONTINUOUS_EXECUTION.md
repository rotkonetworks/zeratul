# Continuous Execution Analysis

## Current Design

Our implementation correctly verifies:
- ✅ Each step executes its instruction correctly
- ✅ PC continuity within each step (pc → next_pc)
- ✅ Register updates match instruction semantics
- ✅ Memory Merkle proofs authenticate loads/stores
- ✅ Batched constraint verification

## The Gap: State Continuity Between Steps

**What's missing:**

We don't verify that `step[i].regs_after == step[i+1].regs_before`!

### Current Behavior

```rust
// Step i
transition_i = {
    regs_before: [0, 0, 0, ...],
    regs_after: [1, 2, 3, ...],  // After executing instruction
    ...
}

// Step i+1
transition_i+1 = {
    regs_before: [99, 88, 77, ...],  // ⚠️ COULD BE ANYTHING!
    regs_after: [...],
    ...
}
```

A malicious prover could:
1. Execute step i correctly → get regs_after = [1, 2, 3]
2. Claim step i+1 starts with regs_before = [999, 888, 777]
3. Execute step i+1 "correctly" from that forged state
4. Both steps verify individually ✓
5. But the execution chain is broken! ✗

### Why This Matters for Continuous Execution

In the graypaper model:
- PVM executes continuously (millions of steps)
- We batch proofs over windows (e.g., 40k steps)
- Each proof must **chain** to the next
- `proof[i].final_state == proof[i+1].initial_state`

## The Fix: Add State Continuity Constraints

### Option 1: Explicit Continuity Constraints (Recommended)

Add to `polkavm_arithmetization.rs`:

```rust
pub fn verify_trace_continuity(
    trace: &[(ProvenTransition, Instruction)],
) -> Result<Vec<BinaryElem32>, &'static str> {
    let mut constraints = Vec::new();

    for i in 0..(trace.len() - 1) {
        let current = &trace[i].0;
        let next = &trace[i+1].0;

        // Verify: current.regs_after == next.regs_before
        for reg in 0..13 {
            let current_after = current.regs_after.to_array()[reg];
            let next_before = next.regs_before.to_array()[reg];

            // XOR = 0 iff equal in GF(2^32)
            let constraint = BinaryElem32::from(current_after ^ next_before);
            constraints.push(constraint);
        }

        // Verify: current.memory_root_after == next.memory_root_before
        for byte_idx in 0..32 {
            let current_root = current.memory_root_after[byte_idx];
            let next_root = next.memory_root_before[byte_idx];
            let constraint = BinaryElem32::from(current_root ^ next_root);
            constraints.push(constraint);
        }

        // Verify: current.next_pc == next.pc
        let constraint = BinaryElem32::from(current.next_pc ^ next.pc);
        constraints.push(constraint);
    }

    Ok(constraints)
}
```

### Option 2: Implicit via Arithmetization

Encode state as polynomial columns:
```
Row i:   [PC_i, regs_i[0], regs_i[1], ..., mem_root_i]
Row i+1: [PC_{i+1}, regs_{i+1}[0], regs_{i+1}[1], ..., mem_root_{i+1}]
```

Add copy constraints in the polynomial:
```rust
// For each row i < n-1:
// polynomial[i, "regs_after"] == polynomial[i+1, "regs_before"]
```

## Implementation Plan

1. **Add state continuity constraints** to `compute_batched_constraints`
2. **Test with forged continuity** - create step with wrong initial state
3. **Verify it's rejected** - constraint accumulator should be non-zero
4. **Batch into existing accumulator** - use same Schwartz-Zippel approach

## For Production Continuous Execution

### Windowed Proving

```rust
pub struct ContinuousExecutionProof {
    /// Proof for steps [start, end)
    pub window_proof: PolkaVMProof,

    /// Initial state (binds to previous window)
    pub initial_state: StateCommitment,

    /// Final state (binds to next window)
    pub final_state: StateCommitment,
}

pub fn verify_execution_chain(
    proofs: &[ContinuousExecutionProof]
) -> bool {
    // Verify each window
    for proof in proofs {
        if !verify_polkavm_proof(&proof.window_proof, ...) {
            return false;
        }
    }

    // Verify continuity between windows
    for i in 0..(proofs.len() - 1) {
        if proofs[i].final_state != proofs[i+1].initial_state {
            return false;  // BROKEN CHAIN
        }
    }

    true
}
```

### StateCommitment

```rust
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StateCommitment {
    /// All 13 registers
    pub registers: [u32; 13],

    /// Memory Merkle root
    pub memory_root: [u8; 32],

    /// Program counter
    pub pc: u32,
}

impl StateCommitment {
    pub fn hash(&self) -> [u8; 32] {
        // Poseidon hash of all state
        // This becomes the "binding" between windows
    }
}
```

## Answer to Your Question

**Can our design proceed for continuous execution?**

**Yes, but we need to add state continuity constraints.**

Current status:
- ✅ Single-step verification: SOUND
- ✅ Batched verification: IMPLEMENTED
- ✅ Memory authentication: WORKING
- ⚠️ **Multi-step continuity: MISSING**
- ⚠️ **Window chaining: NOT IMPLEMENTED**

With the fixes above (which are straightforward), we get:
- ✅ Continuous execution over arbitrary trace lengths
- ✅ Windowed proving with state commitments
- ✅ Chain verification for long-running PVM

The architecture is **exactly right** for continuous execution - we just need to add the continuity constraints to make it sound.
