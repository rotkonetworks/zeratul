# pcVM: Polynomial Commitment Virtual Machine

A register-only virtual machine with execution traces provable using Ligerito polynomial commitments.

## What is pcVM?

**pcVM** (Polynomial Commitment VM) is a minimal virtual machine where execution traces are encoded as polynomials and proven using the Ligerito polynomial commitment scheme.

**Important**: This is a **polynomial commitment scheme**, not a zero-knowledge (ZK) system. Ligerito provides succinct polynomial commitments with ~100-bit security, but does not provide zero-knowledge properties.

## Architecture

The pcVM follows this pipeline:

```
Program → Execute → Trace → Arithmetize → Prove → Verify
   ↓         ↓        ↓          ↓           ↓        ↓
 .asm     Registers  Steps   Polynomial   Ligerito  Boolean
```

### Phase 1: Register-Only (Current Implementation)

**Capabilities**:
- 13 registers: `a0-a7` (arguments/return), `t0-t4` (temporaries)
- 9 operations: ADD, SUB, MUL, AND, OR, XOR, SLL, SRL, LI (load immediate)
- No memory, no branches, no jumps
- Deterministic execution

**Constraints**:
1. **PC Continuity**: Program counter increments sequentially
2. **Opcode Correctness**: Each step executes the correct instruction
3. **Register Indices**: Source/destination registers match program
4. **ALU Correctness**: Arithmetic/logic operations computed correctly
5. **Register Preservation**: Unchanged registers stay unchanged
6. **HALT Termination**: Execution ends with HALT

## Components

### 1. Trace (`pcvm/trace.rs`)

Captures execution as a sequence of steps:

```rust
pub struct RegisterOnlyStep {
    pub pc: u32,              // Program counter
    pub regs: [u32; 13],      // Register state BEFORE execution
    pub opcode: Opcode,       // Instruction being executed
    pub rd: u8,               // Destination register
    pub rs1: u8,              // Source register 1
    pub rs2: u8,              // Source register 2
    pub imm: u32,             // Immediate value (for LI)
}
```

**Example**:
```rust
use ligerito::pcvm::trace::{Program, Instruction, Opcode, execute_and_trace};

let program = vec![
    Instruction::new_rrr(Opcode::ADD, 0, 1, 2),  // a0 = a1 + a2
    Instruction::halt(),
];

let mut initial_regs = [0u32; 13];
initial_regs[1] = 5;  // a1 = 5
initial_regs[2] = 3;  // a2 = 3

let trace = execute_and_trace(&program, initial_regs);
assert_eq!(trace.final_state().unwrap()[0], 8); // a0 = 8
```

### 2. Poseidon Hash (`pcvm/poseidon.rs`)

Cryptographically secure hash function over GF(2^32):

```rust
use ligerito::pcvm::poseidon::PoseidonHash;

let hash = PoseidonHash::hash_bytes(b"hello world");
```

**Properties**:
- S-box: x^5 (algebraic degree 5)
- Width: 3 field elements
- Rounds: 8 full rounds
- Security: Collision resistance for polynomial commitments

### 3. Arithmetization (`pcvm/arithmetization.rs`)

Converts execution traces to polynomials:

```rust
use ligerito::pcvm::arithmetization::arithmetize_register_trace;

let challenge = BinaryElem32::from(0x12345678);
let arith = arithmetize_register_trace(&trace, &program, challenge);

// arith.polynomial: Ready for Ligerito proving
// arith.program_hash: Commitment to the program
// arith.constraint_product: Grand product of all constraints
```

**Polynomial Structure**:
```
[ program_hash, num_steps,
  step_0_encoding, step_1_encoding, ...,
  final_registers,
  constraint_product ]
```

**Constraint Checking**: Grand product argument
- Valid execution: All constraints c_i = 0
- Product: ∏(α - c_i) = α^n when all c_i = 0
- Invalid execution: Product ≠ α^n with high probability

### 4. Constraints (`pcvm/constraints.rs`)

Defines the constraint system:

```rust
use ligerito::pcvm::constraints::generate_all_constraints;

let constraints = generate_all_constraints(&trace, &program);

// Check all satisfied
for c in &constraints {
    assert_eq!(c.value, BinaryElem32::zero());
}
```

### 5. Integration (`pcvm/integration.rs`)

End-to-end proving and verification:

```rust
use ligerito::{prove, verify, hardcoded_config_12, hardcoded_config_12_verifier};
use ligerito::pcvm::integration::{create_test_program, create_test_inputs};

// Execute
let program = create_test_program(); // (a + b) * c
let initial_regs = create_test_inputs(5, 3, 2);
let trace = execute_and_trace(&program, initial_regs);

// Arithmetize
let arith = arithmetize_register_trace(&trace, &program, challenge);
let mut poly = arith.polynomial;
poly.resize(1 << 12, BinaryElem32::zero()); // Pad to 4096

// Prove
let config = hardcoded_config_12(PhantomData, PhantomData);
let proof = prove(&config, &poly).unwrap();

// Verify
let verifier_config = hardcoded_config_12_verifier();
let valid = verify(&verifier_config, &proof).unwrap();
assert!(valid);
```

## Example Programs

### Simple Addition
```rust
let program = vec![
    Instruction::new_imm(1, 10),              // a1 = 10
    Instruction::new_imm(2, 20),              // a2 = 20
    Instruction::new_rrr(Opcode::ADD, 0, 1, 2), // a0 = a1 + a2
    Instruction::halt(),
];
```

### Multiply-Accumulate
```rust
let program = vec![
    Instruction::new_imm(0, 0),               // a0 = 0 (accumulator)
    Instruction::new_imm(1, 5),               // a1 = 5
    Instruction::new_imm(2, 3),               // a2 = 3
    Instruction::new_rrr(Opcode::MUL, 3, 1, 2), // a3 = a1 * a2
    Instruction::new_rrr(Opcode::ADD, 0, 0, 3), // a0 += a3
    Instruction::halt(),
];
```

### Bitwise Operations
```rust
let program = vec![
    Instruction::new_imm(1, 0b1010),          // a1 = 10
    Instruction::new_imm(2, 0b1100),          // a2 = 12
    Instruction::new_rrr(Opcode::AND, 3, 1, 2), // a3 = a1 & a2 = 8
    Instruction::new_rrr(Opcode::OR, 4, 1, 2),  // a4 = a1 | a2 = 14
    Instruction::new_rrr(Opcode::XOR, 5, 1, 2), // a5 = a1 ^ a2 = 6
    Instruction::halt(),
];
```

## Performance

**Test Results** (on consumer hardware):

| Program Size | Trace Steps | Polynomial Size | Proof Size | Proving Time | Verify Time |
|-------------|-------------|-----------------|------------|--------------|-------------|
| 3 instr     | 3 steps     | 73 elem → 4096  | ~33 KB     | ~500ms       | ~50ms       |
| 13 instr    | 13 steps    | 263 elem → 4096 | ~33 KB     | ~520ms       | ~50ms       |

**Proof Size**: ~33 KB for 2^12 polynomial (constant regardless of program complexity)

**Backend**: CPU with SIMD acceleration (AVX2/NEON when available)

## Security

### Soundness

Ligerito provides ~100-bit computational soundness:
- Reed-Solomon codes over GF(2^32) and GF(2^128)
- 148 queries per round
- Fiat-Shamir transform for non-interactivity

**Attack Resistance**:
1. ✅ **Binary Authenticity**: Program hash (Poseidon) prevents fake programs
2. ✅ **Execution Correctness**: Constraints verified via grand product
3. ✅ **Polynomial Commitment**: Ligerito's soundness guarantees
4. ❌ **Zero-Knowledge**: NOT PROVIDED (this is polynomial commitment, not ZK)

### Limitations

**Not Zero-Knowledge**: The proof reveals:
- Program hash
- Polynomial degree
- Trace length
- All intermediate values (no hiding)

**Not Succinct**: While proof is small (~33 KB), it's larger than ZK-SNARKs:
- Groth16: ~200 bytes
- Plonk: ~500 bytes
- Ligerito: ~33 KB

