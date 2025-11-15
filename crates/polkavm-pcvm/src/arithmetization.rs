//! Arithmetization: Convert execution traces to polynomials
//!
//! This module converts a RegisterOnlyTrace into a polynomial that can be
//! proven with Ligerito. The polynomial encodes:
//! 1. Program hash (using Poseidon)
//! 2. All execution steps
//! 3. Constraints (via grand product argument)

use ligerito_binary_fields::{BinaryElem32, BinaryFieldElement};
use super::trace::{RegisterOnlyTrace, RegisterOnlyStep, Opcode, Program};
use super::poseidon::PoseidonHash;

#[cfg(not(feature = "std"))]
use alloc::vec::Vec;

/// Result of arithmetization: polynomial ready for Ligerito proving
#[derive(Debug, Clone)]
pub struct ArithmetizedTrace {
    /// The polynomial encoding the entire computation
    pub polynomial: Vec<BinaryElem32>,

    /// Program hash (for verification)
    pub program_hash: BinaryElem32,

    /// Constraint product (should equal challenge^num_constraints for valid trace)
    pub constraint_product: BinaryElem32,

    /// Challenge used for constraint checking
    pub challenge: BinaryElem32,
}

/// Convert a register-only trace to a polynomial
pub fn arithmetize_register_trace(
    trace: &RegisterOnlyTrace,
    program: &Program,
    challenge: BinaryElem32,
) -> ArithmetizedTrace {
    let mut poly = Vec::new();

    // Step 1: Compute and encode program hash
    let program_hash = hash_program(program);
    poly.push(program_hash);

    // Step 2: Encode number of steps
    poly.push(BinaryElem32::from(trace.steps.len() as u32));

    // Step 3: Encode each execution step
    let mut constraints = Vec::new();

    for (i, step) in trace.steps.iter().enumerate() {
        // Encode step state
        encode_step(&mut poly, step);

        // Generate constraints for this step
        generate_step_constraints(&mut constraints, step, program, i);
    }

    // Step 4: Encode final register state
    if let Some(final_regs) = trace.final_state() {
        for &reg in &final_regs {
            poly.push(BinaryElem32::from(reg));
        }
    }

    // Step 5: Apply grand product argument to all constraints
    let constraint_product = compute_constraint_product(&constraints, challenge);
    poly.push(constraint_product);

    ArithmetizedTrace {
        polynomial: poly,
        program_hash,
        constraint_product,
        challenge,
    }
}

/// Hash a program using Poseidon
fn hash_program(program: &Program) -> BinaryElem32 {
    let mut elements = Vec::new();

    for instr in program {
        // Encode each instruction as field elements
        elements.push(BinaryElem32::from(instr.opcode as u8 as u32));
        elements.push(BinaryElem32::from(instr.rd as u32));
        elements.push(BinaryElem32::from(instr.rs1 as u32));
        elements.push(BinaryElem32::from(instr.rs2 as u32));
        elements.push(BinaryElem32::from(instr.imm));
    }

    PoseidonHash::hash_elements(&elements)
}

/// Encode a single execution step into the polynomial
fn encode_step(poly: &mut Vec<BinaryElem32>, step: &RegisterOnlyStep) {
    // PC
    poly.push(BinaryElem32::from(step.pc));

    // Opcode
    poly.push(BinaryElem32::from(step.opcode as u8 as u32));

    // Register indices
    poly.push(BinaryElem32::from(step.rd as u32));
    poly.push(BinaryElem32::from(step.rs1 as u32));
    poly.push(BinaryElem32::from(step.rs2 as u32));

    // Immediate value
    poly.push(BinaryElem32::from(step.imm));

    // All register values BEFORE execution
    for &reg in &step.regs {
        poly.push(BinaryElem32::from(reg));
    }
}

