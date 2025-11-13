use ligerito_binary_fields::{BinaryElem32, BinaryElem128};
use ligerito::{prove, verify, configs};
use std::time::Instant;

fn main() {
    println!("=== Verifier Profiling for 2^20 ===\n");

    // Generate test polynomial
    let n = 1 << 20;
    let poly: Vec<BinaryElem32> = (0..n)
        .map(|i| BinaryElem32::from_bits((i % 256) as u64))
        .collect();

    // Get configs
    let prover_config = configs::hardcoded_config_20_prover();
    let verifier_config = configs::hardcoded_config_20_verifier();

    // Prove (we don't time this)
    println!("Generating proof...");
    let proof = prove(&prover_config, &poly).expect("Proving failed");
    println!("Proof generated.\n");

    // Now profile verification with instrumentation
    println!("Running instrumented verification...\n");

    let iterations = 10;
    let mut total_time = 0u128;

    for i in 0..iterations {
        let start = Instant::now();
        let result = verify::<BinaryElem32, BinaryElem128>(&verifier_config, &proof)
            .expect("Verification failed");
        let elapsed = start.elapsed();

        if !result {
            eprintln!("Verification failed on iteration {}", i);
            return;
        }

        total_time += elapsed.as_micros();

        if i == 0 {
            println!("First run (cold cache): {:.2}ms", elapsed.as_secs_f64() * 1000.0);
        }
    }

    let avg_time = total_time as f64 / iterations as f64 / 1000.0;
    println!("\nAverage verification time over {} runs: {:.2}ms", iterations, avg_time);

    // Now let's add manual instrumentation for key operations
    println!("\n=== Manual instrumentation ===");
    println!("Run with DETAILED_TIMING=1 for per-operation timing");
}
