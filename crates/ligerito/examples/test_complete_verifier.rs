/// test the complete verifier with verify_partial check
/// this is the 100% protocol-compliant implementation

use ligerito::{prove, verify_complete, hardcoded_config_12, hardcoded_config_12_verifier};
use ligerito_binary_fields::{BinaryElem32, BinaryElem128, BinaryFieldElement};
use std::marker::PhantomData;

fn main() {
    println!("=== testing complete verifier with verify_partial ===\n");

    let config = hardcoded_config_12(
        PhantomData::<BinaryElem32>,
        PhantomData::<BinaryElem128>,
    );
    let verifier_config = hardcoded_config_12_verifier();

    // test 1: simple polynomial (all ones)
    println!("test 1: all ones polynomial");
    let poly1 = vec![BinaryElem32::one(); 1 << 12];
    match prove(&config, &poly1) {
        Ok(proof) => {
            match verify_complete(&verifier_config, &proof) {
                Ok(true) => println!("✓ verification passed"),
                Ok(false) => println!("✗ verification failed"),
                Err(e) => println!("✗ verification error: {:?}", e),
            }
        }
        Err(e) => println!("✗ proof generation failed: {:?}", e),
    }

    // test 2: zero polynomial
    println!("\ntest 2: zero polynomial");
    let poly2 = vec![BinaryElem32::zero(); 1 << 12];
    match prove(&config, &poly2) {
        Ok(proof) => {
            match verify_complete(&verifier_config, &proof) {
                Ok(true) => println!("✓ verification passed"),
                Ok(false) => println!("✗ verification failed"),
                Err(e) => println!("✗ verification error: {:?}", e),
            }
        }
        Err(e) => println!("✗ proof generation failed: {:?}", e),
    }

    // test 3: pattern polynomial
    println!("\ntest 3: pattern polynomial");
    let poly3: Vec<BinaryElem32> = (0..(1 << 12))
        .map(|i| BinaryElem32::from(i as u32))
        .collect();
    match prove(&config, &poly3) {
        Ok(proof) => {
            match verify_complete(&verifier_config, &proof) {
                Ok(true) => println!("✓ verification passed"),
                Ok(false) => println!("✗ verification failed"),
                Err(e) => println!("✗ verification error: {:?}", e),
            }
        }
        Err(e) => println!("✗ proof generation failed: {:?}", e),
    }

    // test 4: alternating pattern
    println!("\ntest 4: alternating pattern");
    let poly4: Vec<BinaryElem32> = (0..(1 << 12))
        .map(|i| if i % 2 == 0 { BinaryElem32::zero() } else { BinaryElem32::one() })
        .collect();
    match prove(&config, &poly4) {
        Ok(proof) => {
            match verify_complete(&verifier_config, &proof) {
                Ok(true) => println!("✓ verification passed"),
                Ok(false) => println!("✗ verification failed"),
                Err(e) => println!("✗ verification error: {:?}", e),
            }
        }
        Err(e) => println!("✗ proof generation failed: {:?}", e),
    }

    // test 5: sparse polynomial
    println!("\ntest 5: sparse polynomial");
    let mut poly5 = vec![BinaryElem32::zero(); 1 << 12];
    poly5[0] = BinaryElem32::one();
    poly5[100] = BinaryElem32::from(42);
    poly5[1000] = BinaryElem32::from(255);
    match prove(&config, &poly5) {
        Ok(proof) => {
            match verify_complete(&verifier_config, &proof) {
                Ok(true) => println!("✓ verification passed"),
                Ok(false) => println!("✗ verification failed"),
                Err(e) => println!("✗ verification error: {:?}", e),
            }
        }
        Err(e) => println!("✗ proof generation failed: {:?}", e),
    }

    println!("\n=== all tests completed ===");
}
