# PolkaVM Integration Plan

## Overview

Based on the comprehensive study of PolkaVM's architecture, this document outlines the practical steps to integrate PolkaVM execution with Ligerito proving.

## Key Insights from Study

1. **PolkaVM has 13 registers** - Same as our design! ✅
2. **~100 instructions** - More complex than our 9, but structured
3. **Segmented memory** - Different from our flat model
4. **Built-in tracing support** - Can use `step_tracing` mode
5. **Fully deterministic** - Perfect for ZK proofs

## Integration Strategy: Hybrid Approach

### Phase 1: Direct PolkaVM Trace Extraction (Recommended)

**Use PolkaVM's execution engine, extract traces, prove with Ligerito**

```rust
// Pseudocode for integration
pub fn prove_polkavm_execution(
    program_blob: &[u8],
    initial_state: &ExecutionState,
) -> LigeritoProof {
    // 1. Execute with PolkaVM (fast!)
    let (final_state, trace) = execute_and_trace_polkavm(program_blob, initial_state);

    // 2. Convert to our polynomial format
    let polynomial = arithmetize_polkavm_trace(&trace, program_blob);

    // 3. Prove with Ligerito
    let proof = ligerito::prove(&config, &polynomial)?;

    proof
}
```

**Advantages**:
- ✅ Use PolkaVM's optimized execution
- ✅ Proven compatibility with real binaries
- ✅ Near-native performance
- ✅ Minimal reimplementation

**Challenges**:
- Need to hook into PolkaVM's interpreter
- Map 100 instructions to constraints
- Handle segmented memory

## Implementation Roadmap

### Week 1-2: Foundation

**Goal**: Understand PolkaVM's execution model deeply

**Tasks**:
1. ✅ Study PolkaVM ISA (DONE)
2. Create PolkaVM register mapping
3. Implement PolkaVM memory model adapter
4. Write tests for memory segment routing

**Code to write**:
```rust
// src/pcvm/polkavm_adapter.rs

pub struct PolkaVMRegisters {
    pub ra: u32,  // Return address
    pub sp: u32,  // Stack pointer
    pub t0: u32, pub t1: u32, pub t2: u32,  // Temporaries
    pub s0: u32, pub s1: u32,               // Saved
    pub a0: u32, pub a1: u32, pub a2: u32,  // Arguments
    pub a3: u32, pub a4: u32, pub a5: u32,
}

impl From<polkavm::RegValues> for PolkaVMRegisters {
    fn from(regs: polkavm::RegValues) -> Self {
        Self {
            ra: regs[Reg::RA],
            sp: regs[Reg::SP],
            t0: regs[Reg::T0],
            t1: regs[Reg::T1],
            t2: regs[Reg::T2],
            s0: regs[Reg::S0],
            s1: regs[Reg::S1],
            a0: regs[Reg::A0],
            a1: regs[Reg::A1],
            a2: regs[Reg::A2],
            a3: regs[Reg::A3],
            a4: regs[Reg::A4],
            a5: regs[Reg::A5],
        }
    }
}

pub struct PolkaVMMemoryModel {
    ro_data: Vec<u8>,
    rw_data: Vec<u8>,
    heap: Vec<u8>,
    stack: Vec<u8>,
    aux: Vec<u8>,

    ro_base: u32,
    rw_base: u32,
    heap_base: u32,
    stack_base: u32,
    aux_base: u32,
}

impl PolkaVMMemoryModel {
    pub fn read_u32(&self, addr: u32) -> Option<u32> {
        // Route to correct segment
        if self.is_in_ro_data(addr) {
            self.read_from_ro_data(addr)
        } else if self.is_in_rw_data(addr) {
            self.read_from_rw_data(addr)
        } else if self.is_in_stack(addr) {
            self.read_from_stack(addr)
        } else {
            None
        }
    }
}
```

### Week 3-4: Trace Extraction

**Goal**: Extract execution traces from PolkaVM

**Tasks**:
1. Hook into PolkaVM's step_tracing mode
2. Capture register state after each instruction
3. Track memory accesses
4. Record control flow

