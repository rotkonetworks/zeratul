# Formal Verification Challenge: Can the Prover Actually Cheat?

## The Claim

> "The polynomial encodes the binary and trace. Constraints prove correct execution. The prover cannot lie without breaking constraints."

Let me attack this claim systematically.

## Attack Vector 1: Fake Binary

**Prover's Strategy**: Submit a different binary than claimed.

```rust
// Public: Verifier expects proof of binary_A execution
let binary_A = [0x33, 0x13, 0x67];  // The real program

// Prover: Actually proves execution of binary_B
let binary_B = [0x13, 0x33, 0x67];  // Different program

// Prover's polynomial includes binary_B
let poly = arithmetize_polkavm(binary_B, trace_of_B);

// Prover generates proof
let proof = ligerito::prove(&config, &poly)?;
```

**Defense**: The binary hash must be a **public input** to the proof!

```rust
// CORRECT verification:
pub fn verify_polkavm_proof(
    expected_binary_hash: Hash,  // Public input
    proof: &LigeritoProof,
) -> bool {
    // The proof must include the binary hash it's proving
    let claimed_hash = proof.public_inputs[0];

    if claimed_hash != expected_binary_hash {
        return false;  // Different binary!
    }

    verify(&verifier_config, proof)
}
```

**Problem**: How do we make the binary hash part of the public input in Ligerito?

**Answer**: We need to modify the arithmetization to separate:
- **Public inputs**: Binary hash, input data, output data
- **Private witness**: Execution trace

```rust
pub struct PolkaVMProof {
    pub binary_hash: Hash,
    pub inputs: Vec<u32>,
    pub outputs: Vec<u32>,
    pub ligerito_proof: LigeritoProof,
}
```

**Verdict**: ✅ **FIXED** - Binary hash as public input prevents this attack.

---

## Attack Vector 2: Skip Instructions

**Prover's Strategy**: Skip expensive instructions in the trace.

```rust
// Real execution:
// Step 0: PC=0x1000, ADD a0, a1, a2
// Step 1: PC=0x1004, MUL a0, a0, a3  <- expensive!
// Step 2: PC=0x1008, SUB a1, a0, a2

// Prover's fake trace: Skip the MUL
ExecutionTrace {
    steps: vec![
        ExecutionStep { pc: 0x1000, opcode: ADD, ... },
        // SKIPPED: Step 1
        ExecutionStep { pc: 0x1008, opcode: SUB, ... },
    ]
}
```

**Defense**: PC continuity constraint!

```rust
for i in 0..trace.steps.len()-1 {
    let current_pc = trace.steps[i].pc;
    let next_pc = trace.steps[i+1].pc;
    let current_opcode = trace.steps[i].opcode;

    let expected_next_pc = if is_branch(current_opcode) {
        compute_branch_target(&trace.steps[i])
    } else {
        current_pc + 4  // Sequential execution
    };

    // CONSTRAINT: PC must increment correctly
    let constraint = if next_pc == expected_next_pc { 1 } else { 0 };
    poly.push(BinaryElem32::from(constraint));
}
```

**Attack Result**:
- Prover skips PC=0x1004
- Next PC jumps from 0x1000 to 0x1008
- But opcode at 0x1000 is ADD (not a branch!)
- Expected next PC = 0x1000 + 4 = 0x1004
- Actual next PC = 0x1008
- **Constraint fails!** ❌

**Verdict**: ✅ **SAFE** - Cannot skip instructions without breaking PC continuity.

---

## Attack Vector 3: Wrong Computation Result

**Prover's Strategy**: Claim incorrect result for ALU operation.

```rust
// Step 0: a1=5, a2=3, opcode=ADD
// Real result: a0 = 5 + 3 = 8
// Prover claims: a0 = 10

ExecutionStep {
    pc: 0x1000,
    opcode: ADD,
    regs: [0, 5, 3, ...],  // Before
},
ExecutionStep {
    pc: 0x1004,
    regs: [10, 5, 3, ...],  // After - WRONG!
}
```

**Defense**: ALU correctness constraint!

```rust
if step.opcode == ADD {
    let rs1_val = step.regs[step.rs1];  // = 5
    let rs2_val = step.regs[step.rs2];  // = 3
    let expected_result = rs1_val.wrapping_add(rs2_val);  // = 8

    let next_step = &trace.steps[i+1];
    let actual_result = next_step.regs[step.rd];  // Prover claims 10

    // CONSTRAINT: Result must be correct
    let constraint = if actual_result == expected_result { 1 } else { 0 };
    poly.push(BinaryElem32::from(constraint));
}
```

**Attack Result**:
- Expected: 8
- Actual: 10
- Constraint = 0 ❌
- **Proof verification fails!**

**Verdict**: ✅ **SAFE** - Cannot lie about ALU results.

---

## Attack Vector 4: Memory Manipulation

**Prover's Strategy**: Read a value from memory that was never written.

```rust
// Real execution:
// Step 0: STORE [0x2000] = 42
// Step 1: LOAD a0 = [0x3000]  <- uninitialized memory!

// Prover claims: a0 = 0
```

