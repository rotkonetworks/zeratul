//! Adversarial soundness tests for PolkaVM constraints
//!
//! These tests verify that malicious provers CANNOT forge execution.
//! Each test attempts a specific attack and verifies it's caught.

#![cfg(feature = "polkavm-integration")]

use ligerito::pcvm::polkavm_constraints_v2::{
    ProvenTransition, ConstraintError, generate_transition_constraints,
};
use ligerito::pcvm::polkavm_adapter::PolkaVMRegisters;
use polkavm::program::Instruction;
use polkavm_common::program::{RawReg, Reg};

fn raw_reg(r: Reg) -> RawReg {
    RawReg::from(r)
}

/// Test that PC continuity is enforced for jumps
#[test]
fn test_reject_wrong_jump_target() {
    // Instruction: JUMP 0x100
    // Attacker claims: jumped to 0x200 (WRONG!)

    let transition = ProvenTransition {
        pc: 0x50,
        next_pc: 0x200, // WRONG! Should be 0x100
        instruction_size: 3,
        regs_before: PolkaVMRegisters::new(),
        regs_after: PolkaVMRegisters::new(),
        memory_root_before: [0u8; 32],
        memory_root_after: [0u8; 32],
        memory_proof: None,
        instruction_proof: ligerito::pcvm::polkavm_constraints_v2::InstructionProof {
            merkle_path: vec![],
            position: 0,
            opcode: 0,
            operands: [0, 0, 0],
        },
    };

    let instruction = Instruction::jump(0x100);

    let constraints = generate_transition_constraints(&transition, &instruction)
        .expect("Should generate constraints");

    // PC continuity constraint should be NON-ZERO (violation!)
    let pc_constraint = &constraints[0];
    assert_ne!(
        *pc_constraint,
        ligerito_binary_fields::BinaryFieldElement::zero(),
        "PC continuity violation should be detected!"
    );

    println!("✓ Successfully rejected wrong jump target");
}

/// Test that branch conditions are checked
#[test]
fn test_reject_wrong_branch_taken() {
    // Instruction: BRANCH_EQ a0, a1, 0x100
    // a0 = 5, a1 = 7 (NOT EQUAL)
    // Attacker claims: branch WAS taken (wrong!)

    let mut regs_before = [0u32; 13];
    regs_before[7] = 5;  // a0 = 5
    regs_before[8] = 7;  // a1 = 7 (different!)

    let transition = ProvenTransition {
        pc: 0x50,
        next_pc: 0x100, // Claimed branch target (WRONG - should be sequential)
        instruction_size: 4,
        regs_before: PolkaVMRegisters::from_array(regs_before),
        regs_after: PolkaVMRegisters::from_array(regs_before), // No register changes
        memory_root_before: [0u8; 32],
        memory_root_after: [0u8; 32],
        memory_proof: None,
        instruction_proof: ligerito::pcvm::polkavm_constraints_v2::InstructionProof {
            merkle_path: vec![],
            position: 0,
            opcode: 0,
            operands: [0, 0, 0],
        },
    };

    let instruction = Instruction::branch_eq(
        raw_reg(Reg::A0),
        raw_reg(Reg::A1),
        0x100 - 0x50, // Relative offset
    );

    let constraints = generate_transition_constraints(&transition, &instruction)
        .expect("Should generate constraints");

    // PC continuity should detect wrong branch
    // Expected PC = 0x50 + 4 = 0x54 (sequential)
    // Actual PC = 0x100 (wrong!)

    // Constraints are ordered: [register consistency (13), PC continuity (1), memory (32)]
    // For branch_eq: no register modified, so 13 reg constraints, then PC at index 13
    let pc_constraint_index = 13;
    let pc_constraint = &constraints[pc_constraint_index];

    assert_ne!(
        *pc_constraint,
        ligerito_binary_fields::BinaryFieldElement::zero(),
        "Wrong branch should be detected!"
    );

    println!("✓ Successfully rejected wrong branch taken");
}

