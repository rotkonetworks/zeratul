//! Constraint generation for PolkaVM instructions
//!
//! This module generates polynomial constraints that prove correct execution
//! of PolkaVM instructions. Each instruction type has specific constraints
//! that must be satisfied for the execution to be valid.
//!
//! Constraints are expressed over binary extension field GF(2^32), where:
//! - Addition is XOR: a + b = a XOR b
//! - Subtraction is also XOR: a - b = a XOR b
//! - Multiplication uses carryless multiplication
//!
//! All constraints follow the pattern: expected_value XOR actual_value = 0

use crate::polkavm_adapter::{PolkaVMRegisters, PolkaVMStep, MemoryAccess, MemoryAccessSize};
use ligerito_binary_fields::{BinaryElem32, BinaryFieldElement};

#[cfg(feature = "polkavm-integration")]
use polkavm::program::Instruction;

/// Constraint violation - when a constraint is not satisfied
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConstraintViolation {
    pub step_index: usize,
    pub constraint_type: ConstraintType,
    pub expected: u32,
    pub actual: u32,
    pub message: String,
}

/// Types of constraints in the PolkaVM execution model
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConstraintType {
    /// ALU operation correctness (arithmetic, logic, shifts)
    AluCorrectness,
    /// Memory address computation
    MemoryAddress,
    /// Memory bounds checking
    MemoryBounds,
    /// Control flow (PC continuity)
    ControlFlow,
    /// Register state consistency
    RegisterConsistency,
}

/// Generate all constraints for a single PolkaVM execution step
///
/// Returns a vector of field elements where each element should be 0
/// if the constraint is satisfied. Non-zero values indicate violations.
#[cfg(feature = "polkavm-integration")]
pub fn generate_step_constraints(
    step: &PolkaVMStep,
    instruction: &Instruction,
) -> Vec<BinaryElem32> {
    use polkavm::program::Instruction::*;

    match instruction {
        // Arithmetic operations (core 3)
        add_32(dst, src1, src2) => generate_add_32_constraint(step, *dst, *src1, *src2),
        sub_32(dst, src1, src2) => generate_sub_32_constraint(step, *dst, *src1, *src2),
        mul_32(dst, src1, src2) => generate_mul_32_constraint(step, *dst, *src1, *src2),

        // Memory operations (core 2)
        load_indirect_u32(dst, base, offset) => {
            generate_load_u32_constraint(step, *dst, *base, *offset)
        }
        store_indirect_u32(src, base, offset) => {
            generate_store_u32_constraint(step, *src, *base, *offset)
        }

        // Control flow (core 2)
        jump(target) => generate_jump_constraint(step, *target as u32),
        branch_eq(src1, src2, target) => {
            generate_branch_eq_constraint(step, *src1, *src2, *target)
        }

        // Data movement (core 1)
        load_imm(dst, imm) => generate_load_imm_constraint(step, *dst, *imm),

        // System (core 1)
        trap => generate_trap_constraint(step),

        // Other instructions - to be implemented
        _ => vec![BinaryElem32::zero()],
    }
}

/// Constraint for ADD_32: dst = src1 + src2 (in GF(2^32), + is XOR)
#[cfg(feature = "polkavm-integration")]
fn generate_add_32_constraint(
    step: &PolkaVMStep,
    dst: polkavm_common::program::RawReg,
    src1: polkavm_common::program::RawReg,
    src2: polkavm_common::program::RawReg,
) -> Vec<BinaryElem32> {
    let regs_before = step.regs_before.to_array();
    let regs_after = step.regs_after.to_array();

    let dst_idx = dst.get() as usize;
    let src1_idx = src1.get() as usize;
    let src2_idx = src2.get() as usize;

    // In normal arithmetic: expected = src1 + src2
    let expected = regs_before[src1_idx].wrapping_add(regs_before[src2_idx]);
    let actual = regs_after[dst_idx];

    // Constraint: expected XOR actual = 0 (in GF(2^32))
    let constraint = BinaryElem32::from(expected ^ actual);

    // Also check that non-dst registers remain unchanged
    let mut constraints = vec![constraint];
    constraints.extend(generate_register_consistency_constraints(step, dst_idx));

    constraints
}

