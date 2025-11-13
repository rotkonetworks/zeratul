use ligerito_merkle::{build_merkle_tree, Hash};
use ligerito::ligero::hash_row;
use ligerito_binary_fields::BinaryElem32;

fn main() {
    println!("=== TESTING MERKLE TREE ===");

    // Create simple test data
    let rows = vec![
        vec![BinaryElem32::from(1), BinaryElem32::from(2)],
        vec![BinaryElem32::from(3), BinaryElem32::from(4)],
        vec![BinaryElem32::from(5), BinaryElem32::from(6)],
        vec![BinaryElem32::from(7), BinaryElem32::from(8)],
    ];

    // Hash the rows
    let hashed_rows: Vec<Hash> = rows.iter()
        .map(|row| hash_row(row))
        .collect();

    println!("Rows: {:?}", rows);
    println!("Hashes: {:?}", hashed_rows);

    // Build tree
    let tree = build_merkle_tree(&hashed_rows);
    println!("Tree root: {:?}", tree.get_root());

    // Test prove and verify
    let queries = vec![0, 2]; // Query rows 0 and 2
    let proof = tree.prove(&queries);
    println!("Proof for queries {:?}: {} siblings", queries, proof.siblings.len());

    // Get the opened rows
    let opened_rows: Vec<_> = queries.iter().map(|&i| rows[i].clone()).collect();
    let opened_hashes: Vec<Hash> = opened_rows.iter()
        .map(|row| hash_row(row))
        .collect();

    // Verify
    let depth = 2; // 4 leaves = 2^2
    let result = ligerito_merkle::verify(
        &tree.get_root(),
        &proof,
        depth,
        &opened_hashes,
        &queries,
    );

    println!("Verification result: {}", result);

    if result {
        println!("✓ Merkle tree working correctly!");
    } else {
        println!("✗ Merkle tree verification failed");
    }
}