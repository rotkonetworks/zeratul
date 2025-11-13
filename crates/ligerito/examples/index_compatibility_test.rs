use ligerito::{
    prove_sha256, verify_sha256, hardcoded_config_12, hardcoded_config_12_verifier,
    transcript::{FiatShamir, Transcript},
};
use ligerito_binary_fields::{BinaryElem32, BinaryElem128};
use std::marker::PhantomData;

fn main() {
    println!("=== INVESTIGATING INDEX COMPATIBILITY ISSUE ===");

    // This demonstrates the fundamental issue: Julia uses 1-based indexing, Rust uses 0-based
    // Let's trace how indices are generated and used

    let poly: Vec<BinaryElem32> = (0..1 << 12).map(|i| BinaryElem32::from(i as u32)).collect();
    let config = hardcoded_config_12(
        PhantomData::<BinaryElem32>,
        PhantomData::<BinaryElem128>,
    );

    // Generate proof and examine query generation
    println!("Testing query generation compatibility...");

    let mut fs_rust = FiatShamir::new_sha256(1234);

    // Simulate the same sequence as prover
    // Initial commitment would be absorbed here (not recreated for simplicity)
    let dummy_root = ligerito_merkle::MerkleRoot { root: Some([1u8; 32]) };
    fs_rust.absorb_root(&dummy_root);

    // Get initial challenges like prover does
    let _initial_challenges: Vec<BinaryElem32> = (0..4).map(|_| fs_rust.get_challenge()).collect();

    // Second commitment absorbed
    fs_rust.absorb_root(&dummy_root);

    // Now get queries - this is where the issue manifests
    let rust_queries = fs_rust.get_distinct_queries(1024, 148); // 1024 total, 148 queries (0-based)

    println!("First 10 Rust queries (0-based): {:?}", &rust_queries[..10]);
    println!("Max Rust query: {}", rust_queries.iter().max().unwrap_or(&0));
    println!("Min Rust query: {}", rust_queries.iter().min().unwrap_or(&0));

    // For comparison with Julia: Julia would generate 1-based indices (1..=1024)
    // Then when accessing arrays, Julia's 1-based indexing works naturally
    // But when we prove() in Rust, we need to pass 0-based indices to Merkle tree

    println!("\n=== DEMONSTRATING THE FIX ===");

    // The issue is in how we interpret the queries when accessing the Merkle tree
    // Julia generates 1-based queries but its Merkle tree expects 1-based indices
    // Rust generates 0-based queries and its Merkle tree expects 0-based indices

    // Let's test if our current implementation maintains consistency
    match prove_sha256(&config, &poly) {
        Ok(proof) => {
            println!("✓ Proof generation successful");

            // The key insight: the queries in the proof should match between prover and verifier
            // Let's examine the final opened rows count
            println!("Initial opened rows count: {}", proof.initial_ligero_proof.opened_rows.len());
            println!("Final opened rows count: {}", proof.final_ligero_proof.opened_rows.len());

            // Now try verification
            let verifier_config = hardcoded_config_12_verifier();
            match verify_sha256(&verifier_config, &proof) {
                Ok(result) => {
                    if result {
                        println!("✓ Verification successful - indexing is consistent!");
                    } else {
                        println!("✗ Verification failed - but no error thrown, so issue might be elsewhere");
                    }
                },
                Err(e) => {
                    println!("✗ Verification error: {:?}", e);
                }
            }
        },
        Err(e) => {
            println!("✗ Proof generation failed: {:?}", e);
        }
    }

    println!("\n=== TESTING INDIVIDUAL COMPONENTS ===");
    test_merkle_tree_indexing();
}

fn test_merkle_tree_indexing() {
    use ligerito_merkle::{build_merkle_tree, verify};

    println!("Testing Merkle tree indexing directly...");

    // Create a small test case
    let leaves: Vec<u64> = (0..16).collect(); // 16 leaves: [0, 1, 2, ..., 15]
    let tree = build_merkle_tree(&leaves);

    // Test with 0-based queries as our code should use
    let queries_0_based = vec![0, 5, 10, 15]; // 0-based indices
    let proof = tree.prove(&queries_0_based);

    let queried_leaves: Vec<u64> = queries_0_based.iter().map(|&i| leaves[i]).collect();

    let verification_result = verify(
        &tree.get_root(),
        &proof,
        tree.get_depth(),
        &queried_leaves,
        &queries_0_based
    );

    println!("Merkle tree verification (0-based): {}", verification_result);

    if !verification_result {
        println!("ERROR: Basic Merkle tree verification failed!");
        println!("This suggests a deeper issue in the Merkle tree implementation");

        // Debug info
        println!("Tree depth: {}", tree.get_depth());
        println!("Number of queries: {}", queries_0_based.len());
        println!("Proof siblings count: {}", proof.siblings.len());
        println!("Queried leaves: {:?}", queried_leaves);
        println!("Query indices: {:?}", queries_0_based);
    }
}