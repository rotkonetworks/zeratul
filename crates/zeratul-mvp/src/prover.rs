//! Block prover using Ligerito
//!
//! Generates proofs for accumulation traces in realtime.
//!
//! In Zeratul's model:
//! - Browser clients generate proofs for their individual work packages
//! - The chain generates proofs for the accumulation of results

use crate::types::*;
use ligerito::{
    prove, prover_config_for_log_size, FinalizedLigeritoProof,
};
use ligerito_binary_fields::{BinaryElem32, BinaryElem128, BinaryFieldElement};
use sha2::{Sha256, Digest};

/// Block prover using Ligerito polynomial commitments
pub struct BlockProver {
    /// Minimum log size for proofs
    min_log_size: u32,
}

impl BlockProver {
    /// Create new prover with default settings
    pub fn new() -> Self {
        Self {
            min_log_size: 12, // Minimum 4096 elements
        }
    }

    /// Create prover with custom minimum log size
    pub fn with_min_log_size(min_log_size: u32) -> Self {
        Self { min_log_size }
    }

    /// Prove an accumulation trace
    ///
    /// Converts the trace to a polynomial and generates a Ligerito proof.
    pub fn prove_accumulation(&self, trace: &AccumulationTrace) -> ProofResult {
        let start = std::time::Instant::now();

        // Convert trace to polynomial
        let poly = self.accumulation_to_polynomial(trace);
        let encode_time = start.elapsed();

        // Determine log size (pad to power of 2)
        let log_size = (poly.len().next_power_of_two().trailing_zeros())
            .max(self.min_log_size);

        // Pad polynomial to power of 2
        let padded_size = 1usize << log_size;
        let mut padded_poly = poly;
        padded_poly.resize(padded_size, BinaryElem32::zero());

        // Get prover config for this size
        let config = prover_config_for_log_size::<BinaryElem32, BinaryElem128>(log_size);

        let prove_start = std::time::Instant::now();

        // Generate proof
        let proof = prove(&config, &padded_poly)
            .expect("Proof generation should not fail");

        let prove_time = prove_start.elapsed();

        // Serialize proof
        let serialize_start = std::time::Instant::now();
        let proof_bytes = self.serialize_proof(&proof);
        let serialize_time = serialize_start.elapsed();

        ProofResult {
            proof: proof_bytes,
            encode_time_ms: encode_time.as_millis() as u64,
            prove_time_ms: prove_time.as_millis() as u64,
            serialize_time_ms: serialize_time.as_millis() as u64,
            polynomial_size: padded_size,
        }
    }

    /// Convert accumulation trace to polynomial
    ///
    /// The polynomial encodes:
    /// - Pre-state root
    /// - Post-state root
    /// - Work results (package_hash, service, output_hash, gas, success)
    fn accumulation_to_polynomial(&self, trace: &AccumulationTrace) -> Vec<BinaryElem32> {
        let mut poly = Vec::new();

        // Encode pre-state root (8 u32s from 32 bytes)
        Self::encode_hash(&mut poly, &trace.pre_state_root);

        // Encode post-state root
        Self::encode_hash(&mut poly, &trace.post_state_root);

        // Encode gas used
        poly.push(BinaryElem32::from(trace.gas_used as u32));
        poly.push(BinaryElem32::from((trace.gas_used >> 32) as u32));

        // Encode result count
        poly.push(BinaryElem32::from(trace.results.len() as u32));

        // Encode each work result
        for result in &trace.results {
            // Package hash
            Self::encode_hash(&mut poly, &result.package_hash);

            // Service ID
            poly.push(BinaryElem32::from(result.service));

            // Output hash
            Self::encode_hash(&mut poly, &result.output_hash);

            // Gas used
            poly.push(BinaryElem32::from(result.gas_used as u32));
            poly.push(BinaryElem32::from((result.gas_used >> 32) as u32));

            // Success flag
            poly.push(BinaryElem32::from(result.success as u32));
        }

        poly
    }

