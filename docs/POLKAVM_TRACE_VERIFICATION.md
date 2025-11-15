# How Does the Verifier Know the Trace is Correct?

## The Critical Question

> "How does the verifier verify that the trace is actually the trace of that executed .polkavm binary?"

**Answer**: The verifier doesn't re-execute! Instead, the polynomial encodes:
1. The program bytecode itself
2. The execution trace
3. Constraints proving the trace correctly executes that bytecode

## How RISC Zero (and other zkVMs) Work

### Step 1: Commit to the Program

The polynomial includes the **entire program bytecode**:

```rust
pub fn arithmetize_polkavm(
    binary: &[u8],           // The .polkavm bytecode
    trace: &ExecutionTrace,  // The execution trace
) -> Vec<BinaryElem32> {
    let mut poly = Vec::new();

    // PART 1: Encode the program binary (read-only memory)
    for (addr, byte) in binary.iter().enumerate() {
        poly.push(BinaryElem32::from(addr as u32));
        poly.push(BinaryElem32::from(*byte as u32));
    }

    // PART 2: Encode the execution trace
    for step in &trace.steps {
        // ... (we'll detail this below)
    }

    poly
}
```

### Step 2: Constrain Instruction Fetch

At each execution step, we prove "the instruction at PC was correctly fetched from the binary":

```rust
for (i, step) in trace.steps.iter().enumerate() {
    // Encode current PC
    poly.push(BinaryElem32::from(step.pc));

    // CONSTRAINT: The opcode at this PC matches the binary
    let binary_addr = step.pc as usize;
    let actual_opcode = binary[binary_addr];
    let claimed_opcode = step.opcode;

    // This constraint MUST be satisfied!
    let constraint = if actual_opcode == claimed_opcode { 1 } else { 0 };
    poly.push(BinaryElem32::from(constraint));

    // The verifier checks that ALL constraints equal 1
    // If prover lies about the opcode, this constraint fails!
}
```

### Step 3: Constrain Execution Semantics

For each instruction, we verify it was executed correctly:

```rust
// Example: ADD instruction
if step.opcode == ADD {
    let rs1_val = step.regs[step.rs1];
    let rs2_val = step.regs[step.rs2];
    let result = rs1_val.wrapping_add(rs2_val);

    // Next step must have rd = result
    let next_rd_val = trace.steps[i+1].regs[step.rd];

    // Constraint: result equals next rd value
    let constraint = if result == next_rd_val { 1 } else { 0 };
    poly.push(BinaryElem32::from(constraint));
}
```

## Complete Example: Simple Program

Let's trace through a concrete example.

### The Program Binary
```assembly
// program.polkavm
0x1000: ADD  a0, a1, a2   // opcode: 0x33, rd=a0, rs1=a1, rs2=a2
0x1004: SUB  a3, a0, a2   // opcode: 0x33, rd=a3, rs1=a0, rs2=a2
0x1008: HALT
```

### The Execution Trace
```rust
ExecutionTrace {
    steps: vec![
        // Step 0: Before ADD
        ExecutionStep {
            pc: 0x1000,
            opcode: 0x33,  // ADD
            regs: [0, 5, 3, 0, ...],  // a0=0, a1=5, a2=3
        },
        // Step 1: After ADD, before SUB
        ExecutionStep {
            pc: 0x1004,
            opcode: 0x33,  // SUB
            regs: [8, 5, 3, 0, ...],  // a0=8 (5+3), a1=5, a2=3
        },
        // Step 2: After SUB
        ExecutionStep {
            pc: 0x1008,
            opcode: 0x00,  // HALT
            regs: [8, 5, 3, 5, ...],  // a3=5 (8-3)
        },
    ]
}
```

### The Polynomial (Simplified)

```rust
poly = vec![
    // SECTION 1: Program binary
    BinaryElem32::from(0x1000),  // address
    BinaryElem32::from(0x33),    // opcode: ADD
    BinaryElem32::from(0x1004),  // address
    BinaryElem32::from(0x33),    // opcode: SUB
    BinaryElem32::from(0x1008),  // address
    BinaryElem32::from(0x00),    // opcode: HALT

    // SECTION 2: Execution trace with constraints
    // Step 0
    BinaryElem32::from(0x1000),  // PC
    BinaryElem32::from(0x33),    // claimed opcode
    BinaryElem32::from(1),       // CONSTRAINT 1: opcode matches binary[PC] ✓
    BinaryElem32::from(5),       // a1 value
    BinaryElem32::from(3),       // a2 value
    BinaryElem32::from(8),       // expected result (5+3)
    BinaryElem32::from(1),       // CONSTRAINT 2: next_a0 == 8 ✓

    // Step 1
    BinaryElem32::from(0x1004),  // PC
    BinaryElem32::from(0x33),    // claimed opcode
    BinaryElem32::from(1),       // CONSTRAINT 1: opcode matches binary[PC] ✓
    BinaryElem32::from(8),       // a0 value
    BinaryElem32::from(3),       // a2 value
    BinaryElem32::from(5),       // expected result (8-3)
    BinaryElem32::from(1),       // CONSTRAINT 2: next_a3 == 5 ✓

    // ... etc
];
```

