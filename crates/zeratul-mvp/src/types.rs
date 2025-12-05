//! Core types for Zeratul - Verification Layer
//!
//! Zeratul is workload-agnostic. It doesn't know or care what computation
//! happens - it just verifies proofs and accumulates results from browser
//! clients running arbitrary workloads.

use serde::{Deserialize, Serialize};
use serde_big_array::BigArray;
use sha2::{Sha256, Digest};

/// 32-byte hash
pub type Hash = [u8; 32];

/// 64-byte signature
pub type Signature = [u8; 64];

/// 32-byte public key
pub type PublicKey = [u8; 32];

/// Service identifier (which workload type)
pub type ServiceId = u32;

/// Block height
pub type Height = u64;

/// Timestamp in milliseconds since epoch
pub type Timestamp = u64;

/// Validator index
pub type ValidatorId = u16;

/// Gas units
pub type Gas = u64;

// ============================================================================
// Block Types
// ============================================================================

/// Block header - minimal for consensus
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Header {
    /// Hash of parent block
    pub parent: Hash,
    /// Block height (0 = genesis)
    pub height: Height,
    /// Unix timestamp in milliseconds
    pub timestamp: Timestamp,
    /// Merkle root of accumulated state
    pub state_root: Hash,
    /// Merkle root of work results
    pub results_root: Hash,
    /// Hash of the block execution proof
    pub proof_hash: Hash,
    /// Block author (validator index)
    pub author: ValidatorId,
}

impl Header {
    /// Compute header hash
    pub fn hash(&self) -> Hash {
        let mut hasher = Sha256::new();
        hasher.update(&self.parent);
        hasher.update(&self.height.to_le_bytes());
        hasher.update(&self.timestamp.to_le_bytes());
        hasher.update(&self.state_root);
        hasher.update(&self.results_root);
        hasher.update(&self.proof_hash);
        hasher.update(&self.author.to_le_bytes());
        hasher.finalize().into()
    }
}

/// Complete block with header, work results, and proof
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Block {
    pub header: Header,
    /// Work results being accumulated this block
    pub work_results: Vec<WorkResult>,
    /// Serialized Ligerito proof of valid accumulation
    pub proof: Vec<u8>,
    /// Author's signature over header hash
    #[serde(with = "BigArray")]
    pub signature: Signature,
}

impl Block {
    pub fn hash(&self) -> Hash {
        self.header.hash()
    }

    pub fn is_genesis(&self) -> bool {
        self.header.height == 0
    }
}

// ============================================================================
// Work Package Types - Workload Agnostic
// ============================================================================

/// Work package submitted by a browser client
///
/// Zeratul doesn't interpret the payload - it just verifies the proof
/// and accumulates the result.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct WorkPackage {
    /// Which service/workload type this is for
    pub service: ServiceId,
    /// Opaque input data - Zeratul doesn't care what this is
    pub payload: Vec<u8>,
    /// Gas limit for verification
    pub gas_limit: Gas,
    /// Proof that the work was done correctly
    pub proof: Vec<u8>,
    /// Hash of the computation output
    pub output_hash: Hash,
    /// The actual output (optional - can be in DA)
    pub output: Option<Vec<u8>>,
    /// Submitter's signature
    #[serde(with = "BigArray")]
    pub signature: Signature,
    /// Submitter's public key
    pub submitter: PublicKey,
}

impl WorkPackage {
    /// Compute work package hash
    pub fn hash(&self) -> Hash {
        let mut hasher = Sha256::new();
        hasher.update(&self.service.to_le_bytes());
        hasher.update(&self.payload);
        hasher.update(&self.output_hash);
        hasher.finalize().into()
    }
}

/// Result of processing a work package
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct WorkResult {
    /// Hash of the work package
    pub package_hash: Hash,
    /// Service that processed it
    pub service: ServiceId,
    /// Output hash (commitment to result)
    pub output_hash: Hash,
    /// Gas used for verification
    pub gas_used: Gas,
    /// Whether verification succeeded
    pub success: bool,
}

impl WorkResult {
    pub fn hash(&self) -> Hash {
        let mut hasher = Sha256::new();
        hasher.update(&self.package_hash);
        hasher.update(&self.service.to_le_bytes());
        hasher.update(&self.output_hash);
        hasher.update(&self.gas_used.to_le_bytes());
        hasher.update(&[self.success as u8]);
        hasher.finalize().into()
    }
}

