//! Test GF(2^128) multiplication to ensure GPU and CPU match
//!
//! Usage: cargo run --release --example test_gf128_mul

use ligerito_binary_fields::{BinaryElem128, BinaryFieldElement};

fn main() {
    println!("Testing GF(2^128) Multiplication");
    println!("================================\n");

    // Test case from our debug: 361 * alpha^7
    let dot = BinaryElem128::from_value(361);
    let alpha = BinaryElem128::from_value(42);

    // Compute alpha^7
    let mut alpha_pow = BinaryElem128::one();
    for _ in 0..7 {
        alpha_pow = alpha_pow.mul(&alpha);
    }

    println!("Testing: 361 * alpha^7");
    println!("  dot = {:?}", dot);
    println!("  alpha^7 = {:?}", alpha_pow);

    let result = dot.mul(&alpha_pow);
    println!("  result = {:?}", result);
    println!("  Expected (from CPU): 10707260393088");
    println!("  GPU produced: 10707260395290");

    // Check if they match
    let expected = BinaryElem128::from_value(10707260393088);
    if result == expected {
        println!("\n✓ PASS: Multiplication is correct!");
    } else {
        println!("\n✗ FAIL: Multiplication mismatch!");
        println!("  Got:      {:?}", result);
        println!("  Expected: {:?}", expected);
    }

    // Additional tests with small values
    println!("\n\nAdditional tests:");

    let test_cases = vec![
        (BinaryElem128::from_value(1), BinaryElem128::from_value(1)),
        (BinaryElem128::from_value(2), BinaryElem128::from_value(3)),
        (BinaryElem128::from_value(42), BinaryElem128::from_value(42)),
        (BinaryElem128::from_value(255), BinaryElem128::from_value(256)),
    ];

    for (a, b) in test_cases {
        let result = a.mul(&b);
        println!("  {} * {} = {:?}", a, b, result);
    }
}
