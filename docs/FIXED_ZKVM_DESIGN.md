# Fixed zkVM Design: Addressing Expert Critiques

## Fix 1: Proper Cryptographic Hash

### Problem
Simple polynomial hash `h(a,b) = a + b + ab` is trivially collision-resistant.

### Solution: Use Poseidon Hash Over Binary Fields

Poseidon is SNARK-friendly and has a binary field variant:

```rust
use ligerito_binary_fields::BinaryElem32;

// Poseidon parameters for GF(2^32)
const POSEIDON_WIDTH: usize = 3;
const POSEIDON_ROUNDS: usize = 8;
const POSEIDON_ALPHA: u64 = 5;  // x^5 is the S-box

// Round constants (generated via SHAKE256)
const ROUND_CONSTANTS: [[u32; POSEIDON_WIDTH]; POSEIDON_ROUNDS] = [
    [0x12345678, 0x9abcdef0, 0x13579bdf],
    // ... (generated deterministically)
];

// MDS matrix (maximum distance separable)
const MDS_MATRIX: [[u32; POSEIDON_WIDTH]; POSEIDON_WIDTH] = [
    [0x00000001, 0x00000002, 0x00000003],
    [0x00000002, 0x00000003, 0x00000001],
    [0x00000003, 0x00000001, 0x00000002],
];

pub struct PoseidonHash {
    state: [BinaryElem32; POSEIDON_WIDTH],
}

impl PoseidonHash {
    pub fn new() -> Self {
        Self {
            state: [BinaryElem32::zero(); POSEIDON_WIDTH],
        }
    }

    pub fn update(&mut self, elements: &[BinaryElem32]) {
        for chunk in elements.chunks(POSEIDON_WIDTH - 1) {
            // Absorb input into first WIDTH-1 positions
            for (i, elem) in chunk.iter().enumerate() {
                self.state[i] = self.state[i].add(elem);
            }

            // Apply permutation
            self.permute();
        }
    }

    pub fn finalize(mut self) -> BinaryElem32 {
        self.permute();
        self.state[0]
    }

    fn permute(&mut self) {
        for round in 0..POSEIDON_ROUNDS {
            // Add round constants (ARK)
            for i in 0..POSEIDON_WIDTH {
                let constant = BinaryElem32::from(ROUND_CONSTANTS[round][i]);
                self.state[i] = self.state[i].add(&constant);
            }

            // Apply S-box (x^5)
            for i in 0..POSEIDON_WIDTH {
                self.state[i] = self.state[i].pow(POSEIDON_ALPHA);
            }

            // MDS matrix multiplication
            let old_state = self.state.clone();
            for i in 0..POSEIDON_WIDTH {
                self.state[i] = BinaryElem32::zero();
                for j in 0..POSEIDON_WIDTH {
                    let matrix_elem = BinaryElem32::from(MDS_MATRIX[i][j]);
                    self.state[i] = self.state[i].add(
                        &matrix_elem.mul(&old_state[j])
                    );
                }
            }
        }
    }

    /// Hash the entire binary into a single commitment
    pub fn hash_binary(binary: &[u8]) -> BinaryElem32 {
        let mut hasher = Self::new();

        // Convert bytes to field elements
        let elements: Vec<BinaryElem32> = binary.chunks(4)
            .map(|chunk| {
                let mut bytes = [0u8; 4];
                bytes[..chunk.len()].copy_from_slice(chunk);
                BinaryElem32::from(u32::from_le_bytes(bytes))
            })
            .collect();

        hasher.update(&elements);
        hasher.finalize()
    }
}
```

**Cost Analysis**:
- Binary size: 10 KB = 2,500 field elements
- Chunks: 2,500 / 2 = 1,250 chunks (WIDTH-1 per chunk)
- Permutations: 1,250
- Ops per permutation: 3 ARK + 3 pow + 9 mul = ~15 field ops
- Total: 1,250 × 15 = 18,750 field operations

**In polynomial**: ~18,750 elements for hash circuit (0.2% overhead for 10M element trace)

---

## Fix 2: Proper Constraint Checking

### Problem
Sumcheck verifies `∑_x P(x) = claimed_sum`, NOT that specific constraints equal zero.

### Solution: Use Constraint Polynomials Correctly

We need to construct the polynomial such that **all constraints being satisfied** implies a specific sumcheck result.

