//! State Continuity Tests
//!
//! These tests verify that execution chains correctly - critical for
//! continuous PVM execution (JAM/graypaper model).
//!
//! Without state continuity constraints, a prover could:
//! 1. Execute step i correctly
//! 2. Fork to a different state for step i+1
//! 3. Execute step i+1 correctly from that forged state
//! 4. Have both steps verify individually ✓
//! 5. But break the execution chain ✗

#![cfg(feature = "polkavm-integration")]

use ligerito::pcvm::polkavm_constraints_v2::{ProvenTransition, InstructionProof};
use ligerito::pcvm::polkavm_adapter::PolkaVMRegisters;
use ligerito::pcvm::polkavm_arithmetization::arithmetize_polkavm_trace;
use ligerito_binary_fields::{BinaryElem32, BinaryFieldElement};

use polkavm::program::Instruction;
use polkavm_common::program::{RawReg, Reg};

fn raw_reg(r: Reg) -> RawReg {
    RawReg::from(r)
}

/// Test: Forged register continuity is rejected
#[test]
fn test_reject_forged_register_continuity() {
    // Step 1: load_imm a0, 42
    let mut regs_0 = [0u32; 13];
    let mut regs_1 = regs_0;
    regs_1[7] = 42;  // a0 = 42

    let step1 = (
        ProvenTransition {
            pc: 0x1000,
            next_pc: 0x1002,
            instruction_size: 2,
            regs_before: PolkaVMRegisters::from_array(regs_0),
            regs_after: PolkaVMRegisters::from_array(regs_1),
            memory_root_before: [0u8; 32],
            memory_root_after: [0u8; 32],
            memory_proof: None,
            instruction_proof: InstructionProof {
                merkle_path: vec![],
                position: 0,
                opcode: 0,
                operands: [0, 0, 0],
            },
        },
        Instruction::load_imm(raw_reg(Reg::A0), 42),
    );

    // Step 2: FORGED initial state
    // Instead of starting with a0=42 (from step 1), we claim a0=999
    let mut forged_regs_before = [0u32; 13];
    forged_regs_before[7] = 999;  // WRONG! Should be 42

    let mut regs_2 = forged_regs_before;
    regs_2[8] = 100;  // a1 = 100

    let step2 = (
        ProvenTransition {
            pc: 0x1002,
            next_pc: 0x1004,
            instruction_size: 2,
            regs_before: PolkaVMRegisters::from_array(forged_regs_before),  // FORGED!
            regs_after: PolkaVMRegisters::from_array(regs_2),
            memory_root_before: [0u8; 32],
            memory_root_after: [0u8; 32],
            memory_proof: None,
            instruction_proof: InstructionProof {
                merkle_path: vec![],
                position: 0,
                opcode: 0,
                operands: [0, 0, 0],
            },
        },
        Instruction::load_imm(raw_reg(Reg::A1), 100),
    );

    let trace = vec![step1, step2];

    // Arithmetize with batching challenge
    let batching_challenge = BinaryElem32::from(0x12345678u32);
    let arith = arithmetize_polkavm_trace(&trace, [0u8; 32], batching_challenge)
        .expect("Should arithmetize");

    // Constraint accumulator should be NON-ZERO due to forged register continuity
    assert_ne!(
        arith.constraint_accumulator,
        BinaryElem32::zero(),
        "Forged register continuity should fail! Accumulator: {:?}",
        arith.constraint_accumulator
    );

    println!("✓ Forged register continuity correctly rejected");
    println!("  - Step 1 ends with: a0 = 42");
    println!("  - Step 2 claims to start with: a0 = 999");
    println!("  - Constraint accumulator: {:?} (non-zero)", arith.constraint_accumulator);
}

/// Test: Forged memory root continuity is rejected
#[test]
fn test_reject_forged_memory_continuity() {
    let mut regs = [0u32; 13];

    // Step 1: Some operation that produces memory_root_after = [1, 2, 3, ...]
    let mut memory_root_1 = [0u8; 32];
    memory_root_1[0] = 1;
    memory_root_1[1] = 2;

    let step1 = (
        ProvenTransition {
            pc: 0x1000,
            next_pc: 0x1002,
            instruction_size: 2,
            regs_before: PolkaVMRegisters::from_array(regs),
            regs_after: PolkaVMRegisters::from_array(regs),
            memory_root_before: [0u8; 32],
            memory_root_after: memory_root_1,  // Final state: [1, 2, 0, 0, ...]
            memory_proof: None,
            instruction_proof: InstructionProof {
                merkle_path: vec![],
                position: 0,
                opcode: 0,
                operands: [0, 0, 0],
            },
        },
        Instruction::load_imm(raw_reg(Reg::A0), 0),
    );

    // Step 2: FORGED memory root
    // Claims to start with [99, 88, ...] instead of [1, 2, ...]
    let mut forged_memory_root = [0u8; 32];
    forged_memory_root[0] = 99;  // WRONG! Should be 1
    forged_memory_root[1] = 88;  // WRONG! Should be 2

    let step2 = (
        ProvenTransition {
            pc: 0x1002,
            next_pc: 0x1004,
            instruction_size: 2,
            regs_before: PolkaVMRegisters::from_array(regs),
            regs_after: PolkaVMRegisters::from_array(regs),
            memory_root_before: forged_memory_root,  // FORGED!
            memory_root_after: forged_memory_root,
            memory_proof: None,
            instruction_proof: InstructionProof {
                merkle_path: vec![],
                position: 0,
                opcode: 0,
                operands: [0, 0, 0],
            },
        },
        Instruction::load_imm(raw_reg(Reg::A0), 0),
    );

    let trace = vec![step1, step2];

    let batching_challenge = BinaryElem32::from(0xABCDEF01u32);
    let arith = arithmetize_polkavm_trace(&trace, [0u8; 32], batching_challenge)
        .expect("Should arithmetize");

    // Constraint accumulator should be NON-ZERO
    assert_ne!(
        arith.constraint_accumulator,
        BinaryElem32::zero(),
        "Forged memory continuity should fail!"
    );

    println!("✓ Forged memory root continuity correctly rejected");
    println!("  - Step 1 ends with memory_root: [1, 2, 0, ...]");
    println!("  - Step 2 claims to start with: [99, 88, 0, ...]");
    println!("  - Constraint accumulator: {:?} (non-zero)", arith.constraint_accumulator);
}

