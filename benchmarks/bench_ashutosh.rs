/// standardized benchmark for ashutosh-ligerito
use cryptoutils::*;
use std::time::Instant;

fn main() {
    println!("=== ashutosh-ligerito standardized benchmark ===");

    // standard parameters: 2^20 polynomial with seed 1234
    let poly: Vec<BinaryElem32> = (0..(1 << 20))
        .map(|i| BinaryElem32::new((i % 0xFFFFFFFF) as u32))
        .collect();

    let prover_config: ProverConfig<BinaryElem32> = hardcoded_config_20::<BinaryElem32>();
    let verifier_config = hardcoded_config_20_verifier();

    // benchmark proving
    let prove_start = Instant::now();
    let proof = prover(prover_config, poly);
    let prove_time = prove_start.elapsed();

    // benchmark verification
    let verify_start = Instant::now();
    let result = verifier(verifier_config, proof);
    let verify_time = verify_start.elapsed();

    println!("proving: {:.2}ms", prove_time.as_secs_f64() * 1000.0);
    println!("verification: {:.2}ms", verify_time.as_secs_f64() * 1000.0);
    println!("verified: {}", result);
}
