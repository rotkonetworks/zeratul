//! Integration tests for Ligerito.

use binary_fields::{BinaryElem128, BinaryElem32};
use ligerito::{prover, verifier, Config};
use rand::{thread_rng, Rng};

#[test]
fn test_prove_verify_small() {
    // Use a small polynomial for quick testing
    let config = Config::hardcoded_config_20::<BinaryElem32, BinaryElem128>();

    // Generate a random polynomial
    let mut rng = thread_rng();
    let poly: Vec<BinaryElem32> = (0..2_u32.pow(20))
        .map(|_| BinaryElem32::random_with_rng(&mut rng))
        .collect();

    // Generate a proof
    let proof = prover(&config, &poly).expect("Proof generation failed");

    // Create a verifier configuration
    let verifier_config = Config::hardcoded_config_20_verifier();

    // Verify the proof
    let verification_result = verifier(&verifier_config, &proof).expect("Verification failed");
    assert!(verification_result, "Verification should succeed");
}

#[test]
fn test_prove_verify_medium() {
    // Use a medium polynomial
    let config = Config::hardcoded_config_24::<BinaryElem32, BinaryElem128>();

    // Generate a random polynomial
    let mut rng = thread_rng();
    let poly: Vec<BinaryElem32> = (0..2_u32.pow(24))
        .map(|_| BinaryElem32::random_with_rng(&mut rng))
        .collect();

    // Generate a proof
    let proof = prover(&config, &poly).expect("Proof generation failed");

    // Create a verifier configuration
    let verifier_config = Config::hardcoded_config_24_verifier();

    // Verify the proof
    let verification_result = verifier(&verifier_config, &proof).expect("Verification failed");
    assert!(verification_result, "Verification should succeed");
}

#[test]
fn test_custom_config() {
    // Create a custom configuration
    let config = Config::ConfigBuilder::new()
        .recursive_steps(2)
        .initial_dims(2_usize.pow(18), 2_usize.pow(6))
        .dim(0, 2_usize.pow(14), 2_usize.pow(4))
        .dim(1, 2_usize.pow(10), 2_usize.pow(4))
        .initial_k(6)
        .k(0, 4)
        .k(1, 4)
        .inv_rate(4)
        .build_prover::<BinaryElem32, BinaryElem128>();

    // Generate a random polynomial
    let mut rng = thread_rng();
    let poly: Vec<BinaryElem32> = (0..2_u32.pow(24))
        .map(|_| BinaryElem32::random_with_rng(&mut rng))
        .collect();

    // Generate a proof
    let proof = prover(&config, &poly).expect("Proof generation failed");

    // Create a verifier configuration
    let verifier_config = Config::ConfigBuilder::new()
        .recursive_steps(2)
        .initial_dims(2_usize.pow(18), 2_usize.pow(6))
        .dim(0, 2_usize.pow(14), 2_usize.pow(4))
        .dim(1, 2_usize.pow(10), 2_usize.pow(4))
        .initial_k(6)
        .k(0, 4)
        .k(1, 4)
        .inv_rate(4)
        .build_verifier();

    // Verify the proof
    let verification_result = verifier(&verifier_config, &proof).expect("Verification failed");
    assert!(verification_result, "Verification should succeed");
}

#[test]
fn test_invalid_proof() {
    // Use a small polynomial for quick testing
    let config = Config::hardcoded_config_20::<BinaryElem32, BinaryElem128>();

    // Generate a random polynomial
    let mut rng = thread_rng();
    let poly: Vec<BinaryElem32> = (0..2_u32.pow(20))
        .map(|_| BinaryElem32::random_with_rng(&mut rng))
        .collect();

    // Generate a proof
    let mut proof = prover(&config, &poly).expect("Proof generation failed");

    // Tamper with the proof by changing the final Ligero proof
    if let Some(elem) = proof.final_ligero_proof.yr.get_mut(0) {
        *elem = BinaryElem128::random_with_rng(&mut rng);
    }

    // Create a verifier configuration
    let verifier_config = Config::hardcoded_config_20_verifier();

    // Verify the proof - should fail
    let verification_result = verifier(&verifier_config, &proof);

    match verification_result {
        Ok(true) => panic!("Verification should have failed for tampered proof"),
        Ok(false) => {} // Expected result
        Err(_) => {}    // Also acceptable
    }
}

#[test]
fn test_proof_size() {
    // Test different polynomial sizes and verify the proof size increases reasonably
    let mut sizes = Vec::new();

    // 2^20 polynomial
    let config_20 = Config::hardcoded_config_20::<BinaryElem32, BinaryElem128>();
    let mut rng = thread_rng();
    let poly_20: Vec<BinaryElem32> = (0..2_u32.pow(20))
        .map(|_| BinaryElem32::random_with_rng(&mut rng))
        .collect();
    let proof_20 = prover(&config_20, &poly_20).expect("Proof generation failed");
    sizes.push(proof_20.size_in_bytes());

    // 2^24 polynomial
    let config_24 = Config::hardcoded_config_24::<BinaryElem32, BinaryElem128>();
    let poly_24: Vec<BinaryElem32> = (0..2_u32.pow(24))
        .map(|_| BinaryElem32::random_with_rng(&mut rng))
        .collect();
    let proof_24 = prover(&config_24, &poly_24).expect("Proof generation failed");
    sizes.push(proof_24.size_in_bytes());

    // Verify that proof size grows sublinearly with polynomial size
    // 2^24 / 2^20 = 16, but proof size should grow much less than 16x
    let size_ratio = sizes[1] as f64 / sizes[0] as f64;
    assert!(size_ratio < 8.0, "Proof size growth should be sublinear");
}