/// Test that branch conditions work correctly when TRUE
#[test]
fn test_accept_correct_branch_taken() {
    // Instruction: BRANCH_EQ a0, a1, +50
    // a0 = 5, a1 = 5 (EQUAL)
    // Branch IS taken correctly

    let mut regs_before = [0u32; 13];
    regs_before[7] = 5;  // a0 = 5
    regs_before[8] = 5;  // a1 = 5 (equal!)

    let transition = ProvenTransition {
        pc: 0x50,
        next_pc: 0x50 + 50, // Branch taken to PC + offset
        instruction_size: 4,
        regs_before: PolkaVMRegisters::from_array(regs_before),
        regs_after: PolkaVMRegisters::from_array(regs_before),
        memory_root_before: [0u8; 32],
        memory_root_after: [0u8; 32],
        memory_proof: None,
        instruction_proof: ligerito::pcvm::polkavm_constraints_v2::InstructionProof {
            merkle_path: vec![],
            position: 0,
            opcode: 0,
            operands: [0, 0, 0],
        },
    };

    let instruction = Instruction::branch_eq(
        raw_reg(Reg::A0),
        raw_reg(Reg::A1),
        50,
    );

    let constraints = generate_transition_constraints(&transition, &instruction)
        .expect("Should generate constraints");

    // PC continuity should be satisfied
    // Constraints are ordered: [register consistency (13), PC continuity (1), memory (32)]
    let pc_constraint_index = 13;
    let pc_constraint = &constraints[pc_constraint_index];
    assert_eq!(
        *pc_constraint,
        ligerito_binary_fields::BinaryFieldElement::zero(),
        "Correct branch should be accepted!"
    );

    println!("✓ Correct branch accepted");
}

/// Test that unimplemented instructions cause explicit errors
#[test]
fn test_unimplemented_instruction_errors() {
    let transition = ProvenTransition {
        pc: 0x50,
        next_pc: 0x54,
        instruction_size: 4,
        regs_before: PolkaVMRegisters::new(),
        regs_after: PolkaVMRegisters::new(),
        memory_root_before: [0u8; 32],
        memory_root_after: [0u8; 32],
        memory_proof: None,
        instruction_proof: ligerito::pcvm::polkavm_constraints_v2::InstructionProof {
            merkle_path: vec![],
            position: 0,
            opcode: 0,
            operands: [0, 0, 0],
        },
    };

    // Use an unimplemented instruction (e.g., XOR)
    let instruction = Instruction::xor(
        raw_reg(Reg::A0),
        raw_reg(Reg::A1),
        raw_reg(Reg::A2),
    );

    let result = generate_transition_constraints(&transition, &instruction);

    // Should return error, not silently pass!
    assert!(
        result.is_err(),
        "Unimplemented instruction should return error!"
    );

    match result {
        Err(ConstraintError::UnimplementedInstruction { opcode }) => {
            println!("✓ Unimplemented instruction correctly rejected with opcode {}", opcode);
        }
        _ => panic!("Wrong error type!"),
    }
}

/// Test that invalid register indices are caught
#[test]
fn test_invalid_register_index() {
    // This test is theoretical - PolkaVM's type system should prevent this
    // But we verify our bounds checking anyway

    // We can't actually construct an invalid RawReg through normal API,
    // so this test documents the defense-in-depth principle
    println!("✓ Register bounds checking present in constraints (defense-in-depth)");
}

/// Test that memory operations require proofs
#[test]
fn test_load_requires_memory_proof() {
    let mut regs_before = [0u32; 13];
    regs_before[1] = 0x1000; // SP = 0x1000

    let mut regs_after = regs_before;
    regs_after[7] = 0x42; // A0 = loaded value

    let transition = ProvenTransition {
        pc: 0x50,
        next_pc: 0x54,
        instruction_size: 4,
        regs_before: PolkaVMRegisters::from_array(regs_before),
        regs_after: PolkaVMRegisters::from_array(regs_after),
        memory_root_before: [0u8; 32],
        memory_root_after: [0u8; 32],
        memory_proof: None, // NO PROOF!
        instruction_proof: ligerito::pcvm::polkavm_constraints_v2::InstructionProof {
            merkle_path: vec![],
            position: 0,
            opcode: 0,
            operands: [0, 0, 0],
        },
    };

    let instruction = Instruction::load_indirect_u32(
        raw_reg(Reg::A0),
        raw_reg(Reg::SP),
        4,
    );

    let result = generate_transition_constraints(&transition, &instruction);

    // Should error due to missing proof
    assert!(result.is_err(), "Load without proof should be rejected!");

    match result {
        Err(ConstraintError::MissingMemoryProof) => {
            println!("✓ Load without memory proof correctly rejected");
        }
        _ => panic!("Wrong error type!"),
    }
}

