//! Zeratul Accumulator - Verify proofs and accumulate work results
//!
//! This is the core of Zeratul's verification layer.
//!
//! # Model
//!
//! ```text
//! Browser clients                    Zeratul Chain
//! ┌─────────────────┐               ┌─────────────────┐
//! │ Run computation │               │                 │
//! │ Generate proof  │──WorkPackage─▶│ Verify proof    │
//! │ (Refine phase)  │               │ (if valid)      │
//! └─────────────────┘               │ Accumulate      │
//!                                   │ Update state    │
//!                                   └─────────────────┘
//! ```
//!
//! Unlike traditional blockchains:
//! - No leader required (anyone can propose)
//! - Proofs make computation verifiable
//! - 2/3+ validators agreeing = finality

use crate::types::*;
use crate::state::State;
use crate::prover::verify_proof;
use sha2::{Sha256, Digest};

/// Accumulator - verifies work proofs and creates blocks
pub struct Accumulator {
    /// Maximum work results per block
    max_results_per_block: usize,
    /// Maximum gas per block
    max_block_gas: Gas,
}

impl Accumulator {
    pub fn new() -> Self {
        Self {
            max_results_per_block: 256,
            max_block_gas: 100_000_000,
        }
    }

    /// Verify a work package's proof
    ///
    /// This is the key operation - if proof verifies, the work is valid.
    /// No re-execution needed, no leader trust needed.
    pub fn verify_work_package(&self, package: &WorkPackage, state: &State) -> VerifyResult {
        // Check service exists
        if !state.is_service_active(package.service) {
            return VerifyResult::ServiceNotFound;
        }

        // Check gas limit
        if package.gas_limit > self.max_block_gas {
            return VerifyResult::GasLimitExceeded;
        }

        // Verify the proof
        // The proof commits to: (service, payload, output_hash)
        // If proof verifies, we know computation was done correctly
        let proof_data_hash = {
            let mut hasher = Sha256::new();
            hasher.update(&package.service.to_le_bytes());
            hasher.update(&package.payload);
            hasher.update(&package.output_hash);
            let h: Hash = hasher.finalize().into();
            h
        };

        if !verify_proof(&package.proof, &proof_data_hash) {
            return VerifyResult::InvalidProof;
        }

        VerifyResult::Valid
    }

    /// Process work packages into verified results
    pub fn process_work_packages(
        &self,
        state: &State,
        packages: Vec<WorkPackage>,
    ) -> Vec<WorkResult> {
        let mut results = Vec::new();
        let mut total_gas = 0u64;

        for package in packages {
            if results.len() >= self.max_results_per_block {
                break;
            }

            if total_gas + package.gas_limit > self.max_block_gas {
                continue;
            }

            let verify_result = self.verify_work_package(&package, state);
            let success = matches!(verify_result, VerifyResult::Valid);

            // Even failed verifications can be recorded (to charge gas)
            let gas_used = if success {
                package.gas_limit / 10 // Verification is cheap
            } else {
                1000 // Minimal gas for failed verification
            };

            total_gas += gas_used;

            results.push(WorkResult {
                package_hash: package.hash(),
                service: package.service,
                output_hash: package.output_hash,
                gas_used,
                success,
            });
        }

        results
    }

    /// Build a block from verified work results
    pub fn build_block(
        &self,
        state: &State,
        results: Vec<WorkResult>,
        timestamp: Timestamp,
    ) -> AccumulationResult {
        let pre_state_root = state.state_root();

        // Compute what state root would be after accumulation
        let mut temp_state = state.clone();
        for result in &results {
            if result.success {
                let _ = temp_state.accumulate(result);
            }
        }
        let post_state_root = temp_state.compute_state_root();

        let trace = AccumulationTrace {
            pre_state_root,
            post_state_root,
            results: results.clone(),
            gas_used: results.iter().map(|r| r.gas_used).sum(),
        };

        AccumulationResult {
            results,
            trace,
            timestamp,
            post_state_root,
        }
    }
}

impl Default for Accumulator {
    fn default() -> Self {
        Self::new()
    }
}

/// Result of verification
#[derive(Debug, Clone, PartialEq)]
pub enum VerifyResult {
    Valid,
    InvalidProof,
    ServiceNotFound,
    GasLimitExceeded,
}

/// Result of accumulation
pub struct AccumulationResult {
    /// Verified work results
    pub results: Vec<WorkResult>,
    /// Trace for proving
    pub trace: AccumulationTrace,
    /// Block timestamp
    pub timestamp: Timestamp,
    /// Post-accumulation state root
    pub post_state_root: Hash,
}

impl AccumulationResult {
    /// Convert to block
    pub fn into_block(
        self,
        parent: Hash,
        height: Height,
        author: ValidatorId,
        proof: Vec<u8>,
        signature: Signature,
    ) -> Block {
        let results_root = compute_results_root(&self.results);
        let proof_hash: Hash = Sha256::digest(&proof).into();

        let header = Header {
            parent,
            height,
            timestamp: self.timestamp,
            state_root: self.post_state_root,
            results_root,
            proof_hash,
            author,
        };

        Block {
            header,
            work_results: self.results,
            proof,
            signature,
        }
    }

    /// Number of results
    pub fn result_count(&self) -> usize {
        self.results.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_verify_work_package() {
        let state = State::genesis(vec![]);
        let accumulator = Accumulator::new();

        // Package for service 0 (null service, no real verification)
        let package = WorkPackage {
            service: 0,
            payload: vec![1, 2, 3],
            gas_limit: 10000,
            proof: vec![], // Empty proof - verify_proof returns true for MVP
            output_hash: [0xAB; 32],
            output: None,
            signature: [0; 64],
            submitter: [0; 32],
        };

        let result = accumulator.verify_work_package(&package, &state);
        assert_eq!(result, VerifyResult::Valid);
    }

    #[test]
    fn test_verify_unknown_service() {
        let state = State::genesis(vec![]);
        let accumulator = Accumulator::new();

        let package = WorkPackage {
            service: 999, // Unknown service
            payload: vec![],
            gas_limit: 10000,
            proof: vec![],
            output_hash: [0; 32],
            output: None,
            signature: [0; 64],
            submitter: [0; 32],
        };

        let result = accumulator.verify_work_package(&package, &state);
        assert_eq!(result, VerifyResult::ServiceNotFound);
    }

    #[test]
    fn test_process_packages() {
        let state = State::genesis(vec![]);
        let accumulator = Accumulator::new();

        let packages = vec![
            WorkPackage {
                service: 0,
                payload: vec![1],
                gas_limit: 10000,
                proof: vec![],
                output_hash: [1u8; 32],
                output: None,
                signature: [0; 64],
                submitter: [0; 32],
            },
            WorkPackage {
                service: 0,
                payload: vec![2],
                gas_limit: 10000,
                proof: vec![],
                output_hash: [2u8; 32],
                output: None,
                signature: [0; 64],
                submitter: [0; 32],
            },
        ];

        let results = accumulator.process_work_packages(&state, packages);
        assert_eq!(results.len(), 2);
        assert!(results.iter().all(|r| r.success));
    }

    #[test]
    fn test_build_block() {
        let state = State::genesis(vec![]);
        let accumulator = Accumulator::new();

        let packages = vec![WorkPackage {
            service: 0,
            payload: vec![1, 2, 3],
            gas_limit: 10000,
            proof: vec![],
            output_hash: [0xCD; 32],
            output: None,
            signature: [0; 64],
            submitter: [0; 32],
        }];

        let results = accumulator.process_work_packages(&state, packages);
        let accum_result = accumulator.build_block(&state, results, 1000);

        assert_eq!(accum_result.result_count(), 1);
        assert_ne!(accum_result.post_state_root, ZERO_HASH);
    }
}
