//! Ligerito polynomial commitment scheme implementation
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
pub use verifier::{verify, verify_sha256, verify_with_transcript};
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
        let config = hardcoded_config_20(
            std::marker::PhantomData::<BinaryElem32>,
            std::marker::PhantomData::<BinaryElem128>,
        );

        // Test with all ones
        let poly: Vec<BinaryElem32> = vec![BinaryElem32::one(); 1 << 20];

        println!("Testing with all-ones polynomial");
        let proof = prover(&config, &poly).unwrap();

        let verifier_config = hardcoded_config_20_verifier();
        let result = verifier(&verifier_config, &proof).unwrap();

        assert!(result);
    }

    #[test]
    fn test_zero_polynomial() {
        // Test with zero polynomial
        let config = hardcoded_config_20(
            std::marker::PhantomData::<BinaryElem32>,
            std::marker::PhantomData::<BinaryElem128>,
        );

        let poly: Vec<BinaryElem32> = vec![BinaryElem32::zero(); 1 << 20];

        println!("Testing with zero polynomial");
        let proof = prover(&config, &poly).unwrap();

        let verifier_config = hardcoded_config_20_verifier();
        let result = verifier(&verifier_config, &proof).unwrap();

        assert!(result);
    }

    #[test]
    fn test_debug_zero_challenges() {
        use crate::transcript::{FiatShamir, Transcript};
        use crate::utils::evaluate_lagrange_basis;
        use crate::sumcheck_polys::precompute_alpha_powers;
        use binary_fields::{BinaryElem32, BinaryElem128};
        use merkle_tree::MerkleRoot;

        println!("\n=== Testing Challenge Generation ===");

        // Test 1: Basic challenge generation
        let mut fs = FiatShamir::new_merlin();

        // Absorb some data first
        let root = MerkleRoot { root: Some([1u8; 32]) };
        fs.absorb_root(&root);

        // Get challenges
        println!("\nGenerating base field challenges:");
        for i in 0..4 {
            let challenge: BinaryElem32 = fs.get_challenge();
            println!("Challenge {}: {:?}", i, challenge);
            if challenge == BinaryElem32::zero() {
                println!("WARNING: Challenge {} is zero!", i);
            }
        }

        println!("\nGenerating extension field challenges:");
        for i in 0..4 {
            let challenge: BinaryElem128 = fs.get_challenge();
            println!("Challenge {}: {:?}", i, challenge);
            if challenge == BinaryElem128::zero() {
                println!("WARNING: Challenge {} is zero!", i);
            }
        }

        // Test 2: Lagrange basis with various inputs
        println!("\n=== Testing Lagrange Basis ===");

        // All zeros - should give specific pattern
        let rs_zero = vec![BinaryElem128::zero(); 4];
        let basis_zero = evaluate_lagrange_basis(&rs_zero);
        println!("Lagrange([0,0,0,0]): {:?}", &basis_zero[..4]);
        assert_eq!(basis_zero[0], BinaryElem128::one(), "First element should be 1");

        // Mixed values
        let rs_mixed = vec![
            BinaryElem128::from(1u128),
            BinaryElem128::zero(),
            BinaryElem128::from(1u128),
            BinaryElem128::zero(),
        ];
        let basis_mixed = evaluate_lagrange_basis(&rs_mixed);
        println!("Lagrange([1,0,1,0]): {:?}", &basis_mixed[..4]);
        assert!(!basis_mixed.iter().all(|&x| x == BinaryElem128::zero()));

        // All ones
        let rs_ones = vec![BinaryElem128::one(); 4];
        let basis_ones = evaluate_lagrange_basis(&rs_ones);
        println!("Lagrange([1,1,1,1]): {:?}", &basis_ones[..4]);

        // Test 3: Alpha powers
        println!("\n=== Testing Alpha Powers ===");

        let alpha_zero = BinaryElem128::zero();
        let powers_zero = precompute_alpha_powers(alpha_zero, 5);
        println!("Powers of 0: {:?}", powers_zero);
        assert_eq!(powers_zero[0], BinaryElem128::one());
        assert!(powers_zero[1..].iter().all(|&x| x == BinaryElem128::zero()));

        let alpha_one = BinaryElem128::one();
        let powers_one = precompute_alpha_powers(alpha_one, 5);
        println!("Powers of 1: {:?}", powers_one);
        assert!(powers_one.iter().all(|&x| x == BinaryElem128::one()));

        // Test 4: Field element conversion
        println!("\n=== Testing Field Conversion ===");

        let base_vals = vec![
            BinaryElem32::zero(),
            BinaryElem32::one(),
            BinaryElem32::from(0x1234),
        ];

        for val in base_vals {
            let converted = BinaryElem128::from(val);
            println!("{:?} -> {:?}", val, converted);
        }

        // Test 5: Test with actual transcript flow
        println!("\n=== Testing Full Transcript Flow ===");

        let mut fs2 = FiatShamir::new_merlin();

        // Simulate the actual protocol flow
        let root1 = MerkleRoot { root: Some([42u8; 32]) };
        fs2.absorb_root(&root1);

        let challenge1: BinaryElem32 = fs2.get_challenge();
        println!("After root absorption, challenge: {:?}", challenge1);

        // Absorb more data
        fs2.absorb_elem(challenge1);

        let challenge2: BinaryElem128 = fs2.get_challenge();
        println!("After elem absorption, challenge: {:?}", challenge2);

        // Verify we're getting non-zero challenges
        assert!(challenge1 != BinaryElem32::zero(), "Challenge1 should not be zero");
        assert!(challenge2 != BinaryElem128::zero(), "Challenge2 should not be zero");
    }

