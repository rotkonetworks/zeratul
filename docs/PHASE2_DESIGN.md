# Phase 2: Read-Only Memory Design

## Overview

Extend pcVM to support **read-only memory** for fetching program instructions and constant data.

**Key Constraint**: Memory is **immutable** - no writes, only reads.

## Goals

1. ✅ Load program binary from memory
2. ✅ Verify each instruction fetch reads correct opcode
3. ✅ Support immediate loads from memory (constants)
4. ❌ No writes (Phase 3)
5. ❌ No permutation argument needed (read-only = no consistency issues)

## Architecture

### Memory Model

```rust
pub struct ReadOnlyMemory {
    /// Memory contents (immutable after initialization)
    pub data: Vec<u32>,

    /// Program hash (commitment to memory contents)
    pub hash: BinaryElem32,
}
```

**Properties**:
- Fixed at trace generation time
- Hashed with Poseidon for integrity
- Maximum size: 2^16 words (256 KB) for Phase 2

### Instruction Set Extension

**New Opcodes**:
```rust
pub enum Opcode {
    // Phase 1 (existing)
    ADD, SUB, MUL, AND, OR, XOR, SLL, SRL, LI, HALT,

    // Phase 2 (new)
    LOAD,  // rd = mem[rs1]  - Load word from memory
}
```

**Encoding**:
```
LOAD rd, offset(rs1)
→ rd = memory[rs1 + offset]
```

### Trace Extension

```rust
pub struct MemoryEnabledStep {
    // Phase 1 fields (unchanged)
    pub pc: u32,
    pub regs: [u32; 13],
    pub opcode: Opcode,
    pub rd: u8,
    pub rs1: u8,
    pub rs2: u8,
    pub imm: u32,

    // Phase 2 fields (new)
    pub memory_address: Option<u32>,  // Address read (if LOAD)
    pub memory_value: Option<u32>,    // Value read (if LOAD)
}
```

### Constraint Extensions

**New Constraints**:

1. **Memory Fetch Correctness** (for LOAD instruction):
   ```rust
   // Constraint: value read from memory matches expected value
   let addr = step.regs[step.rs1 as usize].wrapping_add(step.imm);
   let expected_value = memory.data[addr as usize];
   let actual_value = step.memory_value.unwrap();

   constraint = expected_value ⊕ actual_value  // Must be 0
   ```

2. **Program Code Fetch** (for all instructions):
   ```rust
   // Constraint: instruction at PC matches memory[PC]
   let expected_instr = memory.data[step.pc as usize];
   let actual_opcode = step.opcode as u32;

   // Extract opcode from instruction encoding
   constraint = (expected_instr & 0xFF) ⊕ actual_opcode
   ```

3. **Memory Bounds Check**:
   ```rust
   // Constraint: address is within bounds
   let addr = step.memory_address.unwrap_or(0);
   let in_bounds = addr < memory.data.len() as u32;

   constraint = if in_bounds { 0 } else { 1 }
   ```

### Arithmetization Extension

**Polynomial Structure**:
```
[ memory_hash,           // Poseidon(entire memory)
  memory_size,           // Number of words in memory
  program_hash,          // Separate hash of program portion
  num_steps,

  // For each step:
  step_i_pc,
  step_i_opcode,
  step_i_rd/rs1/rs2,
  step_i_imm,
  step_i_regs[0..12],
  step_i_memory_addr,    // NEW: memory address accessed (0 if none)
  step_i_memory_value,   // NEW: value read from memory (0 if none)

  final_regs[0..12],
  constraint_product ]
```

**Size Impact**:
- Phase 1: ~20 elements per step
- Phase 2: ~22 elements per step (+2 for memory fields)

### Memory Hash Computation

Use Poseidon to hash memory contents in chunks:

```rust
pub fn hash_memory(memory: &[u32]) -> BinaryElem32 {
    let chunks: Vec<Vec<BinaryElem32>> = memory
        .chunks(256)  // Process 256 words at a time
        .map(|chunk| chunk.iter().map(|&w| BinaryElem32::from(w)).collect())
        .collect();

    let mut hash = BinaryElem32::zero();
    for chunk in chunks {
        let chunk_hash = PoseidonHash::hash_elements(&chunk);
        hash = hash.add(&chunk_hash);  // Combine hashes
    }
    hash
}
```

## Example Programs

### Example 1: Load Constant
```rust
// Memory layout:
// 0x0000: Program code
// 0x1000: Constant data (value = 42)

let program = vec![
    Instruction::new_imm(1, 0x1000),        // a1 = 0x1000 (data address)
    Instruction::new_load(0, 1, 0),         // a0 = mem[a1 + 0] = 42
    Instruction::halt(),
];

let mut memory = vec![0u32; 0x2000];
memory[0x1000] = 42;  // Constant data
```

### Example 2: Fetch Instruction from Memory
```rust
// Self-modifying-like behavior (but read-only!)
// Read what the next instruction would be

let program = vec![
    Instruction::new_imm(1, 1),             // a1 = 1 (next PC)
    Instruction::new_load(0, 1, 0),         // a0 = mem[1] = next instruction
    Instruction::halt(),
];
```