// ============================================================================
// Service Registry - Minimal
// ============================================================================

/// Service definition - just enough to verify work
///
/// Services run in browsers, not on Zeratul. This just records
/// what verification logic to use.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Service {
    /// Service identifier
    pub id: ServiceId,
    /// Hash of the service's verification code
    pub verifier_hash: Hash,
    /// Accumulated state root for this service
    pub state_root: Hash,
    /// Whether service is active
    pub active: bool,
}

// ============================================================================
// Consensus Types
// ============================================================================

/// Vote for a block
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Vote {
    /// Block hash being voted for
    pub block_hash: Hash,
    /// Validator index
    pub validator: ValidatorId,
    /// Signature over block_hash
    #[serde(with = "BigArray")]
    pub signature: Signature,
}

/// Finality certificate - proves block is finalized
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct FinalityCertificate {
    /// Block hash
    pub block_hash: Hash,
    /// Block height
    pub height: Height,
    /// Votes from validators
    pub votes: Vec<Vote>,
}

impl FinalityCertificate {
    pub fn is_valid(&self, validator_count: usize) -> bool {
        self.votes.len() >= crate::finality_threshold(validator_count)
    }
}

// ============================================================================
// Accumulation Types
// ============================================================================

/// Accumulation trace for proving
#[derive(Clone, Debug)]
pub struct AccumulationTrace {
    /// Pre-state root
    pub pre_state_root: Hash,
    /// Post-state root
    pub post_state_root: Hash,
    /// Work results processed
    pub results: Vec<WorkResult>,
    /// Total gas used
    pub gas_used: Gas,
}

/// Validator info
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Validator {
    /// Validator public key
    pub pubkey: PublicKey,
    /// Stake amount (for leader selection weight)
    pub stake: u64,
    /// Is currently active
    pub active: bool,
}

// ============================================================================
// Helper functions
// ============================================================================

/// Compute merkle root of work results
pub fn compute_results_root(results: &[WorkResult]) -> Hash {
    if results.is_empty() {
        return ZERO_HASH;
    }

    let mut leaves: Vec<Hash> = results.iter().map(|r| r.hash()).collect();

    // Pad to power of 2
    while leaves.len().count_ones() != 1 {
        leaves.push(ZERO_HASH);
    }

    // Build merkle tree
    while leaves.len() > 1 {
        let mut next_level = Vec::with_capacity(leaves.len() / 2);
        for chunk in leaves.chunks(2) {
            let mut hasher = Sha256::new();
            hasher.update(&chunk[0]);
            hasher.update(&chunk[1]);
            next_level.push(hasher.finalize().into());
        }
        leaves = next_level;
    }

    leaves[0]
}

/// Zero hash constant
pub const ZERO_HASH: Hash = [0u8; 32];

/// Genesis block parent hash
pub const GENESIS_PARENT: Hash = [0u8; 32];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_header_hash_deterministic() {
        let header = Header {
            parent: [1u8; 32],
            height: 100,
            timestamp: 1234567890,
            state_root: [2u8; 32],
            results_root: [3u8; 32],
            proof_hash: [4u8; 32],
            author: 5,
        };

        let h1 = header.hash();
        let h2 = header.hash();
        assert_eq!(h1, h2);
    }

    #[test]
    fn test_results_root_empty() {
        let root = compute_results_root(&[]);
        assert_eq!(root, ZERO_HASH);
    }

    #[test]
    fn test_work_package_hash() {
        let wp = WorkPackage {
            service: 1,
            payload: vec![1, 2, 3],
            gas_limit: 1000,
            proof: vec![],
            output_hash: [0xAB; 32],
            output: None,
            signature: [0; 64],
            submitter: [0; 32],
        };

        let h1 = wp.hash();
        let h2 = wp.hash();
        assert_eq!(h1, h2);
    }

    #[test]
    fn test_finality_threshold() {
        assert_eq!(crate::finality_threshold(3), 3);
        assert_eq!(crate::finality_threshold(4), 3);
        assert_eq!(crate::finality_threshold(6), 5);
        assert_eq!(crate::finality_threshold(100), 67);
    }
}
