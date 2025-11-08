use ligerito::*;
use binary_fields::{BinaryElem32, BinaryElem128};
use std::marker::PhantomData;
use std::time::Instant;

fn main() {
    println!("=== zeratul standardized benchmark (2^20) ===");
    
    let config = hardcoded_config_20(
        PhantomData::<BinaryElem32>,
        PhantomData::<BinaryElem128>,
    );
    let poly: Vec<BinaryElem32> = (0..1048576)
        .map(|i| BinaryElem32::from(i as u32 % 0xFFFFFFFF))
        .collect();
    
    // Warmup
    println!("warming up...");
    let _ = prove_sha256(&config, &poly).unwrap();
    
    // Benchmark proving
    let start = Instant::now();
    let proof = prove_sha256(&config, &poly).unwrap();
    let prove_time = start.elapsed().as_secs_f64() * 1000.0;
    
    // Benchmark verification
    let verifier_cfg = hardcoded_config_20_verifier();
    let start = Instant::now();
    let result = verify_sha256(&verifier_cfg, &proof).unwrap();
    let verify_time = start.elapsed().as_secs_f64() * 1000.0;
    
    println!("proving: {:.2}ms", prove_time);
    println!("verification: {:.2}ms", verify_time);
    println!("verified: {}", result);
}