/// Test: Forged PC continuity is rejected
#[test]
fn test_reject_forged_pc_continuity() {
    let mut regs = [0u32; 13];

    // Step 1: Instruction at 0x1000, next_pc = 0x1002
    let step1 = (
        ProvenTransition {
            pc: 0x1000,
            next_pc: 0x1002,  // Step 1 says "next is 0x1002"
            instruction_size: 2,
            regs_before: PolkaVMRegisters::from_array(regs),
            regs_after: PolkaVMRegisters::from_array(regs),
            memory_root_before: [0u8; 32],
            memory_root_after: [0u8; 32],
            memory_proof: None,
            instruction_proof: InstructionProof {
                merkle_path: vec![],
                position: 0,
                opcode: 0,
                operands: [0, 0, 0],
            },
        },
        Instruction::load_imm(raw_reg(Reg::A0), 0),
    );

    // Step 2: FORGED PC
    // Claims to start at 0x9999 instead of 0x1002
    let step2 = (
        ProvenTransition {
            pc: 0x9999,  // WRONG! Should be 0x1002
            next_pc: 0x999B,
            instruction_size: 2,
            regs_before: PolkaVMRegisters::from_array(regs),
            regs_after: PolkaVMRegisters::from_array(regs),
            memory_root_before: [0u8; 32],
            memory_root_after: [0u8; 32],
            memory_proof: None,
            instruction_proof: InstructionProof {
                merkle_path: vec![],
                position: 0,
                opcode: 0,
                operands: [0, 0, 0],
            },
        },
        Instruction::load_imm(raw_reg(Reg::A0), 0),
    );

    let trace = vec![step1, step2];

    let batching_challenge = BinaryElem32::from(0x11111111u32);
    let arith = arithmetize_polkavm_trace(&trace, [0u8; 32], batching_challenge)
        .expect("Should arithmetize");

    // Constraint accumulator should be NON-ZERO
    assert_ne!(
        arith.constraint_accumulator,
        BinaryElem32::zero(),
        "Forged PC continuity should fail!"
    );

    println!("✓ Forged PC continuity correctly rejected");
    println!("  - Step 1 next_pc: 0x1002");
    println!("  - Step 2 pc: 0x9999 (control flow forgery!)");
    println!("  - Constraint accumulator: {:?} (non-zero)", arith.constraint_accumulator);
}

/// Test: Valid continuity passes
#[test]
fn test_valid_state_continuity_passes() {
    // Create a valid 3-step chain
    let mut regs_0 = [0u32; 13];

    // Step 1: load_imm a0, 10
    let mut regs_1 = regs_0;
    regs_1[7] = 10;

    let step1 = (
        ProvenTransition {
            pc: 0x1000,
            next_pc: 0x1002,
            instruction_size: 2,
            regs_before: PolkaVMRegisters::from_array(regs_0),
            regs_after: PolkaVMRegisters::from_array(regs_1),  // Final: a0=10
            memory_root_before: [0u8; 32],
            memory_root_after: [0u8; 32],
            memory_proof: None,
            instruction_proof: InstructionProof {
                merkle_path: vec![],
                position: 0,
                opcode: 0,
                operands: [0, 0, 0],
            },
        },
        Instruction::load_imm(raw_reg(Reg::A0), 10),
    );

    // Step 2: load_imm a1, 20
    // CORRECT: regs_before = regs_1 (from step 1's regs_after)
    let mut regs_2 = regs_1;
    regs_2[8] = 20;

    let step2 = (
        ProvenTransition {
            pc: 0x1002,  // CORRECT: matches step1.next_pc
            next_pc: 0x1004,
            instruction_size: 2,
            regs_before: PolkaVMRegisters::from_array(regs_1),  // CORRECT: chains from step 1
            regs_after: PolkaVMRegisters::from_array(regs_2),
            memory_root_before: [0u8; 32],  // CORRECT: matches step1.memory_root_after
            memory_root_after: [0u8; 32],
            memory_proof: None,
            instruction_proof: InstructionProof {
                merkle_path: vec![],
                position: 0,
                opcode: 0,
                operands: [0, 0, 0],
            },
        },
        Instruction::load_imm(raw_reg(Reg::A1), 20),
    );

    let trace = vec![step1, step2];

    let batching_challenge = BinaryElem32::from(0x42424242u32);
    let arith = arithmetize_polkavm_trace(&trace, [0u8; 32], batching_challenge)
        .expect("Should arithmetize");

    // Constraint accumulator should be ZERO (all constraints satisfied)
    assert_eq!(
        arith.constraint_accumulator,
        BinaryElem32::zero(),
        "Valid continuity should pass! Accumulator: {:?}",
        arith.constraint_accumulator
    );

    println!("✓ Valid state continuity correctly accepted");
    println!("  - All states chain correctly");
    println!("  - Constraint accumulator: ZERO (all satisfied)");
}
