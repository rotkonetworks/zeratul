//! zk-shuffle: chaum-pedersen shuffle proofs for mental poker
//!
//! uses batch chaum-pedersen proofs over ristretto255 for valid remasking
//! and scalar grand product for permutation correctness
//!
//! no trusted setup, DDH-based security

#![cfg_attr(not(feature = "std"), no_std)]

#[cfg(not(feature = "std"))]
extern crate alloc;

#[cfg(not(feature = "std"))]
use alloc::{vec, vec::Vec};

pub mod audit;
pub mod poker;
pub mod proof;
pub mod remasking;
pub mod transcript;
pub mod verify;

/// minimal on-chain verifier (no arkworks deps)
/// use with `--no-default-features` for no_std on-chain use
pub mod verifier;

#[cfg(test)]
mod tests;

#[cfg(not(feature = "std"))]
use alloc::string::String;

pub use proof::{ShuffleProof, prove_shuffle};
pub use remasking::ElGamalCiphertext;
pub use transcript::ShuffleTranscript;
pub use verify::verify_shuffle;

/// shuffle proof errors
#[derive(Debug, Clone)]
pub enum ShuffleError {
    DeckSizeMismatch { expected: usize, got: usize },
    InvalidPermutation,
    ProofError(String),
    VerificationError(String),
    TranscriptError(String),
}

impl core::fmt::Display for ShuffleError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            ShuffleError::DeckSizeMismatch { expected, got } => {
                write!(f, "deck size mismatch: expected {}, got {}", expected, got)
            }
            ShuffleError::InvalidPermutation => write!(f, "invalid permutation: not a bijection"),
            ShuffleError::ProofError(s) => write!(f, "proof generation failed: {}", s),
            ShuffleError::VerificationError(s) => write!(f, "verification failed: {}", s),
            ShuffleError::TranscriptError(s) => write!(f, "transcript error: {}", s),
        }
    }
}

#[cfg(feature = "std")]
impl std::error::Error for ShuffleError {}

pub type Result<T> = core::result::Result<T, ShuffleError>;

/// configuration for shuffle proofs
#[derive(Clone, Debug)]
pub struct ShuffleConfig {
    /// number of cards
    pub deck_size: usize,
}

impl ShuffleConfig {
    /// config for standard 52-card deck
    pub fn standard_deck() -> Self {
        Self { deck_size: 52 }
    }

    /// config for custom deck size
    pub fn custom(deck_size: usize) -> Self {
        Self { deck_size }
    }
}

/// a permutation of indices 0..n
#[derive(Clone, Debug)]
pub struct Permutation {
    mapping: Vec<usize>,
}

impl Permutation {
    /// create a new permutation from a mapping
    pub fn new(mapping: Vec<usize>) -> Result<Self> {
        let n = mapping.len();
        let mut seen = vec![false; n];

        for &idx in &mapping {
            if idx >= n {
                return Err(ShuffleError::InvalidPermutation);
            }
            if seen[idx] {
                return Err(ShuffleError::InvalidPermutation);
            }
            seen[idx] = true;
        }

        Ok(Self { mapping })
    }

    /// create a random permutation using fisher-yates
    pub fn random<R: ark_std::rand::Rng>(rng: &mut R, n: usize) -> Self {
        let mut mapping: Vec<usize> = (0..n).collect();
        for i in (1..n).rev() {
            let j = rng.gen_range(0..=i);
            mapping.swap(i, j);
        }
        Self { mapping }
    }

    /// apply permutation: output[i] = input[perm[i]]
    pub fn apply<T: Clone>(&self, input: &[T]) -> Vec<T> {
        self.mapping.iter().map(|&i| input[i].clone()).collect()
    }

    /// get the mapping at index i
    pub fn get(&self, i: usize) -> usize {
        self.mapping[i]
    }

    /// length of permutation
    pub fn len(&self) -> usize {
        self.mapping.len()
    }

    /// check if empty
    pub fn is_empty(&self) -> bool {
        self.mapping.is_empty()
    }

    /// get the underlying mapping
    pub fn mapping(&self) -> &[usize] {
        &self.mapping
    }
}