### What Happens if Prover Lies?

**Scenario 1: Fake Opcode**
```rust
// Prover claims step 0 was SUB instead of ADD
step.opcode = 0x33;  // SUB opcode

// But binary[0x1000] = 0x33 (ADD)
// CONSTRAINT FAILS:
let constraint = if binary[0x1000] == 0x33 { 1 } else { 0 };  // = 0 ❌
```

**Scenario 2: Wrong Result**
```rust
// Prover claims 5 + 3 = 10 (wrong!)
next_step.regs[a0] = 10;

// CONSTRAINT FAILS:
let expected = 5 + 3;  // = 8
let actual = 10;
let constraint = if expected == actual { 1 } else { 0 };  // = 0 ❌
```

**When verification happens:**
- Ligerito verifier checks that ALL constraint polynomials evaluate to 1
- If ANY constraint is 0, the proof is rejected!
- **The prover cannot lie** without breaking the constraints

## How the Verifier Works (Without Re-execution)

```rust
pub fn verify_polkavm_proof(
    binary: &[u8],           // The .polkavm bytecode (public input)
    proof: &LigeritoProof,   // The proof
) -> bool {
    // 1. Reconstruct the commitment to the binary
    let binary_commitment = hash_binary(binary);

    // 2. Extract commitment from proof
    let claimed_binary_commitment = proof.public_inputs[0];

    // 3. Check they match
    if binary_commitment != claimed_binary_commitment {
        return false;  // Prover used different binary!
    }

    // 4. Verify the Ligerito proof
    //    This checks that:
    //    - All constraints are satisfied
    //    - The polynomial is committed correctly
    //    - The sumcheck passes
    let verifier_config = hardcoded_config_20_verifier();
    verify(&verifier_config, proof)
}
```

**Key insight**: The verifier only needs to:
1. Check the binary hash matches
2. Verify the Ligerito proof (which proves all constraints hold)
3. **NO re-execution needed!**

## Memory Consistency

The same technique applies to memory:

```rust
// Encode memory as a table: (address, value, timestamp)
for step in trace.steps {
    if let Some(mem_access) = step.memory_access {
        poly.push(BinaryElem32::from(mem_access.addr));
        poly.push(BinaryElem32::from(mem_access.value));
        poly.push(BinaryElem32::from(step.timestamp));

        if mem_access.is_write {
            // Record write
        } else {
            // CONSTRAINT: Read value matches last write
            let last_write = find_last_write(mem_access.addr, step.timestamp);
            let constraint = if mem_access.value == last_write { 1 } else { 0 };
            poly.push(BinaryElem32::from(constraint));
        }
    }
}
```

## The Full Constraint System

For a complete zkVM, we need constraints for:

1. **Instruction Fetch**: `opcode == binary[PC]`
2. **PC Update**: `next_PC == PC + 4` (or branch target)
3. **ALU Operations**: `result == execute_op(opcode, rs1, rs2)`
4. **Register Updates**: `next_regs[rd] == result`
5. **Register Preservation**: `next_regs[r] == regs[r]` for r ≠ rd
6. **Memory Consistency**: Read values match last write
7. **Control Flow**: Branches/jumps computed correctly

All of these are encoded as polynomial constraints that Ligerito proves!

## Comparison to Traditional Verification

**Traditional (re-execution)**:
```rust
// Verifier must re-run the program
let result = execute_program(binary, input);
assert_eq!(result, claimed_output);  // Takes as long as original execution!
```

**zkVM (proof verification)**:
```rust
// Verifier just checks the proof
let valid = verify_proof(proof, binary_hash);  // Fast! (~100ms)
```

## Why This Works

The proof system ensures:
1. **Soundness**: If the prover lies, constraints fail → proof invalid
2. **Completeness**: If execution is correct, constraints pass → proof valid
3. **Succinctness**: Proof size is constant (~150 KB) regardless of trace length
4. **Efficiency**: Verification is fast (~100ms) regardless of trace length

## Next Steps for Implementation

1. **Define constraint system** for RV32EM instructions
2. **Implement arithmetization** that encodes:
   - Program binary
   - Execution trace
   - All constraints
3. **Generate proof** with Ligerito
4. **Verify proof** by checking:
   - Binary hash matches
   - Ligerito proof is valid

The verifier NEVER re-executes - it just checks mathematical constraints!