**Code to write**:
```rust
// src/pcvm/polkavm_tracer.rs

use polkavm::{Config, Instance, Module, InterruptKind};

pub struct PolkaVMTrace {
    pub steps: Vec<PolkaVMStep>,
    pub initial_memory: PolkaVMMemoryModel,
    pub program_hash: BinaryElem32,
}

pub struct PolkaVMStep {
    pub pc: u32,
    pub instruction: polkavm::Instruction,
    pub regs_before: PolkaVMRegisters,
    pub regs_after: PolkaVMRegisters,
    pub memory_access: Option<MemoryAccess>,
}

pub fn extract_trace(
    program_blob: &[u8],
    input_data: &[u8],
) -> Result<PolkaVMTrace, Error> {
    // 1. Create module
    let module = Module::from_blob(&config, program_blob)?;

    // 2. Enable step tracing
    let mut config = Config::default();
    config.set_step_tracing(true);

    // 3. Create instance
    let mut instance = module.instantiate()?;
    instance.set_input(input_data);

    // 4. Execute and collect trace
    let mut trace = PolkaVMTrace::new();

    loop {
        // Capture state before step
        let regs_before = capture_registers(&instance);
        let pc = instance.program_counter();

        // Execute one step
        match instance.run()? {
            InterruptKind::Step => {
                // Capture state after step
                let regs_after = capture_registers(&instance);
                let instruction = instance.last_instruction();

                trace.push(PolkaVMStep {
                    pc,
                    instruction,
                    regs_before,
                    regs_after,
                    memory_access: capture_memory_access(&instance),
                });
            }
            InterruptKind::Finished => break,
            InterruptKind::Trap => return Err(Error::ExecutionTrapped),
            _ => {}
        }
    }

    Ok(trace)
}

fn capture_registers(instance: &Instance) -> PolkaVMRegisters {
    PolkaVMRegisters {
        ra: instance.get_reg(Reg::RA),
        sp: instance.get_reg(Reg::SP),
        // ... capture all 13 registers
    }
}
```

### Week 5-7: Constraint Generation

**Goal**: Generate constraints for all PolkaVM instructions

**Tasks**:
1. Map each of ~100 instructions to constraints
2. Implement arithmetic constraints (add, sub, mul, div, rem)
3. Implement bitwise constraints (and, or, xor, shifts)
4. Implement memory access constraints
5. Implement control flow constraints

**Strategy**: Auto-generate constraint code from instruction table

```rust
// src/pcvm/polkavm_constraints.rs

pub fn generate_instruction_constraint(
    step: &PolkaVMStep,
    memory: &PolkaVMMemoryModel,
) -> Vec<BinaryElem32> {
    use polkavm::Instruction::*;

    let mut constraints = Vec::new();

    match step.instruction {
        // Arithmetic
        Add32(dst, src1, src2) => {
            let expected = step.regs_before[src1]
                .wrapping_add(step.regs_before[src2]);
            let actual = step.regs_after[dst];
            constraints.push(BinaryElem32::from(expected ^ actual));
        }

        Sub32(dst, src1, src2) => {
            let expected = step.regs_before[src1]
                .wrapping_sub(step.regs_before[src2]);
            let actual = step.regs_after[dst];
            constraints.push(BinaryElem32::from(expected ^ actual));
        }

        // Division (deterministic!)
        DivUnsigned32(dst, src1, src2) => {
            let lhs = step.regs_before[src1];
            let rhs = step.regs_before[src2];
            let expected = if rhs == 0 { u32::MAX } else { lhs / rhs };
            let actual = step.regs_after[dst];
            constraints.push(BinaryElem32::from(expected ^ actual));
        }

        // Memory load
        LoadIndirectU32(dst, base, offset) => {
            let addr = step.regs_before[base].wrapping_add(offset);
            let expected = memory.read_u32(addr).unwrap_or(0);
            let actual = step.regs_after[dst];
            constraints.push(BinaryElem32::from(expected ^ actual));

            // Also constraint: address was accessed
            if let Some(access) = &step.memory_access {
                constraints.push(BinaryElem32::from(addr ^ access.address));
            }
        }

        // Branches
        BranchEqImm(reg, imm, offset) => {
            let reg_val = step.regs_before[reg];
            let should_branch = reg_val == imm;
            let expected_pc = if should_branch {
                step.pc.wrapping_add(offset)
            } else {
                step.pc + instruction_length
            };
            constraints.push(BinaryElem32::from(expected_pc ^ step.next_pc));
        }

        // ... 95 more instructions ...
    }

    constraints
}
```

**Auto-generation approach**:
```rust
// build.rs or codegen tool

// Read instruction table
let instructions = parse_instruction_table();

// Generate constraint code
for instr in instructions {
    generate_constraint_function(instr);
}
```

### Week 8-10: Memory Proving

**Goal**: Prove memory consistency across segments

**Tasks**:
1. Implement segment routing constraints
2. Add bounds checking
3. Merkle tree for each segment
4. Prove memory accesses are valid

```rust
// src/pcvm/polkavm_memory_proving.rs

pub struct SegmentedMemoryProof {
    pub ro_data_hash: BinaryElem32,
    pub rw_data_hash: BinaryElem32,
    pub stack_hash: BinaryElem32,
    pub segment_roots: Vec<BinaryElem32>,
}

pub fn prove_memory_access(
    access: &MemoryAccess,
    segments: &SegmentedMemoryProof,
) -> Vec<Constraint> {
    let mut constraints = Vec::new();

    // 1. Determine which segment
    let segment = determine_segment(access.address);

    // 2. Constraint: address is in valid range
    let in_bounds = check_segment_bounds(access.address, segment);
    constraints.push(Constraint::MemoryBounds(in_bounds));

    // 3. Constraint: value matches segment data
    let merkle_proof = segments.get_proof(segment, access.address);
    let value_correct = verify_merkle_proof(
        merkle_proof,
        access.value,
        access.address,
    );
    constraints.push(Constraint::MemoryValue(value_correct));

    constraints
}
```

