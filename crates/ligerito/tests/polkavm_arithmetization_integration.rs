//! Integration test: Arithmetize and verify real PolkaVM execution
//!
//! This test demonstrates the complete flow:
//! 1. Execute PolkaVM program → extract trace
//! 2. Arithmetize trace → multilinear polynomial
//! 3. Verify batched constraints → soundness check
//!
//! This is the foundation for Ligerito polynomial commitment integration.

#![cfg(feature = "polkavm-integration")]

use ligerito::pcvm::polkavm_constraints_v2::ProvenTransition;
use ligerito::pcvm::polkavm_adapter::PolkaVMRegisters;
use ligerito::pcvm::polkavm_arithmetization::{
    arithmetize_polkavm_trace, verify_arithmetized_trace, STEP_WIDTH,
};
use ligerito_binary_fields::{BinaryElem32, BinaryFieldElement};

use polkavm::program::Instruction;
use polkavm_common::program::{RawReg, Reg};

fn raw_reg(r: Reg) -> RawReg {
    RawReg::from(r)
}

/// Test: Arithmetize a simple 3-step trace
#[test]
fn test_arithmetize_simple_trace() {
    // Create a simple trace: load, load, add
    let mut regs_0 = [0u32; 13];

    // Step 1: load_imm a0, 10
    let mut regs_1 = regs_0;
    regs_1[7] = 10;

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
    regs_2[8] = 20;

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
    regs_3[9] = 30;

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

    // Arithmetize with Fiat-Shamir challenge
    let challenge = BinaryElem32::from(0x12345678u32);
    let program_commitment = [0xABu8; 32];

    let arith = arithmetize_polkavm_trace(&trace, program_commitment, challenge)
        .expect("Failed to arithmetize");

    // Verify dimensions
    assert_eq!(arith.num_steps, 3);
    assert_eq!(arith.step_width, STEP_WIDTH);
    assert_eq!(arith.trace_polynomial.len(), 3 * STEP_WIDTH);

    // Verify batched constraints (should be zero for valid trace)
    assert_eq!(
        arith.constraint_accumulator,
        BinaryElem32::zero(),
        "Valid trace should have zero constraint accumulator"
    );

    // Verify the whole arithmetization
    assert!(
        verify_arithmetized_trace(&arith),
        "Arithmetized trace should verify"
    );

    println!("✓ Arithmetized 3-step trace:");
    println!("  - Trace polynomial: {} elements", arith.trace_polynomial.len());
    println!("  - Matrix dimensions: {} steps × {} columns", arith.num_steps, arith.step_width);
    println!("  - Constraint accumulator: {:?}", arith.constraint_accumulator);
    println!("  - Batching challenge: {:?}", arith.batching_challenge);
}

/// Test: Forged execution should fail verification
#[test]
fn test_arithmetize_rejects_forgery() {
    // Create forged trace: claim wrong ADD result
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
    let program_commitment = [0xABu8; 32];

    let arith = arithmetize_polkavm_trace(&trace, program_commitment, challenge)
        .expect("Should arithmetize (but with non-zero constraints)");

    // Constraint accumulator should be NON-ZERO
    assert_ne!(
        arith.constraint_accumulator,
        BinaryElem32::zero(),
        "Forged trace should have non-zero constraint accumulator"
    );

    // Verification should FAIL
    assert!(
        !verify_arithmetized_trace(&arith),
        "Forged trace should fail verification"
    );

    println!("✓ Arithmetization correctly rejects forged execution:");
    println!("  - Constraint accumulator: {:?} (non-zero!)", arith.constraint_accumulator);
}

/// Test: Polynomial dimensions for different trace lengths
#[test]
fn test_polynomial_scaling() {
    // Test traces of different lengths
    for num_steps in [1, 2, 4, 8, 16] {
        let mut trace = Vec::new();

        let mut regs = [0u32; 13];
        for i in 0..num_steps {
            let step = (
                ProvenTransition {
                    pc: 0x100 + (i as u32 * 2),
                    next_pc: 0x100 + ((i + 1) as u32 * 2),
                    instruction_size: 2,
                    regs_before: PolkaVMRegisters::from_array(regs),
                    regs_after: {
                        regs[7] = i as u32;  // a0 = i
                        PolkaVMRegisters::from_array(regs)
                    },
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
                Instruction::load_imm(raw_reg(Reg::A0), i as u32),
            );
            trace.push(step);
        }

        let challenge = BinaryElem32::from(0x42);
        let arith = arithmetize_polkavm_trace(&trace, [0u8; 32], challenge)
            .expect("Failed to arithmetize");

        let expected_size = num_steps * STEP_WIDTH;
        assert_eq!(
            arith.trace_polynomial.len(),
            expected_size,
            "Polynomial size mismatch for {} steps", num_steps
        );

        println!("✓ {} steps → {} polynomial elements", num_steps, expected_size);
    }
}
