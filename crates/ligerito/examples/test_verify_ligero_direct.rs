use ligerito::{prove_sha256, hardcoded_config_12};
use ligerito::ligero::verify_ligero;
use ligerito_binary_fields::{BinaryElem32, BinaryElem128};
use std::marker::PhantomData;

fn main() {
    println!("=== TESTING VERIFY_LIGERO DIRECTLY ===");

    // Generate a proof
    let config = hardcoded_config_12(
        PhantomData::<BinaryElem32>,
        PhantomData::<BinaryElem128>,
    );

    let poly: Vec<BinaryElem32> = (0..4096)
        .map(|i| BinaryElem32::from(i as u32))
        .collect();

    println!("Generating proof...");
    let proof = prove_sha256(&config, &poly).unwrap();

    // Extract the data
    let yr = &proof.final_ligero_proof.yr;
    let opened_rows = &proof.final_ligero_proof.opened_rows;

    println!("yr length: {}", yr.len());
    println!("opened_rows length: {}", opened_rows.len());

    // The challenge we need is what was used in the folding
    // For now, let's test with a simple challenge to see if that's the issue
    let simple_challenges = vec![BinaryElem128::from(1), BinaryElem128::from(2)];

    println!("Testing with simple challenges: {:?}", simple_challenges);

    // Test just a few queries to avoid spam
    let test_queries = vec![0, 1, 2];

    for &query in &test_queries {
        if query < opened_rows.len() {
            let single_row = vec![opened_rows[query].clone()];
            let single_query = vec![query];

            println!("\n=== Testing query {} ===", query);

            // Test if verify_ligero panics
            let result = std::panic::catch_unwind(|| {
                verify_ligero(&single_query, &single_row, yr, &simple_challenges);
            });

            match result {
                Ok(()) => println!("Query {} PASSED verify_ligero", query),
                Err(_) => println!("Query {} FAILED verify_ligero", query),
            }
        }
    }

    // The key insight: verify_ligero is checking if the folding relation holds
    // If it fails with simple challenges, it means either:
    // 1. The opened rows don't correspond to yr (wrong data)
    // 2. The challenges are wrong
    // 3. There's a bug in our implementation

    println!("\nIf all queries fail, it suggests the challenges are wrong.");
    println!("If some pass and some fail, it suggests inconsistent data.");
    println!("If all pass, our implementation works and the issue is elsewhere.");
}