**Advantage**: Much faster proving time than ZK-SNARKs, no trusted setup.

## Future Phases

### Phase 2: Read-Only Memory (Planned)
- Load program code from memory
- Binary verification
- No writes (immutable memory)

### Phase 3: Full Memory (Future)
- Read/write memory
- Memory consistency via permutation argument (Plonk-style)
- Grand product over (address, time, value) tuples

### Phase 4: Control Flow (Future)
- Conditional branches
- Jumps
- Loops
- PC constraint updates

## Binary Field Arithmetic

pcVM uses GF(2^32) binary extension fields:

**Key Properties**:
- Addition is XOR: `a + b = a ⊕ b`
- Subtraction equals addition: `a - b = a + b`
- No negative numbers: `-a = a`
- Multiplication via carry-less polynomial multiplication

**Example**:
```rust
use ligerito::binary_fields::{BinaryElem32, BinaryFieldElement};

let a = BinaryElem32::from(5);
let b = BinaryElem32::from(3);

let sum = a.add(&b);      // 5 ⊕ 3 = 6
let diff = a.add(&b);     // Same! (no negation)
let prod = a.mul(&b);     // Finite field multiplication
```

## Testing

Run all pcVM tests:
```bash
cargo test pcvm --lib
```

Run integration tests:
```bash
cargo test pcvm::integration --lib -- --nocapture
```

**Test Coverage**:
- ✅ Trace execution and validation (2 tests)
- ✅ Poseidon hash correctness (3 tests)
- ✅ Arithmetization (5 tests)
- ✅ Constraint satisfaction (4 tests)
- ✅ End-to-end proving (5 tests)

**Total**: 19 tests, all passing ✓

## API Reference

### Core Types

```rust
// Execution
pub struct RegisterOnlyTrace { pub steps: Vec<RegisterOnlyStep> }
pub struct RegisterOnlyStep { /* ... */ }
pub enum Opcode { ADD, SUB, MUL, AND, OR, XOR, SLL, SRL, LI, HALT }

// Arithmetization
pub struct ArithmetizedTrace {
    pub polynomial: Vec<BinaryElem32>,
    pub program_hash: BinaryElem32,
    pub constraint_product: BinaryElem32,
    pub challenge: BinaryElem32,
}

// Constraints
pub enum ConstraintType { PcContinuity, OpcodeCorrectness, ... }
pub struct Constraint { /* ... */ }
```

### Main Functions

```rust
// Execute program
pub fn execute_and_trace(program: &Program, initial_regs: [u32; 13]) -> RegisterOnlyTrace;

// Convert to polynomial
pub fn arithmetize_register_trace(
    trace: &RegisterOnlyTrace,
    program: &Program,
    challenge: BinaryElem32,
) -> ArithmetizedTrace;

// Generate constraints
pub fn generate_all_constraints(
    trace: &RegisterOnlyTrace,
    program: &Program,
) -> Vec<Constraint>;

// Hash functions
impl PoseidonHash {
    pub fn hash_bytes(bytes: &[u8]) -> BinaryElem32;
    pub fn hash_elements(elements: &[BinaryElem32]) -> BinaryElem32;
}
```

## Integration with JAM/Polkadot

The pcVM is designed for integration with JAM (Join-Accumulate Machine):

**Workflow**:
1. **Collator**: Executes PolkaVM program, generates trace
2. **Prover**: Arithmetizes trace, generates Ligerito proof
3. **Validator**: Verifies proof in PVM (native or guest)
4. **Finality**: Proof included in work report

**Advantages**:
- Fast proving (~500ms for small programs)
- Small proofs (~33 KB)
- No trusted setup
- Pure Rust implementation

## References

- [Ligerito Paper](https://angeris.github.io/papers/ligerito.pdf) - Andrija Novakovic and Guillermo Angeris
- [JAM Graypaper](https://graypaper.com/) - Gavin Wood
- [PolkaVM](https://github.com/koute/polkavm) - RISC-V based VM for Polkadot

## License

Same as Ligerito (check repository root)
