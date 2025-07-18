//! Ligerito polynomial commitment scheme implementation - FIXED VERSION
//! Based on the paper by Andrija Novakovic and Guillermo Angeris
//! https://angeris.github.io/papers/ligerito.pdf

pub mod configs;
pub mod data_structures;
pub mod transcript;
pub mod utils;
pub mod sumcheck_polys;
pub mod ligero;
pub mod prover;
pub mod verifier;

pub use configs::{
    hardcoded_config_12, hardcoded_config_12_verifier,
    hardcoded_config_16, hardcoded_config_16_verifier,
    hardcoded_config_20, hardcoded_config_20_verifier,
    hardcoded_config_24, hardcoded_config_24_verifier,
    hardcoded_config_28, hardcoded_config_28_verifier,
    hardcoded_config_30, hardcoded_config_30_verifier,
};
pub use data_structures::*;
pub use prover::{prove, prove_sha256, prove_with_transcript};
pub use verifier::{verify, verify_sha256, verify_with_transcript, verify_debug};
pub use transcript::{FiatShamir, TranscriptType};

use binary_fields::BinaryFieldElement;

/// Error types for Ligerito
#[derive(Debug, thiserror::Error)]
pub enum LigeritoError {
    #[error("Invalid configuration: {0}")]
    InvalidConfig(String),

    #[error("Proof verification failed")]
    VerificationFailed,

    #[error("Invalid proof structure")]
    InvalidProof,

    #[error("Merkle tree error: {0}")]
    MerkleError(String),

    #[error("Sumcheck consistency error: {0}")]
    SumcheckError(String),
}

pub type Result<T> = std::result::Result<T, LigeritoError>;

/// Main prover function (uses Merlin transcript by default)
pub fn prover<T, U>(
    config: &ProverConfig<T, U>,
    poly: &[T],
) -> Result<FinalizedLigeritoProof<T, U>>
where
    T: BinaryFieldElement + Send + Sync,
    U: BinaryFieldElement + Send + Sync + From<T>,
{
    prover::prove(config, poly)
}

/// Main verifier function (uses Merlin transcript by default)
pub fn verifier<T, U>(
    config: &VerifierConfig,
    proof: &FinalizedLigeritoProof<T, U>,
) -> Result<bool>
where
    T: BinaryFieldElement + Send + Sync,
    U: BinaryFieldElement + Send + Sync + From<T>,
{
    verifier::verify(config, proof)
}

#[cfg(test)]
mod tests {
    use super::*;
    use binary_fields::{BinaryElem32, BinaryElem128};

    #[test]
    fn test_basic_prove_verify_merlin() {
        let config = hardcoded_config_20(
            std::marker::PhantomData::<BinaryElem32>,
            std::marker::PhantomData::<BinaryElem128>,
        );

        // Start with a simple polynomial - all zeros except first coefficient
        let mut poly = vec![BinaryElem32::zero(); 1 << 20];
        poly[0] = BinaryElem32::one(); // Just set the constant term

        println!("Testing with constant polynomial f(x) = 1");
        let proof = prover(&config, &poly).unwrap();
        println!("Proof generated successfully");

        let verifier_config = hardcoded_config_20_verifier();
        let result = verifier(&verifier_config, &proof).unwrap();
        println!("Verification result: {}", result);

        assert!(result);
    }

    #[test]
    fn test_simple_polynomial() {
        // Even simpler test with smaller size
        let config = hardcoded_config_12(
            std::marker::PhantomData::<BinaryElem32>,
            std::marker::PhantomData::<BinaryElem128>,
        );

        // Test with all ones
        let poly: Vec<BinaryElem32> = vec![BinaryElem32::one(); 1 << 12];

        println!("Testing with all-ones polynomial");
        let proof = prover(&config, &poly).unwrap();

        let verifier_config = hardcoded_config_12_verifier();
        let result = verifier(&verifier_config, &proof).unwrap();

        assert!(result);
    }

    #[test]
    fn test_zero_polynomial() {
        // Test with zero polynomial
        let config = hardcoded_config_12(
            std::marker::PhantomData::<BinaryElem32>,
            std::marker::PhantomData::<BinaryElem128>,
        );

        let poly: Vec<BinaryElem32> = vec![BinaryElem32::zero(); 1 << 12];

        println!("Testing with zero polynomial");
        let proof = prover(&config, &poly).unwrap();

        let verifier_config = hardcoded_config_12_verifier();
        let result = verifier(&verifier_config, &proof).unwrap();

        assert!(result);
    }

