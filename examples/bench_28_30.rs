use ligerito::*;
use binary_fields::{BinaryElem32, BinaryElem128};
use std::marker::PhantomData;
use std::time::Instant;

fn main() {
    println!("=== zeratul benchmarks (2^28 and 2^30) ===");

    // 2^28
    println!("\n2^28 (268,435,456 elements):");
    let config_28 = hardcoded_config_28(
        PhantomData::<BinaryElem32>,
        PhantomData::<BinaryElem128>,
    );
    let poly_28: Vec<BinaryElem32> = (0..268435456)
        .map(|i| BinaryElem32::from((i as u32) % 0xFFFFFFFF))
        .collect();

    // warmup
    println!("  warming up...");
    let _ = prove_sha256(&config_28, &poly_28).unwrap();

    // benchmark proving (4 iterations)
    let mut prove_times = Vec::new();
    for i in 1..=4 {
        print!("  prove run {}/4... ", i);
        let start = Instant::now();
        let _proof = prove_sha256(&config_28, &poly_28).unwrap();
        let time = start.elapsed().as_secs_f64();
        prove_times.push(time);
        println!("{:.2}s", time);
    }
    let avg_prove = prove_times.iter().sum::<f64>() / prove_times.len() as f64;
    let min_prove = prove_times.iter().cloned().fold(f64::INFINITY, f64::min);

    // benchmark verification (10 iterations with same proof)
    let proof_28 = prove_sha256(&config_28, &poly_28).unwrap();
    let verifier_cfg_28 = hardcoded_config_28_verifier();
    let mut verify_times = Vec::new();
    for i in 1..=10 {
        let start = Instant::now();
        let _result = verify_sha256(&verifier_cfg_28, &proof_28).unwrap();
        verify_times.push(start.elapsed().as_secs_f64() * 1000.0);
    }
    let avg_verify = verify_times.iter().sum::<f64>() / verify_times.len() as f64;
    let min_verify = verify_times.iter().cloned().fold(f64::INFINITY, f64::min);

    println!("  proving: avg={:.2}s, min={:.2}s", avg_prove, min_prove);
    println!("  verification: avg={:.2}ms, min={:.2}ms", avg_verify, min_verify);

    // 2^30
    println!("\n2^30 (1,073,741,824 elements):");
    let config_30 = hardcoded_config_30(
        PhantomData::<BinaryElem32>,
        PhantomData::<BinaryElem128>,
    );
    let poly_30: Vec<BinaryElem32> = (0..1073741824)
        .map(|i| BinaryElem32::from((i as u32) % 0xFFFFFFFF))
        .collect();

    // warmup
    println!("  warming up...");
    let _ = prove_sha256(&config_30, &poly_30).unwrap();

    // benchmark proving (2 iterations - this is huge)
    let mut prove_times_30 = Vec::new();
    for i in 1..=2 {
        print!("  prove run {}/2... ", i);
        let start = Instant::now();
        let _proof = prove_sha256(&config_30, &poly_30).unwrap();
        let time = start.elapsed().as_secs_f64();
        prove_times_30.push(time);
        println!("{:.2}s", time);
    }
    let avg_prove_30 = prove_times_30.iter().sum::<f64>() / prove_times_30.len() as f64;
    let min_prove_30 = prove_times_30.iter().cloned().fold(f64::INFINITY, f64::min);

    // benchmark verification (10 iterations)
    let proof_30 = prove_sha256(&config_30, &poly_30).unwrap();
    let verifier_cfg_30 = hardcoded_config_30_verifier();
    let mut verify_times_30 = Vec::new();
    for i in 1..=10 {
        let start = Instant::now();
        let _result = verify_sha256(&verifier_cfg_30, &proof_30).unwrap();
        verify_times_30.push(start.elapsed().as_secs_f64() * 1000.0);
    }
    let avg_verify_30 = verify_times_30.iter().sum::<f64>() / verify_times_30.len() as f64;
    let min_verify_30 = verify_times_30.iter().cloned().fold(f64::INFINITY, f64::min);

    println!("  proving: avg={:.2}s, min={:.2}s", avg_prove_30, min_prove_30);
    println!("  verification: avg={:.2}ms, min={:.2}ms", avg_verify_30, min_verify_30);
}