/// Constraint for SUB_32: dst = src1 - src2
#[cfg(feature = "polkavm-integration")]
fn generate_sub_32_constraint(
    step: &PolkaVMStep,
    dst: polkavm_common::program::RawReg,
    src1: polkavm_common::program::RawReg,
    src2: polkavm_common::program::RawReg,
) -> Vec<BinaryElem32> {
    let regs_before = step.regs_before.to_array();
    let regs_after = step.regs_after.to_array();

    let dst_idx = dst.get() as usize;
    let src1_idx = src1.get() as usize;
    let src2_idx = src2.get() as usize;

    // In normal arithmetic: expected = src1 - src2
    let expected = regs_before[src1_idx].wrapping_sub(regs_before[src2_idx]);
    let actual = regs_after[dst_idx];

    let constraint = BinaryElem32::from(expected ^ actual);

    let mut constraints = vec![constraint];
    constraints.extend(generate_register_consistency_constraints(step, dst_idx));

    constraints
}

/// Constraint for MUL_32: dst = src1 * src2
#[cfg(feature = "polkavm-integration")]
fn generate_mul_32_constraint(
    step: &PolkaVMStep,
    dst: polkavm_common::program::RawReg,
    src1: polkavm_common::program::RawReg,
    src2: polkavm_common::program::RawReg,
) -> Vec<BinaryElem32> {
    let regs_before = step.regs_before.to_array();
    let regs_after = step.regs_after.to_array();

    let dst_idx = dst.get() as usize;
    let src1_idx = src1.get() as usize;
    let src2_idx = src2.get() as usize;

    // In normal arithmetic: expected = src1 * src2
    let expected = regs_before[src1_idx].wrapping_mul(regs_before[src2_idx]);
    let actual = regs_after[dst_idx];

    let constraint = BinaryElem32::from(expected ^ actual);

    let mut constraints = vec![constraint];
    constraints.extend(generate_register_consistency_constraints(step, dst_idx));

    constraints
}

/// Constraint for LOAD_IMM: dst = immediate
#[cfg(feature = "polkavm-integration")]
fn generate_load_imm_constraint(
    step: &PolkaVMStep,
    dst: polkavm_common::program::RawReg,
    imm: u32,
) -> Vec<BinaryElem32> {
    let regs_after = step.regs_after.to_array();
    let dst_idx = dst.get() as usize;

    let expected = imm;
    let actual = regs_after[dst_idx];

    let constraint = BinaryElem32::from(expected ^ actual);

    let mut constraints = vec![constraint];
    constraints.extend(generate_register_consistency_constraints(step, dst_idx));

    constraints
}

/// Constraint for LOAD_INDIRECT_U32: dst = mem[base + offset]
#[cfg(feature = "polkavm-integration")]
fn generate_load_u32_constraint(
    step: &PolkaVMStep,
    dst: polkavm_common::program::RawReg,
    base: polkavm_common::program::RawReg,
    offset: u32,
) -> Vec<BinaryElem32> {
    let regs_before = step.regs_before.to_array();
    let regs_after = step.regs_after.to_array();

    let dst_idx = dst.get() as usize;
    let base_idx = base.get() as usize;

    // Address computation constraint
    let expected_address = regs_before[base_idx].wrapping_add(offset);

    let mut constraints = vec![];

    // Check memory access was recorded
    if let Some(ref mem_access) = step.memory_access {
        // Address must match
        let address_constraint = BinaryElem32::from(expected_address ^ mem_access.address);
        constraints.push(address_constraint);

        // Must be a read operation
        if mem_access.is_write {
            constraints.push(BinaryElem32::one()); // Violation: should be read
        }

        // Size must be Word (u32)
        if mem_access.size != MemoryAccessSize::Word {
            constraints.push(BinaryElem32::one()); // Violation: wrong size
        }
    } else {
        // No memory access recorded - violation
        constraints.push(BinaryElem32::one());
    }

    constraints.extend(generate_register_consistency_constraints(step, dst_idx));

    constraints
}

