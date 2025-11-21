//! Ligerito polynomial commitment scheme implementation
//!
//! Based on the paper by Andrija Novakovic and Guillermo Angeris:
//! https://angeris.github.io/papers/ligerito.pdf
//!
//! # Features
//!
//! - `std` (default): Enable standard library support
//! - `prover` (default): Include proving functionality
//! - `verifier-only`: Only include verifier (minimal dependencies)
//! - `parallel` (default): Enable parallel processing with rayon
//! - `hardware-accel` (default): Enable SIMD acceleration for binary field operations
//! - `webgpu` (optional): GPU-accelerated proving via WebGPU (experimental)
//! - `cli`: Enable CLI binary
//!
//! # Backend Selection
//!
//! The prover can use different computational backends:
//!
//! - **CPU backend** (default): Uses SIMD when `hardware-accel` is enabled
//! - **GPU backend**: Available with `webgpu` feature, automatically falls back to CPU on failure
//!
//! Control via environment variable:
//! ```bash
//! LIGERITO_BACKEND=cpu   # Force CPU
//! LIGERITO_BACKEND=gpu   # Prefer GPU, fallback to CPU
//! LIGERITO_BACKEND=auto  # Auto-detect (default)
//! ```
//!
//! # Performance Optimizations
//!
//! ## Current Implementation
//!
//! Uses Reed-Solomon codes over binary extension fields for all rounds:
//! - Round 1 (G₁): Reed-Solomon over F₂³² with 148 queries
//! - Rounds 2-ℓ (G₂...Gₗ): Reed-Solomon over F₂¹²⁸ with 148 queries each
//!
//! This provides excellent proof sizes (147 KB for 2²⁰ polynomial) and good performance
//! on platforms with SIMD support.
//!
//! ## Future WASM Optimization (Not Yet Implemented)
//!
//! For WASM targets without SIMD support, a **Repeat-Accumulate-Accumulate (RAA)** code
//! could be used for the first round (G₁) as described in the paper:
//!
//! - **G₁**: RAA code over F₂⁸ (no multiplications, just XORs and accumulates)
//! - **G₂...Gₗ**: Reed-Solomon over F₂¹²⁸ (unchanged)
//!
//! ### RAA Tradeoffs
//!
//! - ✅ **5-10x faster proving in WASM** (no field multiplications needed)
//! - ✅ **Smaller field** (F₂⁸ vs F₂³²) reduces memory bandwidth
//! - ❌ **More queries needed** (1060 vs 148 for 100-bit security due to worse distance)
//! - ❌ **Slightly larger proofs** (~165 KB vs 147 KB for 2²⁰)
//! - ⚠️ **Requires GKR construction** from [Bre+24] to avoid RAA generation bottleneck
//!
//! This optimization makes sense primarily for browser-based proving where user experience
//! (proving time) matters more than proof size. For native code with SIMD, pure Reed-Solomon
//! is faster and produces smaller proofs.
//!
//! # Examples
//!
//! ```rust,ignore
//! use ligerito::{prove, verify, hardcoded_config_20, hardcoded_config_20_verifier};
//! use ligerito_binary_fields::{BinaryElem32, BinaryElem128};
//! use std::marker::PhantomData;
//!
//! let config = hardcoded_config_20(
//!     PhantomData::<BinaryElem32>,
//!     PhantomData::<BinaryElem128>,
//! );
//!
//! let poly: Vec<BinaryElem32> = vec![BinaryElem32::from(42); 1 << 20];
//! let proof = prove(&config, &poly).unwrap();
//!
//! let verifier_config = hardcoded_config_20_verifier();
//! let valid = verify(&verifier_config, &proof).unwrap();
//! assert!(valid);
//! ```

#![cfg_attr(not(feature = "std"), no_std)]

#[cfg(not(feature = "std"))]
extern crate alloc;

#[cfg(not(feature = "std"))]
#[macro_use]
extern crate alloc as alloc_crate;

#[cfg(not(feature = "std"))]
use alloc::{vec::Vec, string::String, boxed::Box, format};

// Re-export with shorter names for internal use
extern crate ligerito_binary_fields as binary_fields;
extern crate ligerito_merkle as merkle_tree;
#[cfg(feature = "prover")]
extern crate ligerito_reed_solomon as reed_solomon;

pub mod configs;
pub mod data_structures;
pub mod transcript;
pub mod utils;
pub mod sumcheck_polys;
pub mod sumcheck_verifier;
pub mod verifier;

// Prover-only modules
#[cfg(feature = "prover")]
pub mod ligero;

#[cfg(feature = "prover")]
pub mod prover;

