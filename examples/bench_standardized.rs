/// standardized benchmark for zeratul
use ligerito::*;
use binary_fields::{BinaryElem32, BinaryElem128};
use std::marker::PhantomData;
use std::time::Instant;

fn main() {
    println!("=== zeratul standardized benchmark ===");

    // standard parameters: 2^20 polynomial with seed 1234
    let poly: Vec<BinaryElem32> = (0u32..(1 << 20))
        .map(|i| BinaryElem32::from(i % 0xFFFFFFFFu32))
        .collect();

    let config = hardcoded_config_20(
        PhantomData::<BinaryElem32>,
        PhantomData::<BinaryElem128>,
    );
    let verifier_config = hardcoded_config_20_verifier();

    // benchmark proving
    let prove_start = Instant::now();
    let proof = prove_sha256(&config, &poly).expect("proving failed");
    let prove_time = prove_start.elapsed();

    // measure proof size
    let proof_bytes = bincode::serialize(&proof).expect("serialization failed");
    let proof_size_kb = proof_bytes.len() as f64 / 1024.0;

    // benchmark verification
    let verify_start = Instant::now();
    let result = verify_sha256(&verifier_config, &proof).expect("verification failed");
    let verify_time = verify_start.elapsed();

    println!("proving: {:.2}ms", prove_time.as_secs_f64() * 1000.0);
    println!("verification: {:.2}ms", verify_time.as_secs_f64() * 1000.0);
    println!("proof size: {:.1} KB", proof_size_kb);
    println!("verified: {}", result);
}
