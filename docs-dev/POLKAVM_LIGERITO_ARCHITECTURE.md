# PolkaVM + Ligerito zkVM Architecture

## The Correct Flow

```
1. Application (Rust) → compiles to PolkaVM bytecode
2. PolkaVM executes → WE INSTRUMENT to capture trace
3. Trace → Arithmetize to polynomial
4. Polynomial → Ligerito proves (NATIVE, fast!)
5. Proof → Verify (C verifier in PolkaVM or on-chain)
```

## Phase 1: Capture PolkaVM Execution Trace

### What We Need to Capture

For each instruction executed:
- **Program counter (PC)**: Current instruction address
- **Registers**: All 13 RV32EM registers (a0-a7, t0-t4, sp, ra)
- **Instruction**: The opcode being executed
- **Memory access** (if any): address + value + read/write flag

### How to Instrument PolkaVM

PolkaVM runs in a loop calling `instance.run()`. We can hook into this:

```rust
use polkavm::{Instance, Reg, InterruptKind};

pub struct ExecutionTrace {
    pub steps: Vec<ExecutionStep>,
}

pub struct ExecutionStep {
    // State before instruction
    pub pc: u32,
    pub regs: [u32; 13],  // a0-a7, t0-t4, sp, ra

    // Instruction details (from PolkaVM)
    pub opcode: u8,

    // Memory access (if any)
    pub mem_addr: Option<u32>,
    pub mem_value: Option<u32>,
    pub is_write: bool,
}

pub fn trace_execution(instance: &mut Instance) -> ExecutionTrace {
    let mut trace = ExecutionTrace { steps: Vec::new() };

    loop {
        // Capture state BEFORE instruction
        let step = ExecutionStep {
            pc: instance.program_counter(),
            regs: [
                instance.reg(Reg::A0), instance.reg(Reg::A1),
                instance.reg(Reg::A2), instance.reg(Reg::A3),
                instance.reg(Reg::A4), instance.reg(Reg::A5),
                instance.reg(Reg::A6), instance.reg(Reg::A7),
                instance.reg(Reg::T0), instance.reg(Reg::T1),
                instance.reg(Reg::T2), instance.reg(Reg::T3),
                instance.reg(Reg::T4),
            ],
            opcode: 0, // TODO: extract from bytecode
            mem_addr: None,  // TODO: detect memory ops
            mem_value: None,
            is_write: false,
        };

        trace.steps.push(step);

        // Execute one instruction
        match instance.run() {
            Ok(InterruptKind::Finished) => break,
            Ok(_) => continue,  // Handle host calls
            Err(e) => panic!("Execution error: {:?}", e),
        }
    }

    trace
}
```

**Challenge**: PolkaVM doesn't expose single-step execution. We need to either:
1. Fork PolkaVM and add step-by-step execution
2. Use the interpreter backend (slower but traceable)
3. Post-process compiled code to inject trace points

## Phase 2: Trace → Polynomial Arithmetization

Convert the execution trace to a polynomial that encodes all constraints:

```rust
use ligerito_binary_fields::BinaryElem32;

pub fn arithmetize_trace(trace: &ExecutionTrace) -> Vec<BinaryElem32> {
    let mut poly = Vec::new();

    for (i, step) in trace.steps.iter().enumerate() {
        // Encode current state (~20 field elements per step)
        poly.push(BinaryElem32::from(step.pc));

        for reg in &step.regs {
            poly.push(BinaryElem32::from(*reg));
        }

        poly.push(BinaryElem32::from(step.opcode as u32));

        // CONSTRAINT 1: PC increments correctly
        // next_pc = pc + 4 (unless branch/jump)
        if i + 1 < trace.steps.len() {
            let next_pc = trace.steps[i + 1].pc;
            let expected_pc = if is_branch(step.opcode) {
                compute_branch_target(step)
            } else {
                step.pc + 4
            };

            // Encode constraint: next_pc == expected_pc
            let constraint = if next_pc == expected_pc { 1 } else { 0 };
            poly.push(BinaryElem32::from(constraint));
        }

        // CONSTRAINT 2: ALU operations are correct
        // For each arithmetic instruction, verify the result
        if is_alu_op(step.opcode) && i + 1 < trace.steps.len() {
            let result = execute_alu(step);
            let rd_index = get_dest_register(step.opcode);
            let actual = trace.steps[i + 1].regs[rd_index];

            let constraint = if result == actual { 1 } else { 0 };
            poly.push(BinaryElem32::from(constraint));
        }

        // CONSTRAINT 3: Memory consistency
        // If step reads memory, value must match last write to that address
        if let Some(addr) = step.mem_addr {
            let constraint = verify_memory_consistency(trace, i, addr);
            poly.push(BinaryElem32::from(constraint));
        }

        // CONSTRAINT 4: Register preservation
        // Registers not written should stay the same
        if i + 1 < trace.steps.len() {
            let rd = get_dest_register(step.opcode);
            for r in 0..13 {
                if r != rd {
                    let preserved = trace.steps[i+1].regs[r] == step.regs[r];
                    poly.push(BinaryElem32::from(preserved as u32));
                }
            }
        }
    }

    // Pad to power of 2 for FFT
    let target_len = poly.len().next_power_of_two();
    poly.resize(target_len, BinaryElem32::zero());

    poly
}
```

