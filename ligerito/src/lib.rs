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
    use rand::Rng;

    #[test]
    fn test_basic_prove_verify_merlin() {
        let config = hardcoded_config_20(
            std::marker::PhantomData::<BinaryElem32>,
            std::marker::PhantomData::<BinaryElem128>,
        );

        // Generate random polynomial instead of sequential values
        let mut rng = rand::thread_rng();
        let poly: Vec<BinaryElem32> = (0..1 << 20)
            .map(|_| BinaryElem32::from(rng.gen::<u32>()))
            .collect();

        let proof = prover(&config, &poly).unwrap();

        let verifier_config = hardcoded_config_20_verifier();
        let result = verifier(&verifier_config, &proof).unwrap();

        assert!(result);
    }
    
    #[test]
    fn test_basic_prove_verify_sha256() {
        let config = hardcoded_config_20(
            std::marker::PhantomData::<BinaryElem32>,
            std::marker::PhantomData::<BinaryElem128>,
        );

        // Generate random polynomial instead of sequential values
        let mut rng = rand::thread_rng();
        let poly: Vec<BinaryElem32> = (0..1 << 20)
            .map(|_| BinaryElem32::from(rng.gen::<u32>()))
            .collect();

        let proof = prove_sha256(&config, &poly).unwrap();

        let verifier_config = hardcoded_config_20_verifier();
        let result = verify_sha256(&verifier_config, &proof).unwrap();

        assert!(result);
    }
}
