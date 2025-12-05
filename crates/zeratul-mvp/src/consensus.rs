//! Instant BFT consensus for 1-second finality
//!
//! Simplified single-round BFT suitable for small validator sets.

use crate::types::*;
use crate::state::State;
use crate::prover::verify_proof;
use sha2::{Sha256, Digest};
use std::collections::HashMap;

/// Instant BFT consensus engine
pub struct InstantBFT {
    /// Our validator index (if we are a validator)
    validator_id: Option<ValidatorId>,
    /// Validator keys for signing
    signing_key: Option<[u8; 32]>,
    /// Pending votes for blocks
    pending_votes: HashMap<Hash, Vec<Vote>>,
    /// Finalized certificates
    finality_certs: HashMap<Hash, FinalityCertificate>,
}

impl InstantBFT {
    /// Create new consensus engine (non-validator)
    pub fn new() -> Self {
        Self {
            validator_id: None,
            signing_key: None,
            pending_votes: HashMap::new(),
            finality_certs: HashMap::new(),
        }
    }

    /// Create consensus engine for a validator
    pub fn new_validator(validator_id: ValidatorId, signing_key: [u8; 32]) -> Self {
        Self {
            validator_id: Some(validator_id),
            signing_key: Some(signing_key),
            pending_votes: HashMap::new(),
            finality_certs: HashMap::new(),
        }
    }

    /// Check if we are the leader for this height (round-robin)
    /// Validator 0 produces height 1, validator 1 produces height 2, etc.
    pub fn is_leader(&self, height: Height, validator_count: usize) -> bool {
        match self.validator_id {
            Some(id) => Self::leader_for_height(height, validator_count) == id,
            None => false,
        }
    }

    /// Get leader for a given height
    /// Height 1 -> validator 0, height 2 -> validator 1, etc.
    pub fn leader_for_height(height: Height, validator_count: usize) -> ValidatorId {
        ((height.saturating_sub(1) as usize) % validator_count) as ValidatorId
    }

    /// Verify a block proposal
    ///
    /// In Zeratul's leaderless model, any validator can propose.
    /// We just verify the proof is valid.
    pub fn verify_block(&self, block: &Block, state: &State) -> Result<(), ConsensusError> {
        // Verify height is sequential
        if block.header.height != state.height() + 1 {
            return Err(ConsensusError::InvalidHeight {
                expected: state.height() + 1,
                got: block.header.height,
            });
        }

        // Verify proof hash matches
        let proof_hash: Hash = Sha256::digest(&block.proof).into();
        if proof_hash != block.header.proof_hash {
            return Err(ConsensusError::InvalidProofHash);
        }

        // Verify proof is valid
        if !verify_proof(&block.proof, &block.header.proof_hash) {
            return Err(ConsensusError::InvalidProof);
        }

        // Verify results root
        let results_root = compute_results_root(&block.work_results);
        if results_root != block.header.results_root {
            return Err(ConsensusError::InvalidResultsRoot);
        }

        // In leaderless model, any validator can propose
        // No need to verify author is "the leader"
        // The block is valid if the proof verifies

        Ok(())
    }

    /// Vote for a block
    pub fn vote_for_block(&self, block: &Block) -> Option<Vote> {
        let (validator_id, _signing_key) = match (self.validator_id, self.signing_key) {
            (Some(v), Some(k)) => (v, k),
            _ => return None,
        };

        let block_hash = block.hash();

        // MVP: Use dummy signature
        // Production: ED25519 sign the block hash
        let signature = self.sign(&block_hash);

        Some(Vote {
            block_hash,
            validator: validator_id,
            signature,
        })
    }

    /// Add a vote and check if we have finality
    pub fn add_vote(&mut self, vote: Vote, validator_count: usize) -> Option<FinalityCertificate> {
        let votes = self.pending_votes
            .entry(vote.block_hash)
            .or_insert_with(Vec::new);

        // Don't add duplicate votes
        if votes.iter().any(|v| v.validator == vote.validator) {
            return None;
        }

        votes.push(vote.clone());

        // Check if we have enough votes
        if votes.len() >= crate::finality_threshold(validator_count) {
            let cert = FinalityCertificate {
                block_hash: vote.block_hash,
                height: 0, // Would need to track this
                votes: votes.clone(),
            };

            self.finality_certs.insert(vote.block_hash, cert.clone());
            return Some(cert);
        }

        None
    }

    /// Check if a block is finalized
    pub fn is_finalized(&self, block_hash: &Hash) -> bool {
        self.finality_certs.contains_key(block_hash)
    }

