use ligerito::{prove_sha256, hardcoded_config_12, hardcoded_config_12_verifier, verify_sha256};
use binary_fields::{BinaryElem32, BinaryElem128};
use std::fs::File;
use std::io::Write;
use std::marker::PhantomData;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    println!("Creating detailed interoperability test data...");

    // Use the same config as our tests
    let config = hardcoded_config_12(
        PhantomData::<BinaryElem32>,
        PhantomData::<BinaryElem128>,
    );

    // Simple test polynomial - same as in our tests
    let poly: Vec<BinaryElem32> = (0..4096)
        .map(|i| BinaryElem32::from(i as u32))
        .collect();

    println!("Polynomial size: {}", poly.len());
    println!("First few elements: {:?}", &poly[..4.min(poly.len())]);

    // Generate proof
    println!("Generating proof...");
    let proof = prove_sha256(&config, &poly)?;

    println!("Proof generated successfully!");

    // Export detailed data including actual proof elements
    let export_data = format!(
        "# Detailed Rust-generated proof data for Julia verification\n\
         # Polynomial: size={}, first 10 elements = {:?}\n\
         # Config: initial_dims=({}, {}), k={}, recursive_steps={}\n\
         # Transcript seed: 1234 (SHA256 mode)\n\
         #\n\
         # === PROOF STRUCTURE ===\n\
         # Initial commitment root: {:?}\n\
         # Recursive commitments: {}\n\
         # Final yr length: {}\n\
         # Sumcheck rounds: {}\n\
         #\n\
         # === FINAL YR VALUES (first 10) ===\n\
         # {:?}\n\
         #\n\
         # === SUMCHECK TRANSCRIPT (coefficients) ===\n\
         # Round 1: {:?}\n\
         # Round 2: {:?}\n\
         #\n\
         # This data can be used to manually construct a proof in Julia\n\
         # and compare with our Rust implementation for debugging\n",
        poly.len(),
        &poly[..10.min(poly.len())],
        config.initial_dims.0,
        config.initial_dims.1,
        config.initial_k,
        config.recursive_steps,
        proof.initial_ligero_cm.root.root,
        proof.recursive_commitments.len(),
        proof.final_ligero_proof.yr.len(),
        proof.sumcheck_transcript.transcript.len(),
        &proof.final_ligero_proof.yr[..10.min(proof.final_ligero_proof.yr.len())],
        if proof.sumcheck_transcript.transcript.len() > 0 {
            format!("{:?}", proof.sumcheck_transcript.transcript[0])
        } else {
            "None".to_string()
        },
        if proof.sumcheck_transcript.transcript.len() > 1 {
            format!("{:?}", proof.sumcheck_transcript.transcript[1])
        } else {
            "None".to_string()
        }
    );

    // Write to file
    let mut file = File::create("detailed_rust_proof.txt")?;
    file.write_all(export_data.as_bytes())?;

    println!("Exported detailed proof data to detailed_rust_proof.txt");

    // Also try to verify our own proof
    println!("\n=== SELF-VERIFICATION TEST ===");
    let verifier_config = hardcoded_config_12_verifier();
    match verify_sha256(&verifier_config, &proof) {
        Ok(true) => println!("✓ Rust self-verification PASSED"),
        Ok(false) => println!("✗ Rust self-verification FAILED (returned false)"),
        Err(e) => println!("✗ Rust self-verification ERROR: {}", e),
    }

    Ok(())
}