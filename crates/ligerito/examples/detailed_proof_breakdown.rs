// detailed proof size breakdown to understand why ashutosh's proofs are smaller
use ligerito::{prove_sha256, hardcoded_config_20};
use binary_fields::{BinaryElem32, BinaryElem128};
use std::marker::PhantomData;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    println!("=== detailed proof size breakdown ===\n");

    // create config for 2^20
    let config = hardcoded_config_20(
        PhantomData::<BinaryElem32>,
        PhantomData::<BinaryElem128>,
    );

    // generate test polynomial
    let poly: Vec<BinaryElem32> = (0..1048576)
        .map(|i| BinaryElem32::from(i as u32))
        .collect();

    println!("generating proof for 2^20 elements...\n");
    let proof = prove_sha256(&config, &poly)?;

    // serialize entire proof
    let full_serialized = bincode::serialize(&proof)?;
    println!("=== total proof size ===");
    println!("full serialized: {} bytes ({:.2} KB)\n", full_serialized.len(), full_serialized.len() as f64 / 1024.0);

    // analyze each component
    println!("=== component breakdown ===\n");

    // 1. initial commitment
    let initial_cm_size = bincode::serialize(&proof.initial_ligero_cm)?.len();
    println!("initial_ligero_cm:");
    println!("  size: {} bytes", initial_cm_size);
    println!("  root: {:?}\n", proof.initial_ligero_cm.root);

    // 2. initial proof
    let initial_proof_size = bincode::serialize(&proof.initial_ligero_proof)?.len();
    println!("initial_ligero_proof:");
    println!("  total size: {} bytes ({:.2} KB)", initial_proof_size, initial_proof_size as f64 / 1024.0);
    println!("  opened_rows: {} rows", proof.initial_ligero_proof.opened_rows.len());
    if !proof.initial_ligero_proof.opened_rows.is_empty() {
        let row_size = std::mem::size_of::<BinaryElem32>() * proof.initial_ligero_proof.opened_rows[0].len();
        let total_rows_size = row_size * proof.initial_ligero_proof.opened_rows.len();
        println!("    - row size: {} bytes ({} elements × {} bytes)",
            row_size,
            proof.initial_ligero_proof.opened_rows[0].len(),
            std::mem::size_of::<BinaryElem32>()
        );
        println!("    - total rows: {} bytes ({:.2} KB)", total_rows_size, total_rows_size as f64 / 1024.0);

        let merkle_size = initial_proof_size - total_rows_size - 8; // -8 for Vec header
        println!("  merkle_proof:");
        println!("    - siblings: {} hashes", proof.initial_ligero_proof.merkle_proof.siblings.len());
        println!("    - size: {} bytes ({:.2} KB)", merkle_size, merkle_size as f64 / 1024.0);
    }
    println!();

    // 3. recursive commitments
    let recursive_cm_size = bincode::serialize(&proof.recursive_commitments)?.len();
    println!("recursive_commitments:");
    println!("  count: {}", proof.recursive_commitments.len());
    println!("  total size: {} bytes\n", recursive_cm_size);

    // 4. recursive proofs
    let recursive_proofs_size = bincode::serialize(&proof.recursive_proofs)?.len();
    println!("recursive_proofs:");
    println!("  count: {}", proof.recursive_proofs.len());
    println!("  total size: {} bytes\n", recursive_proofs_size);

    // 5. final proof
    let final_proof_size = bincode::serialize(&proof.final_ligero_proof)?.len();
    println!("final_ligero_proof:");
    println!("  total size: {} bytes ({:.2} KB)", final_proof_size, final_proof_size as f64 / 1024.0);

    let yr_size = std::mem::size_of::<BinaryElem128>() * proof.final_ligero_proof.yr.len();
    println!("  yr vector: {} elements", proof.final_ligero_proof.yr.len());
    println!("    - size: {} bytes ({:.2} KB)", yr_size, yr_size as f64 / 1024.0);

    println!("  opened_rows: {} rows", proof.final_ligero_proof.opened_rows.len());
    if !proof.final_ligero_proof.opened_rows.is_empty() {
        let row_size = std::mem::size_of::<BinaryElem128>() * proof.final_ligero_proof.opened_rows[0].len();
        let total_rows_size = row_size * proof.final_ligero_proof.opened_rows.len();
        println!("    - row size: {} bytes ({} elements × {} bytes)",
            row_size,
            proof.final_ligero_proof.opened_rows[0].len(),
            std::mem::size_of::<BinaryElem128>()
        );
        println!("    - total rows: {} bytes ({:.2} KB)", total_rows_size, total_rows_size as f64 / 1024.0);

        let merkle_size = final_proof_size - yr_size - total_rows_size - 16; // -16 for Vec headers
        println!("  merkle_proof:");
        println!("    - siblings: {} hashes", proof.final_ligero_proof.merkle_proof.siblings.len());
        println!("    - size: {} bytes ({:.2} KB)", merkle_size, merkle_size as f64 / 1024.0);
    }
    println!();

    // 6. sumcheck transcript
    let sumcheck_size = bincode::serialize(&proof.sumcheck_transcript)?.len();
    println!("sumcheck_transcript:");
    println!("  rounds: {}", proof.sumcheck_transcript.transcript.len());
    println!("  size: {} bytes\n", sumcheck_size);

    // summary
    println!("=== summary ===\n");

    let merkle_total = proof.initial_ligero_proof.merkle_proof.siblings.len() +
                       proof.final_ligero_proof.merkle_proof.siblings.len();
    let merkle_total_bytes = merkle_total * 32;

    let opened_rows_total = proof.initial_ligero_proof.opened_rows.len() +
                           proof.final_ligero_proof.opened_rows.len();

    println!("merkle proofs:");
    println!("  total siblings: {}", merkle_total);
    println!("  total size: {} bytes ({:.2} KB) - {:.1}% of proof",
        merkle_total_bytes,
        merkle_total_bytes as f64 / 1024.0,
        merkle_total_bytes as f64 / full_serialized.len() as f64 * 100.0
    );

    println!("\nopened rows:");
    println!("  total rows: {}", opened_rows_total);
    println!("  estimated size: {:.2} KB - {:.1}% of proof",
        (initial_proof_size + final_proof_size - merkle_total_bytes) as f64 / 1024.0,
        (initial_proof_size + final_proof_size - merkle_total_bytes) as f64 / full_serialized.len() as f64 * 100.0
    );

    println!("\nother (commitments + transcript + overhead):");
    let other_size = full_serialized.len() - initial_proof_size - final_proof_size;
    println!("  size: {} bytes ({:.2} KB) - {:.1}% of proof",
        other_size,
        other_size as f64 / 1024.0,
        other_size as f64 / full_serialized.len() as f64 * 100.0
    );

    // comparison with ashutosh
    println!("\n=== comparison with ashutosh (105 KB) ===\n");

    let ashutosh_size = 105.0 * 1024.0;
    let difference = full_serialized.len() as f64 - ashutosh_size;

    println!("our size:       {:.2} KB", full_serialized.len() as f64 / 1024.0);
    println!("ashutosh size:  105.00 KB");
    println!("difference:     {:.2} KB ({:.1}% larger)\n", difference / 1024.0, (difference / ashutosh_size) * 100.0);

    // hypothesize where the difference comes from
    println!("likely causes of size difference:\n");

    println!("1. merkle sibling deduplication:");
    println!("   - our {} siblings could deduplicate to ~50-60%", merkle_total);
    println!("   - potential savings: ~{:.2} KB\n", merkle_total_bytes as f64 * 0.4 / 1024.0);

    println!("2. serialization format:");
    println!("   - bincode adds Vec<T> length prefixes (8 bytes each)");
    println!("   - we have {} Vec allocations in the proof structure",
        2 + // opened_rows in initial + final
        2 + // merkle siblings in initial + final
        1 + // recursive_commitments
        1 + // recursive_proofs
        1   // yr vector
    );
    println!("   - overhead: ~56 bytes from Vec headers");
    println!("   - ashutosh might use flatter structure\n");

    println!("3. compression:");
    println!("   - ashutosh might compress their proofs");
    println!("   - but this seems unlikely (reported as 105 KB, not compressed size)\n");

    println!("4. different parameters:");
    println!("   - ashutosh might use fewer queries (not 148)");
    println!("   - or different rate encoding");
    println!("   - need to verify their config matches ours\n");

    println!("=== conclusion ===\n");
    println!("our merkle proofs are the main size contributor:");
    println!("  - {} siblings × 32 bytes = {:.2} KB ({}% of total)",
        merkle_total,
        merkle_total_bytes as f64 / 1024.0,
        (merkle_total_bytes as f64 / full_serialized.len() as f64 * 100.0) as i32
    );
    println!("\nto match ashutosh's 105 KB, we'd need:");
    println!("  - deduplicate ~40% of merkle siblings (-{:.2} KB)", merkle_total_bytes as f64 * 0.4 / 1024.0);
    println!("  - or use compression (16% reduction = -{:.2} KB)", difference * 0.16 / 1024.0);
    println!("  - or verify they use same parameters as us");

    Ok(())
}
