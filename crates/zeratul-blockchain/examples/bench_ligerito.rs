//! Quick Ligerito performance test
//!
//! Tests proof generation time and size with SIMD

use ligerito::{prove_sha256, verify_sha256, hardcoded_config_20, hardcoded_config_20_verifier};
use ligerito_binary_fields::{BinaryElem32, BinaryElem128};
use std::marker::PhantomData;
use std::time::Instant;

fn main() {
    println!("🔥 Ligerito Performance Test\n");

    // Config for 2^20 polynomial (1M elements)
    let config = hardcoded_config_20(
        PhantomData::<BinaryElem32>,
        PhantomData::<BinaryElem128>,
    );

    println!("Config: 2^20 polynomial (1,048,576 elements)");
    println!("Element size: 4 bytes (BinaryElem32)");
    println!("Total data: 4 MB\n");

    // Generate random polynomial
    println!("Generating random polynomial...");
    let poly: Vec<BinaryElem32> = (0..(1 << 20))
        .map(|i| BinaryElem32::from(i as u32))
        .collect();
    println!("✅ Polynomial generated\n");

    // Prove
    println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
    println!("PROVING");
    println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━\n");

    let start = Instant::now();
    let proof = prove_sha256(&config, &poly).expect("Proving failed");
    let prove_time = start.elapsed();

    println!("⏱️  Prove time: {:?}", prove_time);

    // Serialize proof
    let proof_bytes = bincode::serialize(&proof).expect("Serialization failed");
    let proof_size_kb = proof_bytes.len() as f64 / 1024.0;

    println!("📦 Proof size: {} bytes ({:.2} KB)", proof_bytes.len(), proof_size_kb);
    println!();

    // Verify
    println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
    println!("VERIFYING");
    println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━\n");

    let verifier_config = hardcoded_config_20_verifier();

    let start = Instant::now();
    let valid = verify_sha256(&verifier_config, &proof).expect("Verification failed");
    let verify_time = start.elapsed();

    println!("⏱️  Verify time: {:?}", verify_time);
    println!("✅ Valid: {}", valid);
    println!();

    // Summary
    println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
    println!("📊 SUMMARY");
    println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━\n");

    println!("Polynomial: 2^20 = 1,048,576 elements (4 MB)");
    println!("Prove time: {:?}", prove_time);
    println!("Verify time: {:?}", verify_time);
    println!("Proof size: {:.2} KB", proof_size_kb);
    println!();

    println!("Build mode: {}", if cfg!(debug_assertions) { "DEBUG ⚠️" } else { "RELEASE ✅" });
    println!("SIMD: {}", if cfg!(target_feature = "avx2") { "AVX2 ✅" } else { "disabled ⚠️" });
    println!();

    if cfg!(debug_assertions) {
        println!("⚠️  WARNING: Running in debug mode!");
        println!("    For accurate benchmarks, run:");
        println!("    RUSTFLAGS=\"-C target-cpu=native\" cargo run --release --example bench_ligerito");
    }
}