/// Generate constraints for a single execution step
fn generate_step_constraints(
    constraints: &mut Vec<BinaryElem32>,
    step: &RegisterOnlyStep,
    program: &Program,
    step_index: usize,
) {
    // Constraint 1: PC matches step index
    // In GF(2^32), addition is XOR, so a - b = a + b
    let pc_constraint = BinaryElem32::from(step.pc)
        .add(&BinaryElem32::from(step_index as u32));
    constraints.push(pc_constraint);

    // Constraint 2: Opcode matches program
    if step_index < program.len() {
        let expected_opcode = BinaryElem32::from(program[step_index].opcode as u8 as u32);
        let actual_opcode = BinaryElem32::from(step.opcode as u8 as u32);
        let opcode_constraint = expected_opcode.add(&actual_opcode);
        constraints.push(opcode_constraint);
    }

    // Constraint 3: Register indices match program
    if step_index < program.len() {
        let instr = &program[step_index];

        let rd_constraint = BinaryElem32::from(instr.rd as u32)
            .add(&BinaryElem32::from(step.rd as u32));
        constraints.push(rd_constraint);

        let rs1_constraint = BinaryElem32::from(instr.rs1 as u32)
            .add(&BinaryElem32::from(step.rs1 as u32));
        constraints.push(rs1_constraint);

        let rs2_constraint = BinaryElem32::from(instr.rs2 as u32)
            .add(&BinaryElem32::from(step.rs2 as u32));
        constraints.push(rs2_constraint);
    }

    // Constraint 4: ALU correctness
    let alu_constraint = check_alu_correctness(step);
    constraints.push(alu_constraint);
}

/// Check that the ALU operation was performed correctly
fn check_alu_correctness(step: &RegisterOnlyStep) -> BinaryElem32 {
    let expected_result = match step.opcode {
        Opcode::ADD => step.regs[step.rs1 as usize].wrapping_add(step.regs[step.rs2 as usize]),
        Opcode::SUB => step.regs[step.rs1 as usize].wrapping_sub(step.regs[step.rs2 as usize]),
        Opcode::MUL => step.regs[step.rs1 as usize].wrapping_mul(step.regs[step.rs2 as usize]),
        Opcode::AND => step.regs[step.rs1 as usize] & step.regs[step.rs2 as usize],
        Opcode::OR  => step.regs[step.rs1 as usize] | step.regs[step.rs2 as usize],
        Opcode::XOR => step.regs[step.rs1 as usize] ^ step.regs[step.rs2 as usize],
        Opcode::SLL => step.regs[step.rs1 as usize] << (step.regs[step.rs2 as usize] & 0x1F),
        Opcode::SRL => step.regs[step.rs1 as usize] >> (step.regs[step.rs2 as usize] & 0x1F),
        Opcode::LI  => step.imm,
        Opcode::LOAD => step.memory_value.unwrap_or(0), // Value already fetched
        Opcode::HALT => return BinaryElem32::zero(), // HALT doesn't modify registers
    };

    // Get actual result (what would be in rd after execution)
    let new_regs = step.execute();
    let actual_result = new_regs[step.rd as usize];

    // Constraint: expected XOR actual should be zero
    BinaryElem32::from(expected_result).add(&BinaryElem32::from(actual_result))
}

/// Compute the grand product of all constraints
///
/// For valid execution, all constraints should be 0.
/// The product ∏(α - c_i) equals α^n when all c_i = 0.
fn compute_constraint_product(
    constraints: &[BinaryElem32],
    challenge: BinaryElem32,
) -> BinaryElem32 {
    let mut product = BinaryElem32::one();

    for constraint in constraints {
        // Compute (challenge - constraint) = (challenge + constraint) in GF(2^32)
        let term = challenge.add(constraint);
        product = product.mul(&term);
    }

    product
}