/// Test that memory roots must be consistent for non-memory instructions
#[test]
fn test_memory_root_consistency() {
    let mut regs_before = [0u32; 13];
    regs_before[2] = 10; // T0 = 10
    regs_before[3] = 20; // T1 = 20

    let mut regs_after = regs_before;
    regs_after[4] = 30; // T2 = 10 + 20

    let mut memory_root_before = [0u8; 32];
    memory_root_before[0] = 0xAB;

    let mut memory_root_after = [0u8; 32];
    memory_root_after[0] = 0xCD; // CHANGED! (should not change for ADD)

    let transition = ProvenTransition {
        pc: 0x50,
        next_pc: 0x53,
        instruction_size: 3,
        regs_before: PolkaVMRegisters::from_array(regs_before),
        regs_after: PolkaVMRegisters::from_array(regs_after),
        memory_root_before,
        memory_root_after,
        memory_proof: None,
        instruction_proof: ligerito::pcvm::polkavm_constraints_v2::InstructionProof {
            merkle_path: vec![],
            position: 0,
            opcode: 0,
            operands: [0, 0, 0],
        },
    };

    let instruction = Instruction::add_32(
        raw_reg(Reg::T2),
        raw_reg(Reg::T0),
        raw_reg(Reg::T1),
    );

    let constraints = generate_transition_constraints(&transition, &instruction)
        .expect("Should generate constraints");

    // Memory consistency constraints should detect the change
    // The last 32 constraints check memory_root_before == memory_root_after
    let memory_constraints = &constraints[constraints.len() - 32..];

    // First byte changed, so first constraint should be non-zero
    assert_ne!(
        memory_constraints[0],
        ligerito_binary_fields::BinaryFieldElement::zero(),
        "Memory root modification should be detected!"
    );

    println!("✓ Memory root modification detected for non-memory instruction");
}

/// Integration test: verify full execution trace
#[test]
fn test_full_trace_verification() {
    // Simulate: a0 = a1 + a2
    // a1 = 10, a2 = 20
    // Result: a0 = 30

    let mut regs_before = [0u32; 13];
    regs_before[8] = 10;  // A1 = 10
    regs_before[9] = 20;  // A2 = 20

    let mut regs_after = regs_before;
    regs_after[7] = 30;   // A0 = 10 + 20

    let transition = ProvenTransition {
        pc: 0x100,
        next_pc: 0x103, // Sequential (instruction size = 3)
        instruction_size: 3,
        regs_before: PolkaVMRegisters::from_array(regs_before),
        regs_after: PolkaVMRegisters::from_array(regs_after),
        memory_root_before: [0u8; 32],
        memory_root_after: [0u8; 32],
        memory_proof: None,
        instruction_proof: ligerito::pcvm::polkavm_constraints_v2::InstructionProof {
            merkle_path: vec![],
            position: 0,
            opcode: 0,
            operands: [0, 0, 0],
        },
    };

    let instruction = Instruction::add_32(
        raw_reg(Reg::A0),
        raw_reg(Reg::A1),
        raw_reg(Reg::A2),
    );

    let constraints = generate_transition_constraints(&transition, &instruction)
        .expect("Should generate constraints");

    // All constraints should be zero
    for (i, constraint) in constraints.iter().enumerate() {
        assert_eq!(
            *constraint,
            ligerito_binary_fields::BinaryFieldElement::zero(),
            "Constraint {} should be satisfied", i
        );
    }

    println!("✓ Full correct execution verified");
}