/// Constraint for STORE_INDIRECT_U32: mem[base + offset] = src
#[cfg(feature = "polkavm-integration")]
fn generate_store_u32_constraint(
    step: &PolkaVMStep,
    src: polkavm_common::program::RawReg,
    base: polkavm_common::program::RawReg,
    offset: u32,
) -> Vec<BinaryElem32> {
    let regs_before = step.regs_before.to_array();
    let base_idx = base.get() as usize;
    let src_idx = src.get() as usize;

    // Address computation constraint
    let expected_address = regs_before[base_idx].wrapping_add(offset);
    let expected_value = regs_before[src_idx];

    let mut constraints = vec![];

    // Check memory access was recorded
    if let Some(ref mem_access) = step.memory_access {
        // Address must match
        let address_constraint = BinaryElem32::from(expected_address ^ mem_access.address);
        constraints.push(address_constraint);

        // Value must match
        let value_constraint = BinaryElem32::from(expected_value ^ mem_access.value);
        constraints.push(value_constraint);

        // Must be a write operation
        if !mem_access.is_write {
            constraints.push(BinaryElem32::one()); // Violation: should be write
        }

        // Size must be Word (u32)
        if mem_access.size != MemoryAccessSize::Word {
            constraints.push(BinaryElem32::one()); // Violation: wrong size
        }
    } else {
        // No memory access recorded - violation
        constraints.push(BinaryElem32::one());
    }

    // Store doesn't modify registers (except PC)
    constraints.extend(generate_register_consistency_constraints(step, 13)); // No register changed

    constraints
}

/// Constraint for JUMP: PC = target
#[cfg(feature = "polkavm-integration")]
fn generate_jump_constraint(
    step: &PolkaVMStep,
    _target: u32,
) -> Vec<BinaryElem32> {
    // Jump changes PC but no registers
    // PC continuity will be checked separately in control flow constraints

    // All registers should remain unchanged
    generate_register_consistency_constraints(step, 13) // 13 means no register changed
}

/// Constraint for BRANCH_EQ: if src1 == src2 then PC = target else PC = PC + instruction_size
#[cfg(feature = "polkavm-integration")]
fn generate_branch_eq_constraint(
    step: &PolkaVMStep,
    _src1: polkavm_common::program::RawReg,
    _src2: polkavm_common::program::RawReg,
    _target: u32,
) -> Vec<BinaryElem32> {

    // Branch doesn't modify registers (only PC)
    // The branch condition will be verified by checking PC continuity

    // All registers should remain unchanged
    generate_register_consistency_constraints(step, 13) // 13 means no register changed
}

/// Constraint for TRAP: execution should halt
#[cfg(feature = "polkavm-integration")]
fn generate_trap_constraint(step: &PolkaVMStep) -> Vec<BinaryElem32> {
    // Trap doesn't modify registers
    // Trap causes execution to halt (verified by trace extraction)

    generate_register_consistency_constraints(step, 13) // 13 means no register changed
}

/// Generate constraints that verify all non-modified registers remain unchanged
///
/// dst_idx: the index of the register that was modified (0-12)
///          If dst_idx = 13, no register was modified (for stores, branches, etc.)
fn generate_register_consistency_constraints(
    step: &PolkaVMStep,
    dst_idx: usize,
) -> Vec<BinaryElem32> {
    let regs_before = step.regs_before.to_array();
    let regs_after = step.regs_after.to_array();

    let mut constraints = Vec::new();

    // Check all registers except dst_idx
    for i in 0..13 {
        if i != dst_idx {
            let constraint = BinaryElem32::from(regs_before[i] ^ regs_after[i]);
            constraints.push(constraint);
        }
    }

    constraints
}

/// Verify all constraints for a single step
///
/// Returns Ok(()) if all constraints are satisfied, or Err with violations
#[cfg(feature = "polkavm-integration")]
pub fn verify_step_constraints(
    step: &PolkaVMStep,
    instruction: &Instruction,
    step_index: usize,
) -> Result<(), Vec<ConstraintViolation>> {
    let constraints = generate_step_constraints(step, instruction);

    let mut violations = Vec::new();

    for (i, constraint) in constraints.iter().enumerate() {
        if *constraint != BinaryElem32::zero() {
            // Extract value by converting to u32
            let value = constraint.poly().value();
            violations.push(ConstraintViolation {
                step_index,
                constraint_type: ConstraintType::AluCorrectness, // TODO: track constraint type
                expected: 0,
                actual: value,
                message: format!("Constraint {} failed at step {}", i, step_index),
            });
        }
    }

    if violations.is_empty() {
        Ok(())
    } else {
        Err(violations)
    }
}

