//! Detailed profiling of prover and verifier
use ligerito_binary_fields::{BinaryElem32, BinaryElem128};
use ligerito::{prove_sha256, verify_sha256, hardcoded_config_20, hardcoded_config_20_verifier};
use rand::Rng;
use std::time::Instant;
use std::marker::PhantomData;

fn main() {
    println!("=== LIGERITO DETAILED PROFILING ===\n");
    println!("Polynomial size: 2^20 (1,048,576 elements)\n");

    let config = hardcoded_config_20(
        PhantomData::<BinaryElem32>,
        PhantomData::<BinaryElem128>,
    );

    // Generate random polynomial
    println!("Generating random polynomial...");
    let start = Instant::now();
    let mut rng = rand::thread_rng();
    let poly: Vec<BinaryElem32> = (0..1 << 20)
        .map(|_| BinaryElem32::from(rng.gen::<u32>()))
        .collect();
    println!("  Generated in: {:?}\n", start.elapsed());

    // ============ PROVING ============
    println!("--- PROVING PHASE ---");
    let prove_start = Instant::now();
    let proof = prove_sha256(&config, &poly).expect("Proving failed");
    let prove_time = prove_start.elapsed();
    println!("✓ Total proving time: {:?}\n", prove_time);
    println!("  Proof size: {} bytes ({:.2} KiB)\n",
             proof.size_of(), proof.size_of() as f64 / 1024.0);

    // ============ VERIFICATION ============
    println!("--- VERIFICATION PHASE ---");
    let verifier_config = hardcoded_config_20_verifier();

    // Warmup
    let _ = verify_sha256(&verifier_config, &proof).unwrap();

    // Actual timing - run multiple times for better measurement
    let iterations = 10;
    let mut times = Vec::new();

    for i in 0..iterations {
        let start = Instant::now();
        let result = verify_sha256(&verifier_config, &proof).expect("Verification failed");
        let elapsed = start.elapsed();
        times.push(elapsed);

        if i == 0 {
            println!("  Result: {}", if result { "VALID" } else { "INVALID" });
        }
    }

    // Statistics
    times.sort();
    let min = times[0];
    let max = times[times.len() - 1];
    let median = times[times.len() / 2];
    let mean = times.iter().sum::<std::time::Duration>() / iterations;

    println!("\n✓ Verification statistics ({} runs):", iterations);
    println!("  Min:    {:?}", min);
    println!("  Median: {:?}", median);
    println!("  Mean:   {:?}", mean);
    println!("  Max:    {:?}", max);

    // ============ SUMMARY ============
    println!("\n=== SUMMARY ===");
    println!("Proving:       {:?}", prove_time);
    println!("Verification:  {:?} (median)", median);
    println!("Proof size:    {:.2} KiB", proof.size_of() as f64 / 1024.0);

    // Performance targets
    println!("\n=== TARGETS ===");
    println!("Proving target (paper):      80 ms");
    println!("Current proving:             {:.2} ms", prove_time.as_secs_f64() * 1000.0);
    println!("Gap:                         {:.2}x slower", prove_time.as_secs_f64() / 0.08);

    println!("\nVerification target (goal):  100 ms");
    println!("Current verification:        {:.2} ms", median.as_secs_f64() * 1000.0);
    if median.as_millis() < 100 {
        println!("✨ TARGET ACHIEVED! ✨");
    } else {
        println!("Gap:                         {:.2} ms ({:.1}% over)",
                 median.as_secs_f64() * 1000.0 - 100.0,
                 ((median.as_secs_f64() * 1000.0 / 100.0) - 1.0) * 100.0);
    }
}
