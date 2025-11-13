use ligerito::{prove_sha256, hardcoded_config_12, ligero::verify_ligero};
use binary_fields::{BinaryElem32, BinaryElem128, BinaryFieldElement};
use std::marker::PhantomData;

fn main() {
    println!("=== DEBUG VERIFY_LIGERO ===");

    // Create a minimal test case
    let queries = vec![0, 1, 2];

    // Create simple test data
    let yr = vec![
        BinaryElem128::from(100),
        BinaryElem128::from(200),
        BinaryElem128::from(300),
        BinaryElem128::from(400),
    ];

    let opened_rows = vec![
        vec![BinaryElem128::from(1), BinaryElem128::from(2)],
        vec![BinaryElem128::from(3), BinaryElem128::from(4)],
        vec![BinaryElem128::from(5), BinaryElem128::from(6)],
    ];

    let challenges = vec![
        BinaryElem128::from(10),
        BinaryElem128::from(20),
    ];

    println!("yr = {:?}", yr);
    println!("opened_rows = {:?}", opened_rows);
    println!("challenges = {:?}", challenges);
    println!("queries = {:?}", queries);

    // Let's manually compute what should happen
    use ligerito::utils::evaluate_lagrange_basis;

    let gr = evaluate_lagrange_basis(&challenges);
    println!("\nLagrange basis gr = {:?}", gr);

    // Compute dot products manually
    for (i, row) in opened_rows.iter().enumerate() {
        let dot = row.iter()
            .zip(gr.iter())
            .fold(BinaryElem128::zero(), |acc, (&r, &g)| {
                acc.add(&r.mul(&g))
            });
        println!("row[{}] * gr = {:?}", i, dot);
    }

    println!("\nCalling verify_ligero...");

    // This should panic if verification fails
    // We expect it to fail since this is random data
    std::panic::catch_unwind(|| {
        verify_ligero(&queries, &opened_rows, &yr, &challenges);
    }).ok();

    println!("Done (expected to panic)");
}