#[cfg(all(test, feature = "polkavm-integration"))]
mod tests {
    use super::*;
    use crate::polkavm_adapter::{PolkaVMRegisters, MemoryAccess, MemoryAccessSize};
    use polkavm::program::Instruction;
    use polkavm_common::program::{RawReg, Reg};

    fn make_test_step(
        regs_before: [u32; 13],
        regs_after: [u32; 13],
        memory_access: Option<MemoryAccess>,
    ) -> PolkaVMStep {
        PolkaVMStep {
            pc: 0,
            regs_before: PolkaVMRegisters::from_array(regs_before),
            regs_after: PolkaVMRegisters::from_array(regs_after),
            opcode: 0,
            operands: [0, 0, 0],
            memory_access,
        }
    }

    fn raw_reg(r: Reg) -> RawReg {
        RawReg::from(r)
    }

    #[test]
    fn test_add_32_constraint_valid() {
        let mut regs_before = [0u32; 13];
        regs_before[2] = 10; // T0 (Reg::T0 = 2)
        regs_before[3] = 20; // T1 (Reg::T1 = 3)

        let mut regs_after = regs_before;
        regs_after[4] = 30; // T2 = 10 + 20 (Reg::T2 = 4)

        let step = make_test_step(regs_before, regs_after, None);
        let instruction = Instruction::add_32(
            raw_reg(Reg::T2),
            raw_reg(Reg::T0),
            raw_reg(Reg::T1),
        );

        let constraints = generate_step_constraints(&step, &instruction);

        // First constraint should be satisfied (30 XOR 30 = 0)
        assert_eq!(constraints[0], BinaryElem32::zero());
    }

    #[test]
    fn test_add_32_constraint_invalid() {
        let mut regs_before = [0u32; 13];
        regs_before[2] = 10; // T0 (Reg::T0 = 2)
        regs_before[3] = 20; // T1 (Reg::T1 = 3)

        let mut regs_after = regs_before;
        regs_after[4] = 999; // T2 = WRONG (should be 30, Reg::T2 = 4)

        let step = make_test_step(regs_before, regs_after, None);
        let instruction = Instruction::add_32(
            raw_reg(Reg::T2),
            raw_reg(Reg::T0),
            raw_reg(Reg::T1),
        );

        let constraints = generate_step_constraints(&step, &instruction);

        // First constraint should NOT be satisfied (30 XOR 999 != 0)
        assert_ne!(constraints[0], BinaryElem32::zero());
    }

    #[test]
    fn test_load_imm_constraint_valid() {
        let regs_before = [0u32; 13];

        let mut regs_after = regs_before;
        regs_after[5] = 42; // S0 = 42

        let step = make_test_step(regs_before, regs_after, None);
        let instruction = Instruction::load_imm(raw_reg(Reg::S0), 42);

        let constraints = generate_step_constraints(&step, &instruction);

        // Constraint should be satisfied (42 XOR 42 = 0)
        assert_eq!(constraints[0], BinaryElem32::zero());
    }

    #[test]
    fn test_register_consistency() {
        let mut regs_before = [0u32; 13];
        regs_before[1] = 10;
        regs_before[2] = 20;
        regs_before[3] = 30;

        let mut regs_after = regs_before;
        regs_after[3] = 999; // Only T2 changes
        regs_after[4] = 123; // ERROR: T3 also changed!

        let step = make_test_step(regs_before, regs_after, None);

        let constraints = generate_register_consistency_constraints(&step, 3);

        // Should have violations for all registers except T2 (index 3)
        // Register 4 (T3) changed, so its constraint should be non-zero
        // In constraints array: reg[0,1,2,4,5,...] (skipping reg[3])
        // So register 4 is at constraints[3]
        let t3_constraint = constraints[3];
        assert_ne!(t3_constraint, BinaryElem32::zero());
    }
}