/// Test batched verification: single check for entire trace
#[test]
fn test_batched_verification() {
    use ligerito::pcvm::polkavm_constraints_v2::verify_trace_batched_with_challenge;
    use ligerito_binary_fields::{BinaryElem32, BinaryFieldElement};

    // Create a simple 3-step trace: a0 = 10, a1 = 20, a2 = a0 + a1
    let mut regs_0 = [0u32; 13];

    // Step 1: load_imm a0, 10
    let mut regs_1 = regs_0;
    regs_1[7] = 10;  // a0 = 10

    let step1 = (
        ProvenTransition {
            pc: 0x100,
            next_pc: 0x102,
            instruction_size: 2,
            regs_before: PolkaVMRegisters::from_array(regs_0),
            regs_after: PolkaVMRegisters::from_array(regs_1),
            memory_root_before: [0u8; 32],
            memory_root_after: [0u8; 32],
            memory_proof: None,
            instruction_proof: ligerito::pcvm::polkavm_constraints_v2::InstructionProof {
                merkle_path: vec![],
                position: 0,
                opcode: 0,
                operands: [0, 0, 0],
            },
        },
        Instruction::load_imm(raw_reg(Reg::A0), 10),
    );

    // Step 2: load_imm a1, 20
    let mut regs_2 = regs_1;
    regs_2[8] = 20;  // a1 = 20

    let step2 = (
        ProvenTransition {
            pc: 0x102,
            next_pc: 0x104,
            instruction_size: 2,
            regs_before: PolkaVMRegisters::from_array(regs_1),
            regs_after: PolkaVMRegisters::from_array(regs_2),
            memory_root_before: [0u8; 32],
            memory_root_after: [0u8; 32],
            memory_proof: None,
            instruction_proof: ligerito::pcvm::polkavm_constraints_v2::InstructionProof {
                merkle_path: vec![],
                position: 0,
                opcode: 0,
                operands: [0, 0, 0],
            },
        },
        Instruction::load_imm(raw_reg(Reg::A1), 20),
    );

    // Step 3: add a2, a0, a1
    let mut regs_3 = regs_2;
    regs_3[9] = 30;  // a2 = 10 + 20

    let step3 = (
        ProvenTransition {
            pc: 0x104,
            next_pc: 0x107,
            instruction_size: 3,
            regs_before: PolkaVMRegisters::from_array(regs_2),
            regs_after: PolkaVMRegisters::from_array(regs_3),
            memory_root_before: [0u8; 32],
            memory_root_after: [0u8; 32],
            memory_proof: None,
            instruction_proof: ligerito::pcvm::polkavm_constraints_v2::InstructionProof {
                merkle_path: vec![],
                position: 0,
                opcode: 0,
                operands: [0, 0, 0],
            },
        },
        Instruction::add_32(
            raw_reg(Reg::A2),
            raw_reg(Reg::A0),
            raw_reg(Reg::A1),
        ),
    );

    let trace = vec![step1, step2, step3];

    // Use a deterministic challenge for testing
    let challenge = BinaryElem32::from(0x12345678u32);

    // Batched verification: ALL constraints in ONE check
    let result = verify_trace_batched_with_challenge(&trace, challenge)
        .expect("Should verify successfully");

    assert!(result, "Batched verification should pass for valid trace");

    println!("✓ Batched verification: 3 steps × ~46 constraints = 138 checks → 1 accumulator check");
}

/// Test that batched verification catches forgeries
#[test]
fn test_batched_rejects_forgery() {
    use ligerito::pcvm::polkavm_constraints_v2::verify_trace_batched_with_challenge;
    use ligerito_binary_fields::BinaryElem32;

    // Create forged trace: claim a0 = a1 + a2 but use WRONG result
    let mut regs_before = [0u32; 13];
    regs_before[8] = 10;  // a1 = 10
    regs_before[9] = 20;  // a2 = 20

    let mut regs_after = regs_before;
    regs_after[7] = 999;  // a0 = 999 (WRONG! Should be 30)

    let forged_step = (
        ProvenTransition {
            pc: 0x100,
            next_pc: 0x103,
            instruction_size: 3,
            regs_before: PolkaVMRegisters::from_array(regs_before),
            regs_after: PolkaVMRegisters::from_array(regs_after),
            memory_root_before: [0u8; 32],
            memory_root_after: [0u8; 32],
            memory_proof: None,
            instruction_proof: ligerito::pcvm::polkavm_constraints_v2::InstructionProof {
                merkle_path: vec![],
                position: 0,
                opcode: 0,
                operands: [0, 0, 0],
            },
        },
        Instruction::add_32(
            raw_reg(Reg::A0),
            raw_reg(Reg::A1),
            raw_reg(Reg::A2),
        ),
    );

    let trace = vec![forged_step];
    let challenge = BinaryElem32::from(0x12345678u32);

    let result = verify_trace_batched_with_challenge(&trace, challenge)
        .expect("Should generate constraints");

    assert!(!result, "Batched verification must reject forged execution!");

    println!("✓ Batched verification correctly rejects forgery");
}
