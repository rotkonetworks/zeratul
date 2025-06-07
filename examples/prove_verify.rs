//! Example of proving and verifying with Ligerito

use binary_fields::{BinaryElem32, BinaryElem128};
use ligerito::{prover, verifier, hardcoded_config_24, hardcoded_config_24_verifier};
use rand::Rng;
use std::time::Instant;

fn main() {
    println!("Ligerito Polynomial Commitment Example");
    println!("=====================================");

    // Create configuration for 2^24 polynomial
    let config = hardcoded_config_24(
        std::marker::PhantomData::<BinaryElem32>,
        std::marker::PhantomData::<BinaryElem128>,
    );

    // Generate random polynomial
    println!("Generating random polynomial of size 2^24...");
    let mut rng = rand::thread_rng();
    let poly: Vec<BinaryElem32> = (0..1 << 24)
        .map(|_| BinaryElem32::from(rng.gen::<u32>()))
        .collect();

    // Print thread count
    let thread_count = rayon::current_num_threads();
    println!("Running with {} threads", thread_count);

    // Prove
    println!("\nGenerating proof...");
    let start = Instant::now();
    let proof = prover(&config, &poly).expect("Proving failed");
    let prove_time = start.elapsed();

    let proof_size = std::mem::size_of_val(&proof);
    println!("Proof generated in: {:.2?}", prove_time);
    println!("Proof size: {} bytes", proof_size);
    println!("Proof size (human): {}", format_bytes(proof_size));

    // Verify
    println!("\nVerifying proof...");
    let verifier_config = hardcoded_config_24_verifier();
    let start = Instant::now();
    let verification_result = verifier(&verifier_config, &proof)
        .expect("Verification failed");
    let verify_time = start.elapsed();

    println!("Verification completed in: {:.2?}", verify_time);
    println!("Verification result: {}", if verification_result { "✓ VALID" } else { "✗ INVALID" });

    assert!(verification_result, "Proof verification failed!");

    // Print summary
    println!("\nSummary");
    println!("-------");
    println!("Polynomial size: 2^24 elements");
    println!("Field size: 32 bits (initial), 128 bits (recursive)");
    println!("Proving time: {:.2?}", prove_time);
    println!("Verification time: {:.2?}", verify_time);
    println!("Proof size: {}", format_bytes(proof_size));
}

fn format_bytes(bytes: usize) -> String {
    const UNITS: &[&str] = &["B", "KB", "MB", "GB"];
    let mut size = bytes as f64;
    let mut unit_idx = 0;

    while size >= 1024.0 && unit_idx < UNITS.len() - 1 {
        size /= 1024.0;
        unit_idx += 1;
    }

    format!("{:.2} {}", size, UNITS[unit_idx])
}