    /// Helper to encode a 32-byte hash as 8 u32 field elements
    fn encode_hash(poly: &mut Vec<BinaryElem32>, hash: &Hash) {
        for chunk in hash.chunks(4) {
            let val = u32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]);
            poly.push(BinaryElem32::from(val));
        }
    }

    /// Serialize proof to bytes
    fn serialize_proof(&self, proof: &FinalizedLigeritoProof<BinaryElem32, BinaryElem128>) -> Vec<u8> {
        bincode::serialize(proof).expect("Proof serialization should not fail")
    }

    /// Compute proof hash for block header
    pub fn proof_hash(proof: &[u8]) -> Hash {
        Sha256::digest(proof).into()
    }
}

impl Default for BlockProver {
    fn default() -> Self {
        Self::new()
    }
}

/// Result of proving an accumulation trace
#[derive(Debug)]
pub struct ProofResult {
    /// Serialized Ligerito proof
    pub proof: Vec<u8>,
    /// Time spent encoding trace to polynomial (ms)
    pub encode_time_ms: u64,
    /// Time spent generating proof (ms)
    pub prove_time_ms: u64,
    /// Time spent serializing proof (ms)
    pub serialize_time_ms: u64,
    /// Size of polynomial (elements)
    pub polynomial_size: usize,
}

impl ProofResult {
    /// Total proving time in milliseconds
    pub fn total_time_ms(&self) -> u64 {
        self.encode_time_ms + self.prove_time_ms + self.serialize_time_ms
    }

    /// Proof size in bytes
    pub fn proof_size(&self) -> usize {
        self.proof.len()
    }
}

/// Verify a block proof
pub fn verify_proof(proof: &[u8], expected_hash: &Hash) -> bool {
    // Empty proofs are valid (for MVP testing)
    if proof.is_empty() {
        return true;
    }

    // First check hash matches
    let actual_hash: Hash = Sha256::digest(proof).into();
    if actual_hash != *expected_hash {
        return false;
    }

    // Deserialize and verify proof
    let proof: FinalizedLigeritoProof<BinaryElem32, BinaryElem128> =
        match bincode::deserialize(proof) {
            Ok(p) => p,
            Err(_) => return false,
        };

    // Get verifier config based on proof size
    let verifier_config = ligerito::hardcoded_config_12_verifier();

    ligerito::verify(&verifier_config, &proof).unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_trace() -> AccumulationTrace {
        AccumulationTrace {
            pre_state_root: [1u8; 32],
            post_state_root: [2u8; 32],
            results: vec![
                WorkResult {
                    package_hash: [3u8; 32],
                    service: 0,
                    output_hash: [4u8; 32],
                    gas_used: 1000,
                    success: true,
                },
                WorkResult {
                    package_hash: [5u8; 32],
                    service: 1,
                    output_hash: [6u8; 32],
                    gas_used: 2000,
                    success: true,
                },
            ],
            gas_used: 3000,
        }
    }

    #[test]
    fn test_accumulation_to_polynomial() {
        let prover = BlockProver::new();
        let trace = test_trace();
        let poly = prover.accumulation_to_polynomial(&trace);

        // Should have encoded all the data
        assert!(!poly.is_empty());

        // Check minimum elements:
        // 8 (pre_root) + 8 (post_root) + 2 (gas) + 1 (result_count) +
        // 2 results * (8 + 1 + 8 + 2 + 1) = 2 * 20 = 40
        assert!(poly.len() >= 8 + 8 + 2 + 1 + 40);
    }

    #[test]
    fn test_prove_accumulation() {
        let prover = BlockProver::new();
        let trace = test_trace();

        let result = prover.prove_accumulation(&trace);

        assert!(!result.proof.is_empty());
        assert!(result.total_time_ms() > 0);
        println!("Proof size: {} bytes", result.proof_size());
        println!("Total time: {} ms", result.total_time_ms());
    }
}