/// Verify that a polynomial represents a valid execution
pub fn verify_arithmetization(
    arith: &ArithmetizedTrace,
    num_constraints: usize,
) -> bool {
    // Expected product when all constraints are zero: challenge^num_constraints
    let expected_product = arith.challenge.pow(num_constraints as u64);

    arith.constraint_product == expected_product
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::trace::{execute_and_trace, Instruction};

    #[test]
    fn test_simple_arithmetization() {
        // Program: a0 = a1 + a2, then HALT
        let program = vec![
            Instruction::new_rrr(Opcode::ADD, 0, 1, 2),
            Instruction::halt(),
        ];

        // Initial state: a1=5, a2=3
        let mut initial = [0u32; 13];
        initial[1] = 5;
        initial[2] = 3;

        let trace = execute_and_trace(&program, initial);
        assert!(trace.validate().is_ok());

        // Arithmetize with a random challenge
        let challenge = BinaryElem32::from(0x12345678);
        let arith = arithmetize_register_trace(&trace, &program, challenge);

        // Polynomial should be non-empty
        assert!(!arith.polynomial.is_empty());

        // Program hash should be deterministic
        let program_hash2 = hash_program(&program);
        assert_eq!(arith.program_hash, program_hash2);
    }

    #[test]
    fn test_constraint_validation() {
        // Program: a0 = (a1 + a2) * a3
        let program = vec![
            Instruction::new_rrr(Opcode::ADD, 0, 1, 2),  // a0 = a1 + a2
            Instruction::new_rrr(Opcode::MUL, 0, 0, 3),  // a0 = a0 * a3
            Instruction::halt(),
        ];

        // Initial: a1=5, a2=3, a3=2
        let mut initial = [0u32; 13];
        initial[1] = 5;
        initial[2] = 3;
        initial[3] = 2;

        let trace = execute_and_trace(&program, initial);

        let challenge = BinaryElem32::from(0xdeadbeef);
        let arith = arithmetize_register_trace(&trace, &program, challenge);

        // For a valid trace, constraints should verify
        // Note: We need to count the actual number of constraints generated
        // Each step generates: 1 (PC) + 1 (opcode) + 3 (reg indices) + 1 (ALU) = 6 constraints
        // 3 steps (ADD, MUL, HALT) = 18 constraints
        let num_constraints = trace.steps.len() * 6;

        assert!(verify_arithmetization(&arith, num_constraints));
    }

    #[test]
    fn test_program_hash_collision_resistance() {
        // Different programs should have different hashes
        let program1 = vec![
            Instruction::new_rrr(Opcode::ADD, 0, 1, 2),
            Instruction::halt(),
        ];

        let program2 = vec![
            Instruction::new_rrr(Opcode::SUB, 0, 1, 2),
            Instruction::halt(),
        ];

        let hash1 = hash_program(&program1);
        let hash2 = hash_program(&program2);

        assert_ne!(hash1, hash2);
    }

    #[test]
    fn test_alu_correctness_constraint() {
        // Create a step with ADD operation
        let step = RegisterOnlyStep {
            pc: 0,
            regs: {
                let mut regs = [0u32; 13];
                regs[1] = 10;
                regs[2] = 20;
                regs
            },
            opcode: Opcode::ADD,
            rd: 0,
            rs1: 1,
            rs2: 2,
            imm: 0,
            memory_address: None,
            memory_value: None,
        };

        // ALU constraint should be zero (correct execution)
        let constraint = check_alu_correctness(&step);
        assert_eq!(constraint, BinaryElem32::zero());
    }

    #[test]
    fn test_grand_product_zero_constraints() {
        // All constraints are zero
        let constraints = vec![
            BinaryElem32::zero(),
            BinaryElem32::zero(),
            BinaryElem32::zero(),
        ];

        let challenge = BinaryElem32::from(0x42);
        let product = compute_constraint_product(&constraints, challenge);

        // Product should equal challenge^3
        let expected = challenge.pow(3);
        assert_eq!(product, expected);
    }

    #[test]
    fn test_grand_product_nonzero_constraint() {
        // One constraint is non-zero
        let constraints = vec![
            BinaryElem32::zero(),
            BinaryElem32::from(1),  // Non-zero!
            BinaryElem32::zero(),
        ];

        let challenge = BinaryElem32::from(0x42);
        let product = compute_constraint_product(&constraints, challenge);

        // Product should NOT equal challenge^3
        let expected = challenge.pow(3);
        assert_ne!(product, expected);
    }
}
