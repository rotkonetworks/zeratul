/// comprehensive benchmark: merlin and sha256 at multiple sizes

use ligerito::{prove, verify, prove_sha256, verify_sha256};
use ligerito::{hardcoded_config_12, hardcoded_config_12_verifier};
use ligerito::{hardcoded_config_20, hardcoded_config_20_verifier};
use binary_fields::{BinaryElem32, BinaryElem128, BinaryFieldElement};
use std::marker::PhantomData;
use std::time::Instant;

fn benchmark_config(
    size_log: usize,
    size: usize,
    poly: &[BinaryElem32],
    prover_config: &ligerito::ProverConfig<BinaryElem32, BinaryElem128>,
    verifier_config: &ligerito::VerifierConfig,
) {
    println!("\n=== 2^{} = {} elements ===", size_log, size);

    // merlin
    println!("\nmerlin transcript:");
    let prove_start = Instant::now();
    let proof_merlin = prove(prover_config, poly).expect("merlin prove failed");
    let prove_time = prove_start.elapsed();

    let verify_start = Instant::now();
    let result = verify(verifier_config, &proof_merlin).expect("merlin verify failed");
    let verify_time = verify_start.elapsed();

    let proof_size_merlin = proof_merlin.size_of();

    println!("  ✓ verification {}", if result { "passed" } else { "FAILED" });
    println!("  prove:      {:>8.2}ms", prove_time.as_secs_f64() * 1000.0);
    println!("  verify:     {:>8.2}ms", verify_time.as_secs_f64() * 1000.0);
    println!("  total:      {:>8.2}ms", (prove_time + verify_time).as_secs_f64() * 1000.0);
    println!("  proof size: {:>8} bytes ({:.2} KB)", proof_size_merlin, proof_size_merlin as f64 / 1024.0);

    // sha256
    println!("\nsha256 transcript:");
    let prove_start = Instant::now();
    let proof_sha256 = prove_sha256(prover_config, poly).expect("sha256 prove failed");
    let prove_time = prove_start.elapsed();

    let verify_start = Instant::now();
    let result = verify_sha256(verifier_config, &proof_sha256).expect("sha256 verify failed");
    let verify_time = verify_start.elapsed();

    let proof_size_sha256 = proof_sha256.size_of();

    println!("  ✓ verification {}", if result { "passed" } else { "FAILED" });
    println!("  prove:      {:>8.2}ms", prove_time.as_secs_f64() * 1000.0);
    println!("  verify:     {:>8.2}ms", verify_time.as_secs_f64() * 1000.0);
    println!("  total:      {:>8.2}ms", (prove_time + verify_time).as_secs_f64() * 1000.0);
    println!("  proof size: {:>8} bytes ({:.2} KB)", proof_size_sha256, proof_size_sha256 as f64 / 1024.0);
}

fn main() {
    println!("=== comprehensive ligerito benchmark ===");
    println!("testing both merlin and sha256 transcripts\n");

    // 2^12
    let config_12 = hardcoded_config_12(
        PhantomData::<BinaryElem32>,
        PhantomData::<BinaryElem128>,
    );
    let verifier_config_12 = hardcoded_config_12_verifier();
    let poly_12: Vec<BinaryElem32> = (0..1 << 12)
        .map(|i| BinaryElem32::from(i as u32))
        .collect();

    benchmark_config(12, 1 << 12, &poly_12, &config_12, &verifier_config_12);

    // 2^20
    let config_20 = hardcoded_config_20(
        PhantomData::<BinaryElem32>,
        PhantomData::<BinaryElem128>,
    );
    let verifier_config_20 = hardcoded_config_20_verifier();
    let poly_20: Vec<BinaryElem32> = (0..1 << 20)
        .map(|i| BinaryElem32::from(i as u32))
        .collect();

    benchmark_config(20, 1 << 20, &poly_20, &config_20, &verifier_config_20);

    println!("\n=== comparison notes ===");
    println!("julia 2^20:        prove 3,262ms, verify 383ms, total 3,645ms, proof 147 KB");
    println!("ashutosh 2^20:     prove 3,600ms, verify 279ms, total 3,880ms, proof 105 KB");
}