### Example 3: Array Sum (Read-Only)
```rust
// Sum array of constants from memory
let program = vec![
    Instruction::new_imm(0, 0),             // a0 = 0 (sum)
    Instruction::new_imm(1, 0x1000),        // a1 = 0x1000 (array base)
    Instruction::new_imm(2, 10),            // a2 = 10 (array length)
    Instruction::new_imm(3, 0),             // a3 = 0 (index)

    // Loop (unrolled for now, no branches yet):
    Instruction::new_load(4, 1, 0),         // a4 = mem[a1]
    Instruction::new_rrr(Opcode::ADD, 0, 0, 4),  // sum += a4
    Instruction::new_imm(5, 4),             // a5 = 4 (word size)
    Instruction::new_rrr(Opcode::ADD, 1, 1, 5),  // a1 += 4 (next element)
    // ... repeat for each element ...

    Instruction::halt(),
];

// Memory contains array data at 0x1000
```

## Verification Strategy

### Prover Side
1. Execute program with memory
2. Record all memory accesses (address, value)
3. Hash memory contents
4. Encode memory accesses in polynomial
5. Generate Ligerito proof

### Verifier Side
1. Receive proof with memory hash
2. Trust that memory hash is correct (or verify separately)
3. Check constraints ensure:
   - Each LOAD reads from correct address
   - Value read matches memory[address]
   - Program code fetches match memory contents

**Key Insight**: Verifier doesn't need full memory, just:
- Memory hash
- Proof that accesses are consistent with that hash

## Implementation Plan

### Step 1: Extend Trace Structure
- Add `memory_address` and `memory_value` fields
- Add `ReadOnlyMemory` struct
- Update `execute_and_trace` to track memory accesses

### Step 2: Add LOAD Opcode
- Implement LOAD execution
- Update opcode enum
- Add instruction encoding/decoding

### Step 3: Memory Constraints
- Memory fetch correctness
- Code fetch verification
- Bounds checking

### Step 4: Arithmetization Update
- Include memory hash in polynomial
- Encode memory accesses
- Update constraint product calculation

### Step 5: Testing
- Unit tests for LOAD instruction
- Memory hash collision resistance
- Constraint satisfaction
- End-to-end integration tests

## Security Considerations

### What Phase 2 Provides ✅

1. **Binary Authenticity**: Memory hash proves correct program
2. **Fetch Correctness**: Each instruction fetch verified
3. **Load Correctness**: Data loads verified against memory hash
4. **No Modification**: Read-only = no consistency issues

### What Phase 2 Does NOT Provide ❌

1. **Write Support**: No memory writes
2. **Dynamic Memory**: Memory fixed at trace generation
3. **Memory Consistency**: Not needed (read-only)
4. **Permutation Argument**: Not needed (read-only)

### Attack Resistance

**Attack**: Prover uses different memory than claimed
- **Defense**: Memory hash in polynomial, verified by Ligerito

**Attack**: Prover reads from wrong address
- **Defense**: Memory fetch constraint checks address calculation

**Attack**: Prover reads wrong value
- **Defense**: Memory value constraint checks against expected

## Performance Estimation

**Memory Size Impact**:
- Small programs (< 1KB): Negligible overhead
- Medium programs (< 64KB): ~10% overhead (memory hash)
- Large programs (< 256KB): ~20% overhead

**Polynomial Size**:
- Phase 1: ~20 elements/step
- Phase 2: ~22 elements/step (+10% size)

**Proof Size**:
- Expected: ~35 KB (vs ~33 KB in Phase 1)
- Still constant regardless of memory size!

## Migration Path

### Backward Compatibility

Phase 1 programs still work:
```rust
// Phase 1 program (no memory)
let trace = execute_and_trace_v1(&program, initial_regs);

// Phase 2 program (with memory)
let memory = ReadOnlyMemory::new(vec![0; 1024]);
let trace = execute_and_trace_v2(&program, initial_regs, &memory);
```

### Gradual Rollout

1. Implement Phase 2 alongside Phase 1
2. Keep Phase 1 tests passing
3. Add Phase 2 tests incrementally
4. Eventually deprecate Phase 1 (or keep as special case)

## Future: Phase 3 Preview

Phase 3 will add **write support**:
- Memory writes tracked
- Permutation argument for consistency
- Grand product over (address, time, value) tuples

**Complexity jump**: Phase 2 → 3 is MUCH harder than 1 → 2
- Read-only: No ordering issues
- Read-write: Must prove consistency across time

## References

- [RISC-V LOAD](https://riscv.org/wp-content/uploads/2017/05/riscv-spec-v2.2.pdf) - Instruction format
- [Plonk Permutation](https://eprint.iacr.org/2019/953.pdf) - For Phase 3
- [Cairo Memory Model](https://eprint.iacr.org/2021/1063.pdf) - Continuous memory design

## Next Steps

1. ✅ Design complete (this document)
2. ⏭️ Implement `ReadOnlyMemory` struct
3. ⏭️ Add LOAD opcode to trace
4. ⏭️ Extend constraints for memory
5. ⏭️ Update arithmetization
6. ⏭️ Write tests
