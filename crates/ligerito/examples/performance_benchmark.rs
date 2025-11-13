/// proper performance comparison: our implementation vs ashutosh1206
/// testing same polynomial sizes

use ligerito::{prove, verify, hardcoded_config_12, hardcoded_config_12_verifier, hardcoded_config_20, hardcoded_config_20_verifier};
use binary_fields::{BinaryElem32, BinaryElem128, BinaryFieldElement};
use std::marker::PhantomData;
use std::time::Instant;

fn benchmark_size(log_size: usize) {
    let size = 1 << log_size;
    println!("\n=== polynomial size: 2^{} = {} elements ===", log_size, size);

    let (config, verifier_config) = match log_size {
        12 => (
            hardcoded_config_12(PhantomData::<BinaryElem32>, PhantomData::<BinaryElem128>),
            hardcoded_config_12_verifier()
        ),
        20 => (
            hardcoded_config_20(PhantomData::<BinaryElem32>, PhantomData::<BinaryElem128>),
            hardcoded_config_20_verifier()
        ),
        _ => {
            println!("skipping (no config available)");
            return;
        }
    };

    // create test polynomial
    let poly: Vec<BinaryElem32> = (0..size)
        .map(|i| BinaryElem32::from(i as u32))
        .collect();

    // benchmark prover
    let prove_start = Instant::now();
    let proof = match prove(&config, &poly) {
        Ok(p) => p,
        Err(e) => {
            println!("proof generation failed: {:?}", e);
            return;
        }
    };
    let prove_time = prove_start.elapsed();

    // benchmark verifier
    let verify_start = Instant::now();
    let result = match verify(&verifier_config, &proof) {
        Ok(r) => r,
        Err(e) => {
            println!("verification failed: {:?}", e);
            return;
        }
    };
    let verify_time = verify_start.elapsed();

    if result {
        println!("✓ verification passed");
        println!("  prove time:  {:>8.2}ms", prove_time.as_secs_f64() * 1000.0);
        println!("  verify time: {:>8.2}ms", verify_time.as_secs_f64() * 1000.0);
        println!("  total time:  {:>8.2}ms", (prove_time + verify_time).as_secs_f64() * 1000.0);

        // estimate proof size
        let proof_size = proof.size_of();
        println!("  proof size:  {:>8} bytes ({:.2} KB)", proof_size, proof_size as f64 / 1024.0);
    } else {
        println!("✗ verification failed");
    }
}

fn main() {
    println!("=== ligerito performance benchmark ===");
    println!("our implementation with merlin transcript");

    // test sizes we have configs for
    benchmark_size(12);  // 4096 elements
    benchmark_size(20);  // 1,048,576 elements - direct comparison with ashutosh

    println!("\n=== comparison notes ===");
    println!("to compare with ashutosh1206:");
    println!("  cd ../ashutosh-ligerito");
    println!("  cargo run --release");
    println!("\nashutosh results on 2^20:");
    println!("  prove:  3,600ms");
    println!("  verify:   279ms");
    println!("  total:  3,880ms");
}
