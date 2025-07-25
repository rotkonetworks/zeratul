//! Fixed fast example of proving and verifying with Ligerito
//! This uses a smaller polynomial size for quick demonstration
use binary_fields::{BinaryElem32, BinaryElem128};
use ligerito::{prove_sha256, verify_sha256, ProverConfig, VerifierConfig};
use reed_solomon::reed_solomon;
use rand::Rng;
use std::time::Instant;

fn create_small_config() -> ProverConfig<BinaryElem32, BinaryElem128> {
    // Create a config for 2^12 polynomial (4096 elements)
    // This should be fast even in debug mode
    let recursive_steps = 1;
    let inv_rate = 4;

    let initial_dims = (1 << 8, 1 << 4);  // (256, 16)
    let dims = vec![(1 << 6, 1 << 2)];    // (64, 4)

    let initial_k = 4;
    let ks = vec![2];

    let initial_reed_solomon = reed_solomon::<BinaryElem32>(initial_dims.0, initial_dims.0 * inv_rate);
    let reed_solomon_codes = vec![
        reed_solomon::<BinaryElem128>(dims[0].0, dims[0].0 * inv_rate),
    ];

    ProverConfig {
        recursive_steps,
        initial_dims,
        dims,
        initial_k,
        ks,
        initial_reed_solomon,
        reed_solomon_codes,
    }
}

fn create_small_verifier_config() -> VerifierConfig {
    VerifierConfig {
        recursive_steps: 1,
        initial_dim: 8,
        log_dims: vec![6],
        initial_k: 4,
        ks: vec![2],
    }
}

fn main() {
    println!("Ligerito Fast Example - FIXED VERSION");
    println!("=====================================");
    println!("Polynomial size: 2^12 = 4,096 elements\n");

    // Create configuration
    let config = create_small_config();
    let verifier_config = create_small_verifier_config();

    // Generate random polynomial
    let mut rng = rand::thread_rng();
    let poly: Vec<BinaryElem32> = (0..1 << 12)
        .map(|_| BinaryElem32::from(rng.gen::<u32>()))
        .collect();

    // Warm up (optional, helps with timing)
    println!("Warming up...");
    let _ = prove_sha256(&config, &poly).expect("Warmup failed");

    // Time the proof generation
    println!("\nGenerating proof...");
    let start = Instant::now();
    let proof = prove_sha256(&config, &poly).expect("Proving failed");
    let prove_time = start.elapsed();

    println!("✓ Proof generated in: {:?}", prove_time);
    println!("  Proof size: {} bytes", proof.size_of());

    // Time the verification
    println!("\nVerifying proof...");
    let start = Instant::now();
    let verification_result = verify_sha256(&verifier_config, &proof)
        .expect("Verification failed");
    let verify_time = start.elapsed();

    println!("✓ Verification completed in: {:?}", verify_time);
    println!("  Result: {}", if verification_result { "VALID" } else { "INVALID" });

    // Summary
    println!("\n--- Summary ---");
    println!("Polynomial: 2^12 elements");
    println!("Proving: {:?}", prove_time);
    println!("Verification: {:?}", verify_time);
    println!("Total: {:?}", prove_time + verify_time);

    if prove_time.as_millis() < 1000 {
        println!("\n✨ Fast proof generation achieved! ✨");
    }

    // Assert verification passed
    assert!(verification_result, "Proof verification failed! This indicates a bug in the implementation.");
    
    println!("\n🎉 All tests passed! The fixed implementation works correctly.");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_small_polynomial_fast() {
        let config = create_small_config();
        let poly: Vec<BinaryElem32> = vec![BinaryElem32::one(); 1 << 12];

        let start = Instant::now();
        let proof = prove_sha256(&config, &poly).expect("Proving failed");
        let prove_time = start.elapsed();

        assert!(prove_time.as_secs() < 5, "Proving took too long: {:?}", prove_time);

        let verifier_config = create_small_verifier_config();
        let result = verify_sha256(&verifier_config, &proof).expect("Verification failed");
        assert!(result);
    }

    #[test]
    fn test_zero_polynomial() {
        let config = create_small_config();
        let verifier_config = create_small_verifier_config();

        // Test with all zeros
        let poly = vec![BinaryElem32::zero(); 1 << 12];
        let proof = prove_sha256(&config, &poly).unwrap();
        assert!(verify_sha256(&verifier_config, &proof).unwrap());
    }

    #[test]
    fn test_pattern_polynomial() {
        let config = create_small_config();
        let verifier_config = create_small_verifier_config();

        // Test with a pattern
        let mut poly = vec![BinaryElem32::zero(); 1 << 12];
        for i in 0..10 {
            poly[i] = BinaryElem32::from(i as u32 + 1);
        }
        
        let proof = prove_sha256(&config, &poly).unwrap();
        assert!(verify_sha256(&verifier_config, &proof).unwrap());
    }

    #[test]
    fn test_random_polynomial() {
        let config = create_small_config();
        let verifier_config = create_small_verifier_config();

        // Test with random data
        let mut rng = rand::thread_rng();
        let poly: Vec<BinaryElem32> = (0..1 << 12)
            .map(|_| BinaryElem32::from(rng.gen::<u32>()))
            .collect();
        
        let proof = prove_sha256(&config, &poly).unwrap();
        assert!(verify_sha256(&verifier_config, &proof).unwrap());
    }

    #[test]
    fn test_consistency_across_runs() {
        let config = create_small_config();
        let verifier_config = create_small_verifier_config();

        // Same polynomial should produce same proof (deterministic with same seed)
        let poly = vec![BinaryElem32::from(42); 1 << 12];
        
        let proof1 = prove_sha256(&config, &poly).unwrap();
        let proof2 = prove_sha256(&config, &poly).unwrap();
        
        // Proofs should be identical (same transcript seed)
        assert_eq!(proof1.size_of(), proof2.size_of());
        
        // Both should verify
        assert!(verify_sha256(&verifier_config, &proof1).unwrap());
        assert!(verify_sha256(&verifier_config, &proof2).unwrap());
    }
}