    /// Get finality certificate for a block
    pub fn get_certificate(&self, block_hash: &Hash) -> Option<&FinalityCertificate> {
        self.finality_certs.get(block_hash)
    }

    /// Clear votes for a finalized block
    pub fn clear_votes(&mut self, block_hash: &Hash) {
        self.pending_votes.remove(block_hash);
    }

    /// Sign a message (MVP: dummy signature)
    fn sign(&self, _message: &[u8]) -> Signature {
        // MVP: Just return a dummy signature
        // Production: Use ED25519 with signing_key
        [0u8; 64]
    }

    /// Verify a signature (MVP: always true)
    pub fn verify_signature(&self, _pubkey: &PublicKey, _message: &[u8], _signature: &Signature) -> bool {
        // MVP: Skip verification
        // Production: Verify ED25519 signature
        true
    }
}

impl Default for InstantBFT {
    fn default() -> Self {
        Self::new()
    }
}

/// Consensus errors
#[derive(Debug, Clone)]
pub enum ConsensusError {
    InvalidHeight { expected: Height, got: Height },
    InvalidProofHash,
    InvalidProof,
    InvalidResultsRoot,
    InvalidSignature,
    NotEnoughVotes { have: usize, need: usize },
    BlockNotFound,
}

impl std::fmt::Display for ConsensusError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ConsensusError::InvalidHeight { expected, got } => {
                write!(f, "Invalid height: expected {}, got {}", expected, got)
            }
            ConsensusError::InvalidProofHash => write!(f, "Invalid proof hash"),
            ConsensusError::InvalidProof => write!(f, "Invalid proof"),
            ConsensusError::InvalidResultsRoot => write!(f, "Invalid results root"),
            ConsensusError::InvalidSignature => write!(f, "Invalid signature"),
            ConsensusError::NotEnoughVotes { have, need } => {
                write!(f, "Not enough votes: have {}, need {}", have, need)
            }
            ConsensusError::BlockNotFound => write!(f, "Block not found"),
        }
    }
}

impl std::error::Error for ConsensusError {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_leader_rotation() {
        let validators = 3;

        // Height 1 -> validator 0, height 2 -> validator 1, etc.
        assert_eq!(InstantBFT::leader_for_height(1, validators), 0);
        assert_eq!(InstantBFT::leader_for_height(2, validators), 1);
        assert_eq!(InstantBFT::leader_for_height(3, validators), 2);
        assert_eq!(InstantBFT::leader_for_height(4, validators), 0);
        assert_eq!(InstantBFT::leader_for_height(5, validators), 1);
    }

    #[test]
    fn test_is_leader() {
        let bft = InstantBFT::new_validator(0, [0u8; 32]);

        // Validator 0 is leader for heights 1, 4, 7...
        assert!(bft.is_leader(1, 3));   // Leader is 0
        assert!(!bft.is_leader(2, 3));  // Leader is 1
        assert!(!bft.is_leader(3, 3));  // Leader is 2
        assert!(bft.is_leader(4, 3));   // Leader is 0
        assert!(!bft.is_leader(5, 3));  // Leader is 1
    }

    #[test]
    fn test_vote_collection() {
        let mut bft = InstantBFT::new();
        let block_hash = [1u8; 32];
        let validator_count = 3;

        // Add votes one by one
        let vote1 = Vote {
            block_hash,
            validator: 0,
            signature: [0u8; 64],
        };
        let result1 = bft.add_vote(vote1, validator_count);
        assert!(result1.is_none()); // Need 3 votes for 3 validators

        let vote2 = Vote {
            block_hash,
            validator: 1,
            signature: [0u8; 64],
        };
        let result2 = bft.add_vote(vote2, validator_count);
        assert!(result2.is_none()); // Still need 1 more

        let vote3 = Vote {
            block_hash,
            validator: 2,
            signature: [0u8; 64],
        };
        let result3 = bft.add_vote(vote3, validator_count);
        assert!(result3.is_some()); // Now we have finality!

        assert!(bft.is_finalized(&block_hash));
    }

    #[test]
    fn test_duplicate_vote_ignored() {
        let mut bft = InstantBFT::new();
        let block_hash = [1u8; 32];

        let vote = Vote {
            block_hash,
            validator: 0,
            signature: [0u8; 64],
        };

        bft.add_vote(vote.clone(), 3);
        bft.add_vote(vote.clone(), 3);

        assert_eq!(bft.pending_votes.get(&block_hash).unwrap().len(), 1);
    }
}
