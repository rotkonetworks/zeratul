/// compare merlin vs sha256 transcript performance

use ligerito::{prove, verify, prove_sha256, verify_sha256, hardcoded_config_20, hardcoded_config_20_verifier};
use ligerito_binary_fields::{BinaryElem32, BinaryElem128, BinaryFieldElement};
use std::marker::PhantomData;
use std::time::Instant;

fn main() {
    println!("=== transcript performance comparison ===\n");

    let config = hardcoded_config_20(
        PhantomData::<BinaryElem32>,
        PhantomData::<BinaryElem128>,
    );
    let verifier_config = hardcoded_config_20_verifier();

    println!("polynomial size: 2^20 = 1,048,576 elements\n");

    // create test polynomial
    let poly: Vec<BinaryElem32> = (0..1 << 20)
        .map(|i| BinaryElem32::from(i as u32))
        .collect();

    // benchmark merlin (default)
    println!("--- merlin transcript (default) ---");
    let prove_start = Instant::now();
    let proof_merlin = prove(&config, &poly).expect("merlin prove failed");
    let prove_merlin = prove_start.elapsed();

    let verify_start = Instant::now();
    let result = verify(&verifier_config, &proof_merlin).expect("merlin verify failed");
    let verify_merlin = verify_start.elapsed();

    println!("✓ verification {}", if result { "passed" } else { "failed" });
    println!("  prove:  {:>8.2}ms", prove_merlin.as_secs_f64() * 1000.0);
    println!("  verify: {:>8.2}ms", verify_merlin.as_secs_f64() * 1000.0);
    println!("  total:  {:>8.2}ms", (prove_merlin + verify_merlin).as_secs_f64() * 1000.0);
    println!("  proof:  {:>8} bytes ({:.2} KB)\n", proof_merlin.size_of(), proof_merlin.size_of() as f64 / 1024.0);

    // benchmark sha256
    println!("--- sha256 transcript (julia-compatible) ---");
    let prove_start = Instant::now();
    let proof_sha256 = prove_sha256(&config, &poly).expect("sha256 prove failed");
    let prove_sha256 = prove_start.elapsed();

    let verify_start = Instant::now();
    let result = verify_sha256(&verifier_config, &proof_sha256).expect("sha256 verify failed");
    let verify_sha256 = verify_start.elapsed();

    println!("✓ verification {}", if result { "passed" } else { "failed" });
    println!("  prove:  {:>8.2}ms", prove_sha256.as_secs_f64() * 1000.0);
    println!("  verify: {:>8.2}ms", verify_sha256.as_secs_f64() * 1000.0);
    println!("  total:  {:>8.2}ms", (prove_sha256 + verify_sha256).as_secs_f64() * 1000.0);
    println!("  proof:  {:>8} bytes ({:.2} KB)\n", proof_sha256.size_of(), proof_sha256.size_of() as f64 / 1024.0);

    // comparison
    println!("--- comparison ---");
    let prove_speedup = prove_sha256.as_secs_f64() / prove_merlin.as_secs_f64();
    let verify_speedup = verify_sha256.as_secs_f64() / verify_merlin.as_secs_f64();
    let total_speedup = (prove_sha256 + verify_sha256).as_secs_f64() / (prove_merlin + verify_merlin).as_secs_f64();

    println!("merlin is {:.2}x faster at proving", prove_speedup);
    println!("merlin is {:.2}x faster at verifying", verify_speedup);
    println!("merlin is {:.2}x faster overall", total_speedup);
}