    #[test]
    fn test_random_polynomial() {
        use rand::{thread_rng, Rng};
        
        let config = hardcoded_config_12(
            std::marker::PhantomData::<BinaryElem32>,
            std::marker::PhantomData::<BinaryElem128>,
        );

        let mut rng = thread_rng();
        let poly: Vec<BinaryElem32> = (0..1 << 12)
            .map(|_| BinaryElem32::from(rng.gen::<u32>()))
            .collect();

        println!("Testing with random polynomial");
        let proof = prover(&config, &poly).unwrap();

        let verifier_config = hardcoded_config_12_verifier();
        let result = verifier(&verifier_config, &proof).unwrap();

        assert!(result);
    }

    #[test]
    fn test_sha256_transcript_compatibility() {
        let config = hardcoded_config_12(
            std::marker::PhantomData::<BinaryElem32>,
            std::marker::PhantomData::<BinaryElem128>,
        );

        let poly: Vec<BinaryElem32> = vec![BinaryElem32::from(42); 1 << 12];

        // Test SHA256 transcript
        let proof = prove_sha256(&config, &poly).unwrap();
        let verifier_config = hardcoded_config_12_verifier();
        let result = verify_sha256(&verifier_config, &proof).unwrap();

        assert!(result);
    }

    #[test]
    fn test_debug_verification() {
        let config = hardcoded_config_12(
            std::marker::PhantomData::<BinaryElem32>,
            std::marker::PhantomData::<BinaryElem128>,
        );

        let poly: Vec<BinaryElem32> = vec![BinaryElem32::from(123); 1 << 12];

        let proof = prover(&config, &poly).unwrap();
        let verifier_config = hardcoded_config_12_verifier();
        
        // Test debug verification
        let result = verify_debug(&verifier_config, &proof).unwrap();
        assert!(result);
    }

    #[test]
    fn test_proof_size_reasonable() {
        let config = hardcoded_config_12(
            std::marker::PhantomData::<BinaryElem32>,
            std::marker::PhantomData::<BinaryElem128>,
        );

        let poly: Vec<BinaryElem32> = vec![BinaryElem32::one(); 1 << 12];
        let proof = prover(&config, &poly).unwrap();

        let proof_size = proof.size_of();
        println!("Proof size for 2^12 polynomial: {} bytes", proof_size);

        // Proof should be much smaller than the polynomial itself
        let poly_size = poly.len() * std::mem::size_of::<BinaryElem32>();
        assert!(proof_size < poly_size / 10, "Proof should be at least 10x smaller than polynomial");
    }

    #[test]
    fn test_consistency_across_multiple_runs() {
        let config = hardcoded_config_12(
            std::marker::PhantomData::<BinaryElem32>,
            std::marker::PhantomData::<BinaryElem128>,
        );
        let verifier_config = hardcoded_config_12_verifier();

        let poly: Vec<BinaryElem32> = vec![BinaryElem32::from(999); 1 << 12];

        // Generate multiple proofs of the same polynomial
        for i in 0..3 {
            let proof = prove_sha256(&config, &poly).unwrap();
            let result = verify_sha256(&verifier_config, &proof).unwrap();
            assert!(result, "Verification failed on run {}", i);
        }
    }

    #[test] 
    fn test_pattern_polynomials() {
        let config = hardcoded_config_12(
            std::marker::PhantomData::<BinaryElem32>,
            std::marker::PhantomData::<BinaryElem128>,
        );
        let verifier_config = hardcoded_config_12_verifier();

        // Test various patterns
        let patterns = vec![
            // Alternating pattern
            (0..1 << 12).map(|i| if i % 2 == 0 { BinaryElem32::zero() } else { BinaryElem32::one() }).collect(),
            // Powers of 2 pattern
            (0..1 << 12).map(|i| BinaryElem32::from((i & 0xFF) as u32)).collect(),
            // Sparse pattern (mostly zeros with few ones)
            {
                let mut poly = vec![BinaryElem32::zero(); 1 << 12];
                poly[0] = BinaryElem32::one();
                poly[100] = BinaryElem32::from(5);
                poly[1000] = BinaryElem32::from(255);
                poly
            },
        ];

        for (i, poly) in patterns.into_iter().enumerate() {
            let proof = prover(&config, &poly).unwrap();
            let result = verifier(&verifier_config, &proof).unwrap();
            assert!(result, "Pattern {} verification failed", i);
        }
    }
}