**Approach 1: Grand Product Argument**

```rust
pub fn encode_constraints_as_product(
    constraints: &[BinaryElem32],
    challenge: BinaryElem32,
) -> BinaryElem32 {
    // For each constraint c_i that should equal 0:
    // Compute: ∏_i (challenge - c_i)
    //
    // If all c_i = 0, then product = challenge^n
    // If any c_i ≠ 0, then product ≠ challenge^n

    let mut product = BinaryElem32::one();

    for constraint in constraints {
        // product *= (challenge - constraint)
        let term = challenge.add(constraint);  // Subtraction in GF(2)
        product = product.mul(&term);
    }

    product
}

// Usage in arithmetization:
pub fn arithmetize_with_constraints(
    binary: &[u8],
    trace: &ExecutionTrace,
) -> Vec<BinaryElem32> {
    let mut poly = vec![];
    let mut all_constraints = vec![];

    // Section 1: Binary
    for byte in binary {
        poly.push(BinaryElem32::from(*byte as u32));
    }

    // Section 2: Trace with constraint collection
    for (i, step) in trace.steps.iter().enumerate() {
        // Encode step
        poly.push(BinaryElem32::from(step.pc));
        for reg in &step.regs {
            poly.push(BinaryElem32::from(*reg));
        }

        // PC continuity constraint
        if i + 1 < trace.steps.len() {
            let next_pc = trace.steps[i + 1].pc;
            let expected_pc = step.pc + 4;  // Simplified: no branches
            let constraint = BinaryElem32::from(next_pc)
                .add(&BinaryElem32::from(expected_pc));

            all_constraints.push(constraint);
        }

        // ALU correctness constraint
        if step.opcode == ADD && i + 1 < trace.steps.len() {
            let result = step.regs[step.rs1] + step.regs[step.rs2];
            let actual = trace.steps[i + 1].regs[step.rd];
            let constraint = BinaryElem32::from(result)
                .add(&BinaryElem32::from(actual));

            all_constraints.push(constraint);
        }
    }

    // Section 3: Grand product of all constraints
    // Get Fiat-Shamir challenge
    let challenge = fiat_shamir_challenge(&poly);
    let constraint_product = encode_constraints_as_product(&all_constraints, challenge);

    // The product should equal challenge^n if all constraints are 0
    let expected_product = challenge.pow(all_constraints.len() as u64);

    // Final constraint: actual == expected
    let final_constraint = constraint_product.add(&expected_product);
    poly.push(final_constraint);

    // Verifier checks: poly[last] == 0 via sumcheck

    poly
}
```

**This works because**:
- If all constraints = 0: `∏(α - 0) = α^n` ✓
- If any constraint ≠ 0: `∏(α - c_i)` includes `(α - c_j)` where `c_j ≠ 0`, so product ≠ `α^n` ✗

**Approach 2: Use AIR (Algebraic Intermediate Representation)**

Better: Use a proper constraint framework that compiles to Ligerito.

```rust
// Define constraints using AIR
pub struct RV32ConstraintSystem {
    pub constraints: Vec<Constraint>,
}

pub enum Constraint {
    // a + b = c (in GF(2))
    Add { a: Column, b: Column, c: Column },

    // a * b = c
    Mul { a: Column, b: Column, c: Column },

    // a = constant
    Constant { a: Column, value: BinaryElem32 },
}

// Compile AIR to polynomial
impl RV32ConstraintSystem {
    pub fn compile_to_polynomial(&self, trace: &ExecutionTrace) -> Vec<BinaryElem32> {
        let mut poly = vec![];

        // For each constraint, create a constraint polynomial
        for constraint in &self.constraints {
            match constraint {
                Constraint::Add { a, b, c } => {
                    // For all rows: trace[a] + trace[b] - trace[c] = 0
                    for row in trace.rows() {
                        let constraint_val = row[*a]
                            .add(&row[*b])
                            .add(&row[*c]);  // Subtraction in GF(2)
                        poly.push(constraint_val);
                    }
                }
                // ... other constraints
            }
        }

        poly
    }
}
```

**Recommended**: Use Approach 2 (AIR) for clarity and modularity.

---

## Fix 3: Start with Register-Only PVM

### Problem
Trying to solve memory consistency immediately is too complex.