**Defense**: Memory consistency table!

This is actually TRICKY. We need to track ALL memory operations:

```rust
pub struct MemoryTrace {
    // Sorted by (address, timestamp)
    pub operations: Vec<MemoryOp>,
}

pub struct MemoryOp {
    pub addr: u32,
    pub value: u32,
    pub timestamp: u64,
    pub is_write: bool,
}

// CONSTRAINT: For each read at (addr, time):
// value == last_write(addr, t < time).value
```

**Problem**: This requires sorting and lookup - expensive in a circuit!

**Solution**: Use permutation argument (like Plonk):
1. Prover provides memory operations in execution order
2. Prover provides same operations sorted by (address, timestamp)
3. Prove the two lists are permutations of each other
4. Check consistency in sorted order (easy - consecutive checks)

**Verdict**: ⚠️ **COMPLEX** - Need permutation argument for memory.

**Simpler alternative**: For small programs, include entire memory in trace:

```rust
pub struct ExecutionStep {
    pub pc: u32,
    pub regs: [u32; 13],
    pub memory: HashMap<u32, u32>,  // Full memory snapshot
}

// CONSTRAINT: Memory only changes at STORE instructions
for i in 0..trace.len()-1 {
    if trace[i].opcode != STORE {
        // Memory must be identical
        assert_eq!(trace[i].memory, trace[i+1].memory);
    } else {
        // Memory can change only at store address
        let store_addr = compute_store_addr(&trace[i]);
        for (addr, val) in &trace[i].memory {
            if addr != store_addr {
                assert_eq!(val, trace[i+1].memory[addr]);
            }
        }
    }
}
```

**Verdict**: ✅ **SAFE** (with permutation arg) or **EXPENSIVE** (with full memory snapshots)

---

## Attack Vector 5: Wrong Starting State

**Prover's Strategy**: Start with fake register/memory values.

```rust
// Program should start with regs = [0, 0, 0, ...]
// Prover starts with regs = [1000, 0, 0, ...] to cheat

ExecutionTrace {
    steps: vec![
        ExecutionStep {
            pc: 0x1000,
            regs: [1000, 0, 0, ...],  // WRONG initial state
            ...
        },
        ...
    ]
}
```

**Defense**: Initial state MUST be public input!

```rust
pub struct PolkaVMProof {
    pub binary_hash: Hash,
    pub initial_state: InitialState,  // PUBLIC INPUT
    pub final_state: FinalState,      // PUBLIC INPUT
    pub ligerito_proof: LigeritoProof,
}

pub struct InitialState {
    pub pc: u32,           // Usually 0x1000 (entry point)
    pub regs: [u32; 13],   // Usually all zeros
    pub memory: Vec<(u32, u32)>,  // Initial memory contents
}

// CONSTRAINT: First trace step matches initial state
let constraint = if trace.steps[0].pc == initial_state.pc
                 && trace.steps[0].regs == initial_state.regs { 1 } else { 0 };
poly.push(BinaryElem32::from(constraint));
```

**Verdict**: ✅ **FIXED** - Initial state as public input prevents this.

---

## Attack Vector 6: Fake Opcode from Binary

**Prover's Strategy**: Include a fake binary in the polynomial, claim different opcodes.

```rust
// Real binary: binary[0x1000] = ADD (0x33)
// Prover's polynomial: claims binary[0x1000] = SUB (0x13)

// Prover creates polynomial with fake binary section
let mut poly = vec![];

// Section 1: Fake binary
poly.push(BinaryElem32::from(0x1000));  // address
poly.push(BinaryElem32::from(0x13));    // FAKE opcode (SUB instead of ADD)

// Section 2: Execution trace using the fake opcode
poly.push(BinaryElem32::from(0x1000));  // PC
poly.push(BinaryElem32::from(0x13));    // claimed opcode
let constraint = if claimed_opcode == fake_binary[pc] { 1 } else { 0 };
poly.push(BinaryElem32::from(1));  // Constraint passes with fake binary!
```

**THIS IS THE CRITICAL ATTACK!**

**Defense**: The binary hash MUST be computed inside the polynomial and checked!

```rust
// CORRECT arithmetization:
let mut poly = vec![];

// 1. Include the binary
for (i, byte) in binary.iter().enumerate() {
    poly.push(BinaryElem32::from(*byte as u32));
}

// 2. Compute hash of the binary (inside the polynomial!)
let binary_hash = hash_polynomial(&poly[0..binary.len()]);

// 3. Make the hash a public output
poly.push(binary_hash);

// 4. Verifier checks: proof.public_outputs[0] == expected_binary_hash
```

**Wait... this doesn't work!**

The prover can still:
1. Include fake binary in polynomial
2. Compute hash of fake binary
3. Output that hash
4. Verifier has nothing to compare against!

**REAL SOLUTION**: The binary hash must be provided TO the proof system, not BY it!

