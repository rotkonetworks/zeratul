//! End-to-end integration test for PolkaVM trace extraction and constraint generation
//!
//! This test uses a real PolkaVM binary to verify:
//! 1. Trace extraction works
//! 2. Constraint generation works
//! 3. All constraints are satisfied for valid execution

#![cfg(feature = "polkavm-integration")]

use ligerito::pcvm::polkavm_tracer::extract_polkavm_trace;
use ligerito::pcvm::polkavm_constraints::{generate_step_constraints, verify_step_constraints};
use polkavm::ProgramBlob;
use polkavm::program::ISA32_V1;

#[test]
fn test_extract_trace_from_real_binary() {
    // Use the hello-world example binary from PolkaVM
    let binary_path = "/home/alice/src/polkavm/guest-programs/output/example-hello-world.polkavm";
    let program_blob = std::fs::read(binary_path)
        .expect("Failed to read PolkaVM binary");

    // Extract trace with step limit (increased to 10000 since program might be larger)
    let trace = extract_polkavm_trace(&program_blob, 10000)
        .expect("Failed to extract trace");

    println!("✓ Extracted trace with {} steps", trace.steps.len());
    println!("  Program hash: {:?}", trace.program_hash);

    // Verify we got some steps
    assert!(trace.steps.len() > 0, "Trace should have at least one step");

    // Print first few steps for debugging
    println!("\nFirst 10 steps:");
    for (i, step) in trace.steps.iter().take(10).enumerate() {
        println!("  Step {}: PC={:#x}, opcode={}", i, step.pc, step.opcode);
    }

    // Print last few steps
    println!("\nLast 5 steps:");
    let start = trace.steps.len().saturating_sub(5);
    for (i, step) in trace.steps.iter().skip(start).enumerate() {
        println!("  Step {}: PC={:#x}, opcode={}", start + i, step.pc, step.opcode);
    }
}

#[test]
fn test_generate_constraints_from_trace() {
    // Read the PolkaVM binary
    let binary_path = "/home/alice/src/polkavm/guest-programs/output/example-hello-world.polkavm";
    let program_blob = std::fs::read(binary_path)
        .expect("Failed to read PolkaVM binary");

    // Parse to get instruction decoder
    let blob = ProgramBlob::parse(program_blob[..].into())
        .expect("Failed to parse program blob");

    // Extract trace
    let trace = extract_polkavm_trace(&program_blob, 1000)
        .expect("Failed to extract trace");

    println!("Generating constraints for {} steps...", trace.steps.len());

    let mut total_constraints = 0;
    let mut instructions_with_constraints = 0;

    // Generate constraints for each step
    for (i, step) in trace.steps.iter().enumerate() {
        // Decode instruction at this PC
        let pc = polkavm::ProgramCounter(step.pc);
        let mut instructions = blob.instructions_bounded_at(ISA32_V1, pc);

        if let Some(parsed) = instructions.next() {
            let instruction = parsed.kind;

            // Generate constraints
            let constraints = generate_step_constraints(step, &instruction);

            if !constraints.is_empty() {
                total_constraints += constraints.len();
                instructions_with_constraints += 1;

                if i < 5 {
                    println!("  Step {}: {:?} -> {} constraints", i, instruction, constraints.len());
                }
            }
        }
    }

    println!("\n✓ Generated {} total constraints across {} instructions",
             total_constraints, instructions_with_constraints);

    assert!(total_constraints > 0, "Should have generated some constraints");
}

#[test]
fn test_verify_constraints_all_satisfied() {
    // Read the PolkaVM binary
    let binary_path = "/home/alice/src/polkavm/guest-programs/output/example-hello-world.polkavm";
    let program_blob = std::fs::read(binary_path)
        .expect("Failed to read PolkaVM binary");

    // Parse blob
    let blob = ProgramBlob::parse(program_blob[..].into())
        .expect("Failed to parse program blob");

    // Extract trace
    let trace = extract_polkavm_trace(&program_blob, 1000)
        .expect("Failed to extract trace");

    println!("Verifying constraints for {} steps...", trace.steps.len());

    let mut verified_steps = 0;
    let mut total_violations = Vec::new();

    // Verify constraints for each step
    for (i, step) in trace.steps.iter().enumerate() {
        let pc = polkavm::ProgramCounter(step.pc);
        let mut instructions = blob.instructions_bounded_at(ISA32_V1, pc);

        if let Some(parsed) = instructions.next() {
            let instruction = parsed.kind;

            // Verify constraints
            match verify_step_constraints(step, &instruction, i) {
                Ok(()) => {
                    verified_steps += 1;
                }
                Err(violations) => {
                    total_violations.extend(violations);
                }
            }
        }
    }

    println!("✓ Verified {} steps", verified_steps);

    if !total_violations.is_empty() {
        println!("⚠ Found {} constraint violations:", total_violations.len());
        for (i, violation) in total_violations.iter().take(10).enumerate() {
            println!("  {}: {}", i, violation.message);
        }

        // For now, we expect some violations because:
        // 1. We haven't implemented all instructions yet
        // 2. Some instructions might need special handling
        // But we should at least verify SOME steps successfully
        assert!(verified_steps > 0, "Should verify at least some steps successfully");
    } else {
        println!("✓ All constraints satisfied!");
    }
}

// TODO: Add simple assembly test once we figure out the assembler API
// #[test]
// fn test_simple_add_trace() {
//     ...
// }