// Backend abstraction for CPU/GPU
#[cfg(feature = "prover")]
pub mod backend;

// WASM bindings
#[cfg(feature = "wasm")]
pub mod wasm;

// WebGPU acceleration (optional)
#[cfg(feature = "webgpu")]
pub mod gpu;

// CPU core affinity utilities (for benchmarking)
#[cfg(feature = "core_affinity")]
pub mod cpu_affinity;

// Register-only pcVM (polynomial commitment VM)
// PCVM module moved to its own crate: polkavm-pcvm
// To use PolkaVM constraints, add: polkavm-pcvm = { version = "0.1", features = ["polkavm-integration"] }

// Always export data structures
pub use data_structures::{ProverConfig, VerifierConfig, FinalizedLigeritoProof};

// Always export verifier configs
pub use configs::{
    hardcoded_config_12_verifier,
    hardcoded_config_16_verifier,
    hardcoded_config_20_verifier,
    hardcoded_config_24_verifier,
    hardcoded_config_28_verifier,
    hardcoded_config_30_verifier,
};

// Export prover configs only when prover feature is enabled
#[cfg(feature = "prover")]
pub use configs::{
    hardcoded_config_12,
    hardcoded_config_16,
    hardcoded_config_20,
    hardcoded_config_24,
    hardcoded_config_28,
    hardcoded_config_30,
};

pub use data_structures::*;

// Prover exports (only with prover feature)
#[cfg(feature = "prover")]
pub use prover::{prove, prove_sha256, prove_with_transcript};

// Verifier exports (always available)
pub use verifier::{verify, verify_sha256, verify_with_transcript, verify_debug, verify_complete, verify_complete_sha256};
pub use transcript::{FiatShamir, TranscriptType, Transcript};

use binary_fields::BinaryFieldElement;

/// Error types for Ligerito
#[cfg(feature = "std")]
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

    #[error("GPU initialization failed: {0}")]
    GpuInitFailed(String),
}

/// Error types for Ligerito (no_std version)
#[cfg(not(feature = "std"))]
#[derive(Debug, Clone)]
pub enum LigeritoError {
    InvalidConfig,
    VerificationFailed,
    InvalidProof,
    MerkleError,
    SumcheckError,
    GpuInitFailed,
}

#[cfg(not(feature = "std"))]
impl core::fmt::Display for LigeritoError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            LigeritoError::InvalidConfig => write!(f, "Invalid configuration"),
            LigeritoError::VerificationFailed => write!(f, "Proof verification failed"),
            LigeritoError::InvalidProof => write!(f, "Invalid proof structure"),
            LigeritoError::MerkleError => write!(f, "Merkle tree error"),
            LigeritoError::SumcheckError => write!(f, "Sumcheck consistency error"),
            LigeritoError::GpuInitFailed => write!(f, "GPU initialization failed"),
        }
    }
}

pub type Result<T> = core::result::Result<T, LigeritoError>;

/// Main prover function (uses Merlin transcript by default)
/// Only available with the `prover` feature
#[cfg(feature = "prover")]
pub fn prover<T, U>(
    config: &ProverConfig<T, U>,
    poly: &[T],
) -> Result<FinalizedLigeritoProof<T, U>>
where
    T: BinaryFieldElement + Send + Sync + bytemuck::Pod + 'static,
    U: BinaryFieldElement + Send + Sync + From<T> + bytemuck::Pod + 'static,
{
    prover::prove(config, poly)
}

/// Main verifier function (uses Merlin transcript by default)
/// Always available
pub fn verifier<T, U>(
    config: &VerifierConfig,
    proof: &FinalizedLigeritoProof<T, U>,
) -> Result<bool>
where
    T: BinaryFieldElement,
    U: BinaryFieldElement + From<T>,
{
    verifier::verify(config, proof)
}

#[cfg(all(test, feature = "std", feature = "prover"))]
mod tests {
    use super::*;
    use ligerito_binary_fields::{BinaryElem32, BinaryElem128};

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

        // Use a non-constant polynomial to avoid degenerate case
        let poly: Vec<BinaryElem32> = (0..(1 << 12))
            .map(|i| BinaryElem32::from(i as u32))
            .collect();

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

        // for small polynomials (2^12), proof is ~2x the polynomial size
        // for larger polynomials (2^20+), proof becomes much smaller relative to data
        let poly_size = poly.len() * std::mem::size_of::<BinaryElem32>();
        assert!(proof_size < poly_size * 3, "proof should be reasonable size (< 3x polynomial)");

        // proof should be at least somewhat compact (not trivially large)
        assert!(proof_size < 100_000, "proof for 2^12 should be under 100KB");
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