#[test]
    fn test_lagrange_basis_edge_cases() {
        use binary_fields::{BinaryElem128, BinaryFieldElement};
        use crate::utils::evaluate_lagrange_basis;

        // Test case that's failing: [1,0,1,1]
        let challenges = vec![
            BinaryElem128::one(),
            BinaryElem128::zero(),
            BinaryElem128::one(),
            BinaryElem128::one(),
        ];

        println!("\nTesting Lagrange basis with challenges [1,0,1,1]:");
        let basis = evaluate_lagrange_basis(&challenges);

        println!("Basis length: {}", basis.len());
        println!("First 8 values: {:?}", &basis[..8.min(basis.len())]);

        // Count non-zero entries
        let non_zero = basis.iter().filter(|&&x| x != BinaryElem128::zero()).count();
        println!("Non-zero entries: {}/{}", non_zero, basis.len());

        // The issue is that in binary fields:
        // 1 + 0 = 1
        // 1 + 1 = 0
        // So when we have challenges [1,0,1,1], we get:
        // Layer 0: [1+1, 1] = [0, 1]
        // Layer 1: [0*(1+0), 0*0, 1*(1+0), 1*0] = [0, 0, 1, 0]
        // Layer 2: [0*(1+1), 0*1, 0*(1+1), 0*1, 1*(1+1), 1*1, 0*(1+1), 0*1]
        //        = [0, 0, 0, 0, 0, 1, 0, 0]
        // etc.

        // Let's verify this manually
        let one = BinaryElem128::one();
        let zero = BinaryElem128::zero();

        // First layer
        let layer0 = vec![one.add(&challenges[0]), challenges[0]];
        println!("\nLayer 0: {:?}", layer0);

        // This should be [0, 1] since 1+1=0
        assert_eq!(layer0[0], zero);
        assert_eq!(layer0[1], one);

        // The Lagrange basis SHOULD have some non-zero entries
        // If it's all zeros, our implementation is wrong
        assert!(non_zero > 0, "Lagrange basis should have non-zero entries!");
    }

#[test] 
    fn test_basis_polynomial_sum() {
        use binary_fields::{BinaryElem32, BinaryElem128, BinaryFieldElement};
        use crate::sumcheck_polys::induce_sumcheck_poly_debug;
        use crate::utils::eval_sk_at_vks;

        // Create a simple test case
        let n = 4; // 2^4 = 16 elements
        let sks_vks: Vec<BinaryElem32> = eval_sk_at_vks(1 << n);

        // Use challenges that give non-zero Lagrange basis
        let v_challenges = vec![
            BinaryElem128::from(0x1234),
            BinaryElem128::from(0x5678),
            BinaryElem128::from(0x9ABC),
            BinaryElem128::from(0xDEF0),
        ];

        // Simple queries and rows
        let queries = vec![0, 1, 2, 3];
        let opened_rows = vec![
            vec![BinaryElem32::one(); 16],
            vec![BinaryElem32::from(2); 16],
            vec![BinaryElem32::from(3); 16],
            vec![BinaryElem32::from(4); 16],
        ];

            let alpha = BinaryElem128::from(0x1111);

            let (basis_poly, _) = induce_sumcheck_poly_debug(
                n,
                &sks_vks,
                &opened_rows,
                &v_challenges,
                &queries,
                alpha,
            );

            // Check the sum
            let sum = basis_poly.iter().fold(BinaryElem128::zero(), |acc, &x| acc.add(&x));
            println!("Basis polynomial sum: {:?}", sum);

            // The sum should NOT be zero for a valid polynomial
            assert_ne!(sum, BinaryElem128::zero(), "Basis polynomial sum should not be zero!");
    }

}