### Week 11-13: Integration & Testing

**Goal**: End-to-end proving of real PolkaVM programs

**Tasks**:
1. Compile simple Rust programs to PolkaVM
2. Extract traces
3. Generate and verify proofs
4. Benchmark performance

**Test programs**:
```rust
// examples/polkavm/fibonacci.rs

#[polkavm_derive::polkavm_export]
pub extern "C" fn fibonacci(n: u32) -> u32 {
    if n <= 1 {
        n
    } else {
        fibonacci(n - 1) + fibonacci(n - 2)
    }
}

// Compile to PolkaVM:
// polkavm-linker -o fibonacci.polkavm fibonacci.elf
```

```rust
// Test proving
#[test]
fn test_prove_fibonacci() {
    let program = include_bytes!("fibonacci.polkavm");
    let input = 10u32.to_le_bytes();

    // Execute and trace
    let trace = extract_trace(program, &input).unwrap();

    // Arithmetize
    let polynomial = arithmetize_polkavm_trace(&trace, program);

    // Prove
    let proof = prove(&config, &polynomial).unwrap();

    // Verify
    let valid = verify(&verifier_config, &proof).unwrap();
    assert!(valid);
}
```

### Week 14-17: Optimization & Production

**Goal**: Production-ready PolkaVM proving

**Tasks**:
1. Optimize constraint generation
2. Parallel trace extraction
3. Memory-efficient polynomial encoding
4. Documentation and examples

## Technical Challenges & Solutions

### Challenge 1: 100 Instructions

**Problem**: Too many instructions to manually implement constraints

**Solution**: Code generation
```rust
// Generate from instruction table
macro_rules! impl_instruction_constraints {
    ($($opcode:ident => $constraint_fn:ident),*) => {
        match instruction {
            $(Instruction::$opcode(..) => $constraint_fn(step),)*
        }
    };
}
```

### Challenge 2: Segmented Memory

**Problem**: Complex memory routing

**Solution**: Conditional constraints with range checks
```rust
fn memory_read_constraint(addr: u32) -> BinaryElem32 {
    // Select correct segment based on address range
    if addr < 0x20000 {
        read_from_ro_data(addr)
    } else if addr < 0x40000 {
        read_from_rw_data(addr)
    } else if addr >= 0xfffdc000 {
        read_from_stack(addr)
    } else {
        BinaryElem32::zero() // Out of bounds
    }
}
```

### Challenge 3: Control Flow Complexity

**Problem**: Branches, jumps, indirect jumps

**Solution**: Basic block decomposition
```rust
pub struct BasicBlock {
    pub start_pc: u32,
    pub end_pc: u32,
    pub instructions: Vec<Instruction>,
    pub successors: Vec<u32>,  // Possible next blocks
}

// Prove each basic block separately
// Link blocks with continuity constraints
```

## Minimal Viable Integration

**For initial testing, implement only**:
1. Core ALU: add, sub, mul (3 instructions)
2. Immediate loads: load_imm (1 instruction)
3. Memory: load_indirect_u32, store_indirect_u32 (2 instructions)
4. Control: jump, branch_eq (2 instructions)
5. System: trap (1 instruction)

**Total**: 9 instructions - Same as our current VM!

This allows testing the infrastructure before scaling to all 100 instructions.

## Next Steps

1. **This week**: Implement PolkaVMRegisters and PolkaVMMemoryModel
2. **Next week**: Hook into PolkaVM step tracing
3. **Week after**: Generate constraints for core 9 instructions
4. **Then**: Scale to full instruction set

## Success Metrics

- ✅ Can extract traces from PolkaVM execution
- ✅ Can generate constraints for all instructions
- ✅ Can prove simple programs (< 100 instructions)
- ✅ Can prove complex programs (fibonacci, array sum)
- ✅ Proof size < 50 KB for typical programs
- ✅ Proving time < 5 seconds for typical programs
- ✅ Verification time < 100 ms

## Resources Needed

- PolkaVM crate as dependency
- Example PolkaVM binaries for testing
- Documentation on PolkaVM internals
- Compiler toolchain (polkavm-linker)

## Conclusion

Integration is **feasible** and **well-scoped**. The key insight is that PolkaVM already has tracing support, so we don't need to reimplement the VM - just extract traces and prove them with Ligerito.

Estimated total effort: **11-17 weeks** for production-ready integration.