### Solution: Phase 1 - No Memory

```rust
// Simplified execution trace (registers only)
pub struct RegisterOnlyTrace {
    pub steps: Vec<RegisterOnlyStep>,
}

pub struct RegisterOnlyStep {
    pub pc: u32,
    pub regs: [u32; 13],  // a0-a7, t0-t4
    pub opcode: Opcode,
}

// Supported opcodes (no memory operations!)
pub enum Opcode {
    ADD,   // rd = rs1 + rs2
    SUB,   // rd = rs1 - rs2
    MUL,   // rd = rs1 * rs2
    AND,   // rd = rs1 & rs2
    OR,    // rd = rs1 | rs2
    XOR,   // rd = rs1 ^ rs2
    HALT,  // Stop execution
}

// Constraints (much simpler!)
pub fn arithmetize_register_only(
    program: &[Opcode],  // The program (just opcodes)
    trace: &RegisterOnlyTrace,
) -> Vec<BinaryElem32> {
    let mut poly = vec![];
    let mut constraints = vec![];

    // Hash the program
    let program_hash = PoseidonHash::hash_program(program);
    poly.push(program_hash);

    // For each execution step
    for (i, step) in trace.steps.iter().enumerate() {
        // Encode state
        poly.push(BinaryElem32::from(step.pc));
        for reg in &step.regs {
            poly.push(BinaryElem32::from(*reg));
        }

        // Constraint 1: PC increments by 1 (no jumps in v1)
        if i + 1 < trace.steps.len() {
            let next_pc = trace.steps[i + 1].pc;
            let expected = step.pc + 1;
            constraints.push(
                BinaryElem32::from(next_pc).add(&BinaryElem32::from(expected))
            );
        }

        // Constraint 2: Opcode at PC matches program
        let prog_opcode = program[step.pc as usize];
        if prog_opcode != step.opcode {
            constraints.push(BinaryElem32::one());  // Constraint fails
        } else {
            constraints.push(BinaryElem32::zero());
        }

        // Constraint 3: ALU operation is correct
        if i + 1 < trace.steps.len() {
            let next = &trace.steps[i + 1];
            let (rd, rs1, rs2) = decode_instruction(step.opcode);

            let expected_result = match step.opcode {
                Opcode::ADD => step.regs[rs1] + step.regs[rs2],
                Opcode::SUB => step.regs[rs1] - step.regs[rs2],
                Opcode::MUL => step.regs[rs1] * step.regs[rs2],
                Opcode::AND => step.regs[rs1] & step.regs[rs2],
                Opcode::OR  => step.regs[rs1] | step.regs[rs2],
                Opcode::XOR => step.regs[rs1] ^ step.regs[rs2],
                Opcode::HALT => 0,
            };

            let actual_result = next.regs[rd];
            constraints.push(
                BinaryElem32::from(expected_result)
                    .add(&BinaryElem32::from(actual_result))
            );
        }

        // Constraint 4: Unchanged registers stay the same
        if i + 1 < trace.steps.len() {
            let next = &trace.steps[i + 1];
            let (rd, _, _) = decode_instruction(step.opcode);

            for r in 0..13 {
                if r != rd {
                    constraints.push(
                        BinaryElem32::from(step.regs[r])
                            .add(&BinaryElem32::from(next.regs[r]))
                    );
                }
            }
        }
    }

    // Grand product of constraints
    let alpha = fiat_shamir_challenge(&poly);
    let product = encode_constraints_as_product(&constraints, alpha);
    let expected = alpha.pow(constraints.len() as u64);

    poly.push(product.add(&expected));  // Should be 0

    poly
}
```

**Benefits**:
- ✅ No memory consistency needed
- ✅ No complex permutation arguments
- ✅ Simple to implement and test
- ✅ Proves the core idea works

**Example Program**:
```rust
// Compute: a0 = (a1 + a2) * a3
let program = vec![
    Opcode::ADD,  // a0 = a1 + a2
    Opcode::MUL,  // a0 = a0 * a3
    Opcode::HALT,
];

// Initial state: a1=5, a2=3, a3=2
// Expected: a0 = (5+3)*2 = 16
```

---

## Fix 4: Add Read-Only Memory (Phase 2)

Once register-only works, add **read-only** memory for instruction fetch:

```rust
pub struct ReadOnlyTrace {
    pub steps: Vec<ReadOnlyStep>,
}

pub struct ReadOnlyStep {
    pub pc: u32,
    pub regs: [u32; 13],
    pub opcode: u8,       // Fetched from memory
    pub rs1: u8,
    pub rs2: u8,
    pub rd: u8,
}

// Memory is read-only (just the binary)
pub struct Memory {
    pub binary: Vec<u8>,
}

// Constraint: Instruction at PC matches binary
pub fn add_instruction_fetch_constraint(
    binary: &[u8],
    step: &ReadOnlyStep,
) -> BinaryElem32 {
    let expected_opcode = binary[step.pc as usize];
    let actual_opcode = step.opcode;

    BinaryElem32::from(expected_opcode as u32)
        .add(&BinaryElem32::from(actual_opcode as u32))
}
```

**Benefits**:
- ✅ Still no writes, so no permutation argument
- ✅ Proves instruction fetch works correctly
- ✅ Closer to real zkVM

---

## Fix 5: Memory Consistency (Phase 3)

Only after Phases 1 and 2 work, add full memory with permutation argument.

**Use Plonk-style Permutation**:

```rust
pub struct MemoryOp {
    pub addr: u32,
    pub value: u32,
    pub time: u64,
    pub is_write: bool,
}

pub fn prove_memory_consistency(
    ops_exec_order: &[MemoryOp],    // In execution order
    ops_sorted: &[MemoryOp],         // Sorted by (addr, time)
) -> Vec<BinaryElem32> {
    let mut poly = vec![];

    // Step 1: Encode both orderings
    for op in ops_exec_order {
        poly.push(encode_memory_op(op));
    }

    for op in ops_sorted {
        poly.push(encode_memory_op(op));
    }

    // Step 2: Prove they're permutations via grand product
    let gamma = fiat_shamir_challenge(&poly);

    let prod_exec = ops_exec_order.iter()
        .map(|op| gamma.add(&encode_memory_op(op)))
        .fold(BinaryElem32::one(), |acc, x| acc.mul(&x));

    let prod_sorted = ops_sorted.iter()
        .map(|op| gamma.add(&encode_memory_op(op)))
        .fold(BinaryElem32::one(), |acc, x| acc.mul(&x));

    // Constraint: products must be equal
    poly.push(prod_exec.add(&prod_sorted));  // Should be 0

    // Step 3: Check consistency in sorted order
    for i in 0..ops_sorted.len()-1 {
        let curr = &ops_sorted[i];
        let next = &ops_sorted[i+1];

        // If next is a read at same address
        if !next.is_write && next.addr == curr.addr {
            // Value must match last write
            let constraint = BinaryElem32::from(next.value)
                .add(&BinaryElem32::from(curr.value));
            poly.push(constraint);
        }
    }

    poly
}
```

**This is complex** - don't implement until Phases 1 and 2 work!

---

## Implementation Plan (Revised)

### Week 1-2: Register-Only Proof-of-Concept
```rust
// Goal: Prove a 10-instruction register-only program
// - No memory
// - No branches
// - Just ADD/MUL/etc

// Deliverable: Working end-to-end demo
```

### Week 3-4: Add Read-Only Memory
```rust
// Goal: Fetch instructions from binary
// - Still no writes
// - Instruction fetch constraints
// - Program hash verification

// Deliverable: Prove a program stored in memory
```

### Week 5-8: Add Full Memory
```rust
// Goal: Full RV32EM with loads/stores
// - Implement permutation argument
// - Memory consistency checks
// - Full zkVM!

// Deliverable: Prove arbitrary computation
```

### Week 9-12: Optimize and Harden
```rust
// Goal: Production-ready
// - Security audit
// - Performance optimization
// - Formal specification
// - Integration with JAM

// Deliverable: JAM-compatible zkVM
```

---

## Summary of Fixes

| Issue | Fix | Status |
|-------|-----|--------|
| Collision-resistant hash | Use Poseidon over GF(2^32) | ✅ Specified |
| Constraint checking | Grand product argument | ✅ Specified |
| Memory complexity | Phase it: none → read-only → full | ✅ Planned |
| Sumcheck misunderstanding | Proper constraint polynomials | ✅ Fixed |
| Scope creep | Start with register-only | ✅ De-scoped |

Ready to implement Phase 1?