**Key Insight**: The constraints are **universal** - they just encode RV32EM instruction semantics. This works for ANY program!

## Phase 3: Prove with Native Ligerito

```rust
use ligerito::{prove, hardcoded_config_20};
use std::marker::PhantomData;

pub fn prove_polkavm_execution(trace: &ExecutionTrace) -> Result<LigeritoProof> {
    // 1. Arithmetize trace to polynomial
    let poly = arithmetize_trace(trace);

    // 2. Determine size (should be power of 2)
    let log_size = poly.len().ilog2();

    // 3. Get appropriate Ligerito config
    let config = match log_size {
        20 => hardcoded_config_20(
            PhantomData::<BinaryElem32>,
            PhantomData::<BinaryElem128>,
        ),
        _ => panic!("Unsupported polynomial size: 2^{}", log_size),
    };

    // 4. Prove! (This is FAST - native Ligerito with all optimizations)
    let proof = prove(&config, &poly)?;

    Ok(proof)
}
```

## Phase 4: Verify (Multiple Options)

### Option A: C Verifier in PolkaVM (JAM-compatible!)

```c
// verifier.c - runs in PolkaVM
int verify_ligerito_proof(
    const uint8_t* proof_data,
    size_t proof_len,
    const uint8_t* public_inputs,
    size_t inputs_len
) {
    // Simple trace verification (from MERKLE_MULTIPROOFS_INSIGHTS.md)
    // - Verify each Merkle trace independently
    // - Check sumcheck polynomial evaluation
    // - Return 1 if valid, 0 if invalid
}
```

### Option B: Native Rust Verifier

```rust
use ligerito::{verify, hardcoded_config_20_verifier};

pub fn verify_polkavm_proof(proof: &LigeritoProof) -> Result<bool> {
    let config = hardcoded_config_20_verifier();
    verify(&config, proof)
}
```

## Why This Architecture is Powerful

1. **General-Purpose**: Works for ANY Rust program (or any RISC-V)
2. **Fast Proving**: Native Ligerito with SIMD (~5 seconds for 2^20)
3. **Simple Verification**: Trace-based (perfect for C in PolkaVM)
4. **No Application-Specific Circuits**: Same constraints for all programs
5. **JAM-Compatible**: C verifier runs directly in PolkaVM

## Next Steps

1. ✅ Understand the flow (you corrected me - thanks!)
2. **Implement PolkaVM tracer** (need to figure out single-stepping)
3. **Implement arithmetization** (RV32EM constraints)
4. **Test end-to-end** with simple program
5. **Write C verifier** for PolkaVM
6. **(Optional) Port Ligerito to no_std** for recursion later

## Open Questions

1. **How to single-step PolkaVM?**
   - Option 1: Use interpreter mode (slower but traceable)
   - Option 2: Fork and modify to add step hook
   - Option 3: Instrument compiled code

2. **Trace size**:
   - 1M instructions = 1M steps × ~20 field elements = 20M elements
   - Need to chunk into 2^20 pieces or use larger polynomial size

3. **Public inputs/outputs**:
   - How to extract from trace?
   - First/last register values? Memory range?

Let's tackle question #1 first - how to capture PolkaVM execution step-by-step?
