//! Test the PolkaVM verifier with a generated proof
//!
//! This example:
//! 1. Generates a small Ligerito proof (2^12 for speed)
//! 2. Serializes it to a file
//! 3. Feeds it to the PolkaVM verifier
//! 4. Checks the result

use std::fs::File;
use std::io::Write;
use std::process::{Command, Stdio};
use std::marker::PhantomData;

use binary_fields::{BinaryElem32, BinaryElem128};
use ligerito::{prover, verifier, hardcoded_config_12, hardcoded_config_12_verifier};
use rand::Rng;

fn main() {
    println!("Testing PolkaVM Verifier");
    println!("========================\n");

    // Step 1: Generate a small proof (2^12 for speed)
    println!("Step 1: Generating proof (2^12 polynomial)...");

    let config = hardcoded_config_12(
        PhantomData::<BinaryElem32>,
        PhantomData::<BinaryElem128>,
    );

    let mut rng = rand::thread_rng();
    let poly: Vec<BinaryElem32> = (0..1 << 12)
        .map(|_| BinaryElem32::from(rng.gen::<u32>()))
        .collect();

    let proof = prover(&config, &poly).expect("Proving failed");
    println!("✓ Proof generated");

    // Step 2: Verify locally first (sanity check)
    println!("\nStep 2: Local verification (sanity check)...");
    let verifier_config = hardcoded_config_12_verifier();
    let local_result = verifier(&verifier_config, &proof)
        .expect("Local verification failed");

    if !local_result {
        eprintln!("✗ Local verification FAILED - proof is invalid!");
        std::process::exit(1);
    }
    println!("✓ Local verification passed");

    // Step 3: Serialize proof
    println!("\nStep 3: Serializing proof...");
    let proof_bytes = bincode::serialize(&proof).expect("Serialization failed");
    println!("✓ Proof serialized: {} bytes", proof_bytes.len());

    // Step 4: Prepare input for PolkaVM verifier
    // Format: [config_size: u32][proof_bytes]
    println!("\nStep 4: Preparing PolkaVM input...");
    let config_size: u32 = 12;
    let mut input = Vec::new();
    input.extend_from_slice(&config_size.to_le_bytes());
    input.extend_from_slice(&proof_bytes);
    println!("✓ Input prepared: {} bytes total", input.len());

    // Save to file for manual testing
    let input_file = "/tmp/polkavm_verifier_input.bin";
    let mut file = File::create(input_file).expect("Failed to create input file");
    file.write_all(&input).expect("Failed to write input file");
    println!("✓ Input saved to: {}", input_file);

    // Step 5: Check if PolkaVM verifier binary exists
    println!("\nStep 5: Testing with PolkaVM verifier...");
    let verifier_path = "examples/polkavm_verifier/target/polkavm_verifier";

    // First check if the binary exists
    if !std::path::Path::new(verifier_path).exists() {
        println!("⚠ PolkaVM verifier binary not found at: {}", verifier_path);
        println!("\nTo build it, run:");
        println!("  cd examples/polkavm_verifier");
        println!("  . ../../polkaports/activate.sh corevm");
        println!("  make");
        println!("\nFor now, skipping PolkaVM test.");
        println!("\n✓ Test completed successfully (local verification passed)");
        println!("\nYou can manually test with:");
        println!("  cat {} | {}", input_file, verifier_path);
        return;
    }

    // Try to run the verifier
    println!("Running: cat {} | {}", input_file, verifier_path);

    let mut child = Command::new("sh")
        .arg("-c")
        .arg(format!("cat {} | {}", input_file, verifier_path))
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("Failed to spawn verifier");

    let output = child.wait_with_output().expect("Failed to wait for verifier");

    println!("\nPolkaVM Verifier Output:");
    println!("------------------------");
    if !output.stdout.is_empty() {
        println!("stdout: {}", String::from_utf8_lossy(&output.stdout));
    }
    if !output.stderr.is_empty() {
        println!("stderr: {}", String::from_utf8_lossy(&output.stderr));
    }
    println!("exit code: {:?}", output.status.code());

    // Check result
    match output.status.code() {
        Some(0) => {
            println!("\n✓ PolkaVM verifier returned VALID!");
            println!("✓ All tests passed!");
        }
        Some(1) => {
            eprintln!("\n✗ PolkaVM verifier returned INVALID!");
            std::process::exit(1);
        }
        Some(code) => {
            eprintln!("\n✗ PolkaVM verifier returned error code: {}", code);
            std::process::exit(1);
        }
        None => {
            eprintln!("\n✗ PolkaVM verifier was terminated by signal");
            std::process::exit(1);
        }
    }

    println!("\n=========================");
    println!("Summary:");
    println!("  Polynomial size: 2^12 (4096 elements)");
    println!("  Proof size: {} bytes", proof_bytes.len());
    println!("  Local verification: ✓ PASSED");
    println!("  PolkaVM verification: ✓ PASSED");
    println!("=========================");
}
