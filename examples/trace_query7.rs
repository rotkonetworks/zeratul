//! Trace tensorized_dot_product for query 7 to find the bug
//!
//! Usage: cargo run --release --example trace_query7

use binary_fields::{BinaryElem128, BinaryFieldElement};

fn main() {
    println!("Tracing Query 7 Tensorized Dot Product");
    println!("========================================\n");

    let k = 4;
    let row_size = 1 << k; // 16
    let q = 7;

    // Generate query 7's row (same as debug_gpu_sumcheck)
    let row: Vec<BinaryElem128> = (0..row_size)
        .map(|i| BinaryElem128::from_value(((q * 10 + i) as u128) + 1))
        .collect();

    // Generate challenges
    let v_challenges: Vec<BinaryElem128> = (0..k)
        .map(|i| BinaryElem128::from_value((i as u128) + 1))
        .collect();

    println!("Query 7 row:");
    for (i, &val) in row.iter().enumerate() {
        println!("  row[{}] = {:?}", i, val);
    }
    println!();

    println!("Challenges (will iterate in reverse):");
    for (i, &val) in v_challenges.iter().enumerate() {
        println!("  v_challenges[{}] = {:?}", i, val);
    }
    println!();

    // CPU implementation (from sumcheck_polys.rs)
    let mut current: Vec<BinaryElem128> = row.clone();

    println!("CPU Folding:");
    for (step, &r) in v_challenges.iter().rev().enumerate() {
        println!("\nStep {}: challenge = {:?}", step, r);
        println!("  Before fold (size {}):", current.len());
        for (i, &val) in current.iter().enumerate() {
            println!("    current[{}] = {:?}", i, val);
        }

        let half = current.len() / 2;
        let one_minus_r = BinaryElem128::one().add(&r);

        for i in 0..half {
            current[i] = current[2*i].mul(&one_minus_r)
                        .add(&current[2*i+1].mul(&r));
        }
        current.truncate(half);

        println!("  After fold (size {}):", current.len());
        for (i, &val) in current.iter().enumerate() {
            println!("    current[{}] = {:?}", i, val);
        }
    }

    println!("\n\nFinal result: {:?}", current[0]);
}
