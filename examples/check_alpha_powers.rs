//! Check alpha power computations
//!
//! Usage: cargo run --release --example check_alpha_powers

use ligerito_binary_fields::{BinaryElem128, BinaryFieldElement};

fn main() {
    let alpha = BinaryElem128::from_value(42);
    let dot_query7 = BinaryElem128::from_value(361);

    let mut alpha_pow = BinaryElem128::one();
    for i in 0..=7 {
        println!("alpha^{} = {:?}", i, alpha_pow);
        if i == 7 {
            let contribution = dot_query7.mul(&alpha_pow);
            println!("  361 * alpha^7 = {:?}", contribution);
            println!("  Expected at basis_poly[119]: {:?}", contribution);
        }
        alpha_pow = alpha_pow.mul(&alpha);
    }

    println!();
    println!("CPU expects: 10707260393088");
    println!("GPU produces: 10707260395290");
    println!("Difference: {}", 10707260395290i64 - 10707260393088i64);
}