```rust
// The verifier provides the expected binary hash as INPUT
pub fn verify_polkavm_proof(
    expected_binary_hash: Hash,  // Computed from real binary
    proof: &LigeritoProof,
) -> bool {
    // The polynomial must include constraints that prove:
    // hash(polynomial[binary_section]) == expected_binary_hash

    // This means we need to make expected_binary_hash available
    // to the constraint polynomial as a CONSTANT
}
```

**How to do this in Ligerito?**

Ligerito proves a polynomial evaluation. We need to encode:
```
constraint_poly(x) = (binary_hash(poly) - expected_hash) * randomness(x)
```

If `binary_hash(poly) == expected_hash`, then `constraint_poly` is identically zero.

**Actually... this is getting circular.**

**REAL REAL SOLUTION**: Two-phase verification!

```rust
pub fn verify_polkavm_proof(
    binary: &[u8],           // The actual binary (public)
    input_data: &[u8],       // Initial state (public)
    output_data: &[u8],      // Claimed output (public)
    proof: &LigeritoProof,   // The proof
) -> bool {
    // Phase 1: Check the proof is for THIS specific binary
    // The proof commits to a polynomial that INCLUDES the binary
    // We can check the Merkle tree commitment includes the right binary

    // Phase 2: Verify Ligerito proof
    verify(&verifier_config, proof)
}
```

**But how do we check "the polynomial includes the right binary"?**

**SOLUTION**: Make the binary part of the QUERIES!

```rust
// During verification:
// 1. Verifier randomly queries positions in the polynomial
// 2. Some queries land in the binary section
// 3. Verifier checks: revealed_value == binary[query_position]
// 4. If prover used fake binary, this check fails with high probability!

// This is similar to FRI queries - checking consistency
```

**Hmm, but Ligerito doesn't expose individual polynomial values to the verifier...**

**ACTUAL SOLUTION**: The binary becomes part of the **witness**, and we add it to public inputs properly:

```rust
pub fn prove_polkavm_execution(
    binary: &[u8],
    trace: &ExecutionTrace,
) -> PolkaVMProof {
    // 1. Hash the binary
    let binary_hash = sha256(binary);

    // 2. Build polynomial with:
    //    - Binary data
    //    - Execution trace
    //    - Constraints linking them
    let poly = arithmetize_with_hash_check(binary, trace, binary_hash);

    // 3. Prove the polynomial
    let ligerito_proof = ligerito::prove(&config, &poly)?;

    // 4. Return proof + public data
    PolkaVMProof {
        binary_hash,        // Verifier will check this
        initial_state,
        final_state,
        ligerito_proof,
    }
}

pub fn arithmetize_with_hash_check(
    binary: &[u8],
    trace: &ExecutionTrace,
    expected_hash: Hash,
) -> Vec<BinaryElem32> {
    let mut poly = vec![];

    // Section 1: Binary
    for byte in binary {
        poly.push(BinaryElem32::from(*byte as u32));
    }

    // Section 2: Hash constraint
    // Compute hash of binary section
    let computed_hash = hash_polynomial_section(&poly, 0, binary.len());

    // Add constraint: computed_hash == expected_hash
    for i in 0..32 {  // 32 bytes in hash
        let expected_byte = expected_hash[i];
        let computed_byte = computed_hash[i];
        let constraint = if expected_byte == computed_byte { 1 } else { 0 };
        poly.push(BinaryElem32::from(constraint));
    }

    // Section 3: Execution trace with constraints
    // ... (as before)

    poly
}
```

**Verdict**: ⚠️ **NEEDS CAREFUL DESIGN**

The binary verification requires either:
1. Hash checking inside the polynomial (complex)
2. Making binary queryable during verification (requires Ligerito modification)
3. Trust the prover provided the right binary (UNSAFE!)

---

## The REAL Problem: Ligerito's Abstraction Level

Ligerito proves: "I know a polynomial P such that [properties hold]"

But it doesn't expose:
- Individual polynomial values
- Structured public inputs/outputs
- Application-level constraints

**We need to add a layer above Ligerito that provides:**

```rust
pub struct StructuredProof {
    pub public_inputs: Vec<Field>,   // Binary hash, initial state
    pub public_outputs: Vec<Field>,  // Final state, output data
    pub polynomial_commitment: Hash,  // Ligerito's commitment
    pub ligerito_proof: LigeritoProof,
}
```

And the verifier checks:
1. Public inputs/outputs match expectations
2. Ligerito proof is valid
3. The polynomial STRUCTURE includes binary at specific positions

---

## Summary: What We Need

1. ✅ **PC Continuity**: Prevents skipping instructions
2. ✅ **ALU Correctness**: Prevents wrong computation
3. ⚠️ **Memory Consistency**: Needs permutation argument OR full snapshots
4. ✅ **Initial/Final State**: As public inputs
5. ❌ **Binary Authenticity**: **CRITICAL UNSOLVED PROBLEM**

**The binary authenticity problem is REAL** and needs one of:
- Modify Ligerito to expose polynomial structure
- Add hash checking constraints inside polynomial
- Use a different proof system with better public input support
- Accept that binary is trusted (unacceptable for zkVM!)

**This is actually a fundamental limitation we need to solve!**
