use ligerito::{prove_sha256, hardcoded_config_12};
use binary_fields::{BinaryElem32, BinaryElem128};
use std::fs::File;
use std::io::Write;
use std::marker::PhantomData;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    println!("Creating interoperability test data...");

    // Use the same config as our tests
    let config = hardcoded_config_12(
        PhantomData::<BinaryElem32>,
        PhantomData::<BinaryElem128>,
    );

    // Simple test polynomial - same as in our tests
    let poly: Vec<BinaryElem32> = (0..4096)
        .map(|i| BinaryElem32::from(i as u32))
        .collect();

    println!("Polynomial size: {}", poly.len());
    println!("First few elements: {:?}", &poly[..4.min(poly.len())]);

    // Generate proof
    println!("Generating proof...");
    let proof = prove_sha256(&config, &poly)?;

    println!("Proof generated successfully!");

    // Export key data in simple format for Julia to read
    let export_data = format!(
        "# Rust-generated proof data for Julia verification\n\
         # Polynomial: first 10 elements = {:?}\n\
         # Config: dim={}, k={}, recursive_steps={}\n\
         # Transcript seed: 1234\n\
         # Initial commitment root: {:?}\n\
         # Recursive commitments: {}\n\
         # Final yr length: {}\n\
         # Sumcheck rounds: {}\n",
        &poly[..10.min(poly.len())],
        config.initial_dims.0,
        config.initial_k,
        config.recursive_steps,
        format!("{:?}", proof.initial_ligero_cm.root.root),
        proof.recursive_commitments.len(),
        proof.final_ligero_proof.yr.len(),
        proof.sumcheck_transcript.transcript.len()
    );

    // Write to file
    let mut file = File::create("rust_proof_data.txt")?;
    file.write_all(export_data.as_bytes())?;

    println!("Exported proof data to rust_proof_data.txt");
    println!("Julia can now read this data and attempt verification");

    Ok(())
}