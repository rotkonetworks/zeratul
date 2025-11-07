/// standardized benchmark for zeratul 2^28
use ligerito::*;
use binary_fields::{BinaryElem32, BinaryElem128};
use std::marker::PhantomData;
use std::time::Instant;

fn main() {
    println!("=== zeratul standardized benchmark (2^28) ===");

    let poly: Vec<BinaryElem32> = (0u32..(1 << 28))
        .map(|i| BinaryElem32::from(i % 0xFFFFFFFFu32))
        .collect();

    let config = hardcoded_config_28(
        PhantomData::<BinaryElem32>,
        PhantomData::<BinaryElem128>,
    );
    let verifier_config = hardcoded_config_28_verifier();

    let prove_start = Instant::now();
    let proof = prove_sha256(&config, &poly).expect("proving failed");
    let prove_time = prove_start.elapsed();

    let verify_start = Instant::now();
    let result = verify_sha256(&verifier_config, &proof).expect("verification failed");
    let verify_time = verify_start.elapsed();

    println!("proving: {:.2}ms", prove_time.as_secs_f64() * 1000.0);
    println!("verification: {:.2}ms", verify_time.as_secs_f64() * 1000.0);
    println!("verified: {}", result);
}
