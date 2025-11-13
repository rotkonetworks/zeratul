/// detailed proof size breakdown to understand why ours is larger

use ligerito::{prove, hardcoded_config_20, hardcoded_config_20_verifier};
use ligerito_binary_fields::{BinaryElem32, BinaryElem128, BinaryFieldElement};
use std::marker::PhantomData;

fn main() {
    println!("=== proof size breakdown analysis ===\n");

    let config = hardcoded_config_20(
        PhantomData::<BinaryElem32>,
        PhantomData::<BinaryElem128>,
    );

    // create test polynomial
    let size = 1 << 20;
    let poly: Vec<BinaryElem32> = (0..size)
        .map(|i| BinaryElem32::from(i as u32))
        .collect();

    let proof = match prove(&config, &poly) {
        Ok(p) => p,
        Err(e) => {
            println!("proof generation failed: {:?}", e);
            return;
        }
    };

    // break down the proof size
    println!("polynomial size: 2^20 = {} elements\n", size);

    let initial_cm_size = proof.initial_ligero_cm.size_of();
    let initial_proof_size = proof.initial_ligero_proof.size_of();
    let recursive_cms_size: usize = proof.recursive_commitments.iter()
        .map(|c| c.size_of())
        .sum();
    let recursive_proofs_size: usize = proof.recursive_proofs.iter()
        .map(|p| p.size_of())
        .sum();
    let final_proof_size = proof.final_ligero_proof.size_of();
    let sumcheck_size = proof.sumcheck_transcript.size_of();
    let total_size = proof.size_of();

    println!("component breakdown:");
    println!("  initial commitment:    {:>8} bytes ({:>6.2}%)", initial_cm_size, 100.0 * initial_cm_size as f64 / total_size as f64);
    println!("  initial proof:         {:>8} bytes ({:>6.2}%)", initial_proof_size, 100.0 * initial_proof_size as f64 / total_size as f64);
    println!("  recursive commitments: {:>8} bytes ({:>6.2}%)", recursive_cms_size, 100.0 * recursive_cms_size as f64 / total_size as f64);
    println!("  recursive proofs:      {:>8} bytes ({:>6.2}%)", recursive_proofs_size, 100.0 * recursive_proofs_size as f64 / total_size as f64);
    println!("  final proof:           {:>8} bytes ({:>6.2}%)", final_proof_size, 100.0 * final_proof_size as f64 / total_size as f64);
    println!("  sumcheck transcript:   {:>8} bytes ({:>6.2}%)", sumcheck_size, 100.0 * sumcheck_size as f64 / total_size as f64);
    println!("  ------------------------");
    println!("  total:                 {:>8} bytes ({:.2} KB)\n", total_size, total_size as f64 / 1024.0);

    // detailed breakdown of initial proof
    println!("initial proof details:");
    let opened_rows_size: usize = proof.initial_ligero_proof.opened_rows.iter()
        .map(|row| row.len() * std::mem::size_of::<BinaryElem32>())
        .sum();
    let initial_merkle_size = proof.initial_ligero_proof.merkle_proof.size_of();
    println!("  opened rows:  {:>8} bytes ({} rows)", opened_rows_size, proof.initial_ligero_proof.opened_rows.len());
    println!("  merkle proof: {:>8} bytes ({} siblings)", initial_merkle_size, proof.initial_ligero_proof.merkle_proof.siblings.len());

    // final proof details
    println!("\nfinal proof details:");
    let yr_size = proof.final_ligero_proof.yr.len() * std::mem::size_of::<BinaryElem128>();
    let final_opened_size: usize = proof.final_ligero_proof.opened_rows.iter()
        .map(|row| row.len() * std::mem::size_of::<BinaryElem128>())
        .sum();
    let final_merkle_size = proof.final_ligero_proof.merkle_proof.size_of();
    println!("  yr vector:    {:>8} bytes ({} elements)", yr_size, proof.final_ligero_proof.yr.len());
    println!("  opened rows:  {:>8} bytes ({} rows)", final_opened_size, proof.final_ligero_proof.opened_rows.len());
    println!("  merkle proof: {:>8} bytes ({} siblings)", final_merkle_size, proof.final_ligero_proof.merkle_proof.siblings.len());

    // sumcheck details
    println!("\nsumcheck transcript details:");
    println!("  {} rounds of (s0, s1, s2) coefficients", proof.sumcheck_transcript.transcript.len());
    println!("  {} bytes per coefficient (BinaryElem128)", std::mem::size_of::<BinaryElem128>());
    println!("  total: {} bytes", sumcheck_size);

    println!("\n=== comparison notes ===");
    println!("ashutosh's proof: 105 KB");
    println!("our proof:        145 KB");
    println!("difference:        40 KB (38% larger)");
    println!("\nlikely reasons:");
    println!("1. our BatchedMerkleProof uses Hash (32 bytes) vs their Vec<u8>");
    println!("2. we use BinaryElem128 (16 bytes) for extension field");
    println!("3. potential padding/alignment differences");
}
