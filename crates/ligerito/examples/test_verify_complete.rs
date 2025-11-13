/// test verify_complete with verify_partial check
/// this verifies the complete ligerito protocol with stateful sumcheck

use ligerito_binary_fields::{BinaryElem32, BinaryElem128, BinaryFieldElement};
use ligerito::{prove_sha256, verify_complete_sha256, hardcoded_config_12, hardcoded_config_12_verifier};
use std::marker::PhantomData;

fn main() {
    println!("=== test verify_complete with verify_partial ===\n");

    let config = hardcoded_config_12(
        PhantomData::<BinaryElem32>,
        PhantomData::<BinaryElem128>,
    );
    let verifier_config = hardcoded_config_12_verifier();

    println!("testing with 2^12 = 4096 elements\n");

    // test 1: all ones
    println!("test 1: all ones polynomial");
    let poly = vec![BinaryElem32::one(); 1 << 12];
    let proof = prove_sha256(&config, &poly).expect("proving failed");
    let result = verify_complete_sha256(&verifier_config, &proof).expect("verification error");
    println!("  result: {}\n", if result { "✓ passed" } else { "✗ failed" });
    assert!(result, "all ones test failed");

    // test 2: all zeros
    println!("test 2: all zeros polynomial");
    let poly = vec![BinaryElem32::zero(); 1 << 12];
    let proof = prove_sha256(&config, &poly).expect("proving failed");
    let result = verify_complete_sha256(&verifier_config, &proof).expect("verification error");
    println!("  result: {}\n", if result { "✓ passed" } else { "✗ failed" });
    assert!(result, "all zeros test failed");

    // test 3: sequential integers
    println!("test 3: sequential integers");
    let poly: Vec<BinaryElem32> = (0..1 << 12)
        .map(|i| BinaryElem32::from(i as u32))
        .collect();
    let proof = prove_sha256(&config, &poly).expect("proving failed");
    let result = verify_complete_sha256(&verifier_config, &proof).expect("verification error");
    println!("  result: {}\n", if result { "✓ passed" } else { "✗ failed" });
    assert!(result, "sequential integers test failed");

    // test 4: random-like pattern
    println!("test 4: random-like pattern");
    let poly: Vec<BinaryElem32> = (0..1 << 12)
        .map(|i| BinaryElem32::from((i * 2654435761u32) & 0xFFFFFFFF))
        .collect();
    let proof = prove_sha256(&config, &poly).expect("proving failed");
    let result = verify_complete_sha256(&verifier_config, &proof).expect("verification error");
    println!("  result: {}\n", if result { "✓ passed" } else { "✗ failed" });
    assert!(result, "random-like test failed");

    println!("\n=== all tests passed ===");
    println!("verify_complete with verify_partial check is working correctly!");
}
