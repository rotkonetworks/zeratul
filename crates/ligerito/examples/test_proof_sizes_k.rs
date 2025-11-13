/// Test proof sizes with different k values
///
/// Generates actual proofs and measures serialized sizes

use binary_fields::{BinaryElem128, BinaryElem32, BinaryFieldElement};
use ligerito::{prove, configs::{
    hardcoded_config_20, hardcoded_config_20_k8, hardcoded_config_20_k10,
}};
use std::marker::PhantomData;

fn main() {
    println!("╔═══════════════════════════════════════════════════════════════════════╗");
    println!("║  Proof Size Comparison (n=20, varying k)                             ║");
    println!("╚═══════════════════════════════════════════════════════════════════════╝\n");

    // Generate test polynomial (2^20 elements)
    let n = 20;
    let table: Vec<BinaryElem32> = (0..(1 << n))
        .map(|i| BinaryElem32::from(i as u32))
        .collect();

    println!("Table size: 2^{} = {} elements ({} MB)\n", n, table.len(), (table.len() * 4) / (1024 * 1024));

    // Test k=6 (current default)
    println!("═══ k=6 (default) ═══");
    let config_k6 = hardcoded_config_20(PhantomData::<BinaryElem32>, PhantomData::<BinaryElem128>);
    test_proof_size("k=6", &table, config_k6);

    // Test k=8
    println!("\n═══ k=8 (GPU-optimized) ═══");
    let config_k8 = hardcoded_config_20_k8(PhantomData::<BinaryElem32>, PhantomData::<BinaryElem128>);
    test_proof_size("k=8", &table, config_k8);

    // Test k=10
    println!("\n═══ k=10 (max dot product) ═══");
    let config_k10 = hardcoded_config_20_k10(PhantomData::<BinaryElem32>, PhantomData::<BinaryElem128>);
    test_proof_size("k=10", &table, config_k10);

    println!("\n╔═══════════════════════════════════════════════════════════════════════╗");
    println!("║  Summary                                                              ║");
    println!("╚═══════════════════════════════════════════════════════════════════════╝");
}

fn test_proof_size(
    label: &str,
    table: &[BinaryElem32],
    config: ligerito::data_structures::ProverConfig<BinaryElem32, BinaryElem128>,
) {
    use std::time::Instant;

    println!("Config: {}", label);
    println!("  Matrix dims: 2^{} × 2^{}",
        config.initial_dims.0.trailing_zeros(),
        config.initial_dims.1.trailing_zeros());

    let start = Instant::now();
    let proof = match prove(&config, table) {
        Ok(p) => p,
        Err(e) => {
            println!("  Proof generation failed: {:?}", e);
            return;
        }
    };
    let prove_time = start.elapsed();

    let total_size = proof.size_of();
    let initial_proof_size = proof.initial_ligero_proof.size_of();
    let final_proof_size = proof.final_ligero_proof.size_of();
    let sumcheck_size = proof.sumcheck_transcript.size_of();

    println!("  Proving time: {:.2?}", prove_time);
    println!("  Components:");
    println!("    - Initial proof:  {:>8} bytes", initial_proof_size);
    println!("    - Final proof:    {:>8} bytes", final_proof_size);
    println!("    - Sumcheck:       {:>8} bytes", sumcheck_size);
    println!("    - Recursive:      {:>8} bytes", total_size - initial_proof_size - final_proof_size - sumcheck_size - 32);
    println!("  Total proof size: {:>8} bytes ({:.2} KB)", total_size, total_size as f64 / 1024.0);
}
