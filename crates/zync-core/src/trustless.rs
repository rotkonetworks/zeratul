//! Trustless state verification for secure light clients
//!
//! This module provides verification of:
//! 1. FROST threshold signatures on epoch checkpoints
//! 2. Ligerito state transition proofs
//! 3. NOMT inclusion/exclusion proofs
//!
//! ## Security Model
//!
//! The trust hierarchy is:
//! - Level 0: Trust FROST signer majority (k-of-n honest)
//! - Level 1: Everything else is cryptographically verified
//!
//! The client ONLY trusts that the FROST checkpoint was signed by
//! honest signers. All state after the checkpoint is verified via
//! ligerito proofs and NOMT proofs.

use crate::{verifier_config_for_log_size, Result, ZyncError};
use sha2::{Digest, Sha256};

/// FROST public key (aggregated Schnorr key)
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FrostPublicKey(pub [u8; 32]);

/// FROST threshold signature (Schnorr)
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FrostSignature {
    pub r: [u8; 32],
    pub s: [u8; 32],
}

impl FrostSignature {
    pub fn is_present(&self) -> bool {
        self.r != [0u8; 32] || self.s != [0u8; 32]
    }
}

/// Epoch checkpoint - trust anchor for state verification
#[derive(Debug, Clone)]
pub struct EpochCheckpoint {
    pub epoch_index: u64,
    pub height: u32,
    pub block_hash: [u8; 32],
    pub tree_root: [u8; 32],
    pub nullifier_root: [u8; 32],
    pub timestamp: u64,
    pub signature: FrostSignature,
    pub signer_set_id: [u8; 32],
}

impl EpochCheckpoint {
    /// compute message hash for verification
    pub fn message_hash(&self) -> [u8; 32] {
        let mut hasher = Sha256::new();
        hasher.update(b"ZIDECAR_CHECKPOINT_V1");
        hasher.update(self.epoch_index.to_le_bytes());
        hasher.update(self.height.to_le_bytes());
        hasher.update(&self.block_hash);
        hasher.update(&self.tree_root);
        hasher.update(&self.nullifier_root);
        hasher.update(self.timestamp.to_le_bytes());
        hasher.finalize().into()
    }

    /// verify FROST signature
    pub fn verify(&self, public_key: &FrostPublicKey) -> Result<()> {
        if !self.signature.is_present() {
            return Err(ZyncError::Verification("checkpoint not signed".into()));
        }

        // TODO: implement actual Schnorr verification
        // For now, check that signature is non-zero and signer set is valid
        if self.signer_set_id == [0u8; 32] {
            return Err(ZyncError::Verification("invalid signer set".into()));
        }

        // placeholder: verify signature matches public key
        let _ = public_key;

        Ok(())
    }
}

/// State transition proof from checkpoint to current height
#[derive(Debug, Clone)]
pub struct StateTransitionProof {
    /// serialized ligerito proof
    pub proof_bytes: Vec<u8>,
    /// checkpoint we're proving from
    pub from_height: u32,
    pub from_tree_root: [u8; 32],
    pub from_nullifier_root: [u8; 32],
    /// state we're proving to
    pub to_height: u32,
    pub to_tree_root: [u8; 32],
    pub to_nullifier_root: [u8; 32],
    /// proof size for verifier config
    pub proof_log_size: u32,
}

impl StateTransitionProof {
    /// verify the ligerito state transition proof
    pub fn verify(&self) -> Result<bool> {
        use ligerito::{verify, FinalizedLigeritoProof};
        use ligerito_binary_fields::{BinaryElem32, BinaryElem128};

        if self.proof_bytes.is_empty() {
            return Ok(false);
        }

        // deserialize proof
        let proof: FinalizedLigeritoProof<BinaryElem32, BinaryElem128> =
            bincode::deserialize(&self.proof_bytes)
                .map_err(|e| ZyncError::Verification(format!("invalid proof: {}", e)))?;

        // get verifier config for proof size
        let config = verifier_config_for_log_size(self.proof_log_size);

        // verify
        verify(&config, &proof)
            .map_err(|e| ZyncError::Verification(format!("proof verification failed: {:?}", e)))
    }
}

/// Complete trustless state proof
#[derive(Debug, Clone)]
pub struct TrustlessStateProof {
    /// FROST-signed checkpoint (trust anchor)
    pub checkpoint: EpochCheckpoint,
    /// state transition from checkpoint to current
    pub transition: StateTransitionProof,
    /// current verified state
    pub current_height: u32,
    pub current_hash: [u8; 32],
}

impl TrustlessStateProof {
    /// full verification
    pub fn verify(&self, signer_key: &FrostPublicKey) -> Result<VerifiedState> {
        // 1. verify FROST checkpoint signature
        self.checkpoint.verify(signer_key)?;

        // 2. verify transition starts from checkpoint
        if self.transition.from_height != self.checkpoint.height {
            return Err(ZyncError::Verification(
                "transition doesn't start from checkpoint".into(),
            ));
        }
        if self.transition.from_tree_root != self.checkpoint.tree_root {
            return Err(ZyncError::Verification(
                "transition tree root doesn't match checkpoint".into(),
            ));
        }
        if self.transition.from_nullifier_root != self.checkpoint.nullifier_root {
            return Err(ZyncError::Verification(
                "transition nullifier root doesn't match checkpoint".into(),
            ));
        }

        // 3. verify ligerito state transition proof
        if !self.transition.verify()? {
            return Err(ZyncError::Verification(
                "state transition proof invalid".into(),
            ));
        }

        // 4. check freshness
        if self.transition.to_height != self.current_height {
            return Err(ZyncError::Verification(
                "transition doesn't reach current height".into(),
            ));
        }

        Ok(VerifiedState {
            height: self.current_height,
            block_hash: self.current_hash,
            tree_root: self.transition.to_tree_root,
            nullifier_root: self.transition.to_nullifier_root,
            checkpoint_epoch: self.checkpoint.epoch_index,
        })
    }
}

/// Result of successful verification
#[derive(Debug, Clone)]
pub struct VerifiedState {
    pub height: u32,
    pub block_hash: [u8; 32],
    pub tree_root: [u8; 32],
    pub nullifier_root: [u8; 32],
    pub checkpoint_epoch: u64,
}

/// NOMT merkle proof for inclusion/exclusion
#[derive(Debug, Clone)]
pub struct NomtProof {
    pub key: [u8; 32],
    pub root: [u8; 32],
    pub exists: bool,
    pub path: Vec<[u8; 32]>,
    pub indices: Vec<bool>,
}

impl NomtProof {
    /// verify this NOMT proof
    pub fn verify(&self) -> Result<bool> {
        if self.path.is_empty() {
            // empty proof - only valid for empty tree
            return Ok(self.root == [0u8; 32] && !self.exists);
        }

        // compute merkle root from path
        let mut current = self.key;
        for (sibling, is_right) in self.path.iter().zip(self.indices.iter()) {
            let mut hasher = Sha256::new();
            hasher.update(b"NOMT_NODE");
            if *is_right {
                hasher.update(sibling);
                hasher.update(&current);
            } else {
                hasher.update(&current);
                hasher.update(sibling);
            }
            current = hasher.finalize().into();
        }

        Ok(current == self.root)
    }
}

/// Commitment tree inclusion proof
#[derive(Debug, Clone)]
pub struct CommitmentProof {
    pub cmx: [u8; 32],
    pub position: u64,
    pub tree_root: [u8; 32],
    pub proof: NomtProof,
}

impl CommitmentProof {
    pub fn verify(&self) -> Result<bool> {
        if !self.proof.exists {
            return Ok(false);
        }
        self.proof.verify()
    }
}

/// Nullifier status proof (spent or unspent)
#[derive(Debug, Clone)]
pub struct NullifierProof {
    pub nullifier: [u8; 32],
    pub nullifier_root: [u8; 32],
    pub is_spent: bool,
    pub proof: NomtProof,
}

impl NullifierProof {
    pub fn verify(&self) -> Result<bool> {
        if self.is_spent != self.proof.exists {
            return Ok(false);
        }
        self.proof.verify()
    }
}

/// Known signer sets for checkpoint verification
pub struct SignerRegistry {
    /// mainnet genesis signer key (hardcoded)
    mainnet_key: FrostPublicKey,
    /// testnet genesis signer key (hardcoded)
    testnet_key: FrostPublicKey,
}

impl SignerRegistry {
    pub fn new() -> Self {
        // TODO: replace with actual keys from DKG ceremony
        Self {
            mainnet_key: FrostPublicKey([0x01; 32]),
            testnet_key: FrostPublicKey([0x02; 32]),
        }
    }

    pub fn mainnet_key(&self) -> &FrostPublicKey {
        &self.mainnet_key
    }

    pub fn testnet_key(&self) -> &FrostPublicKey {
        &self.testnet_key
    }
}

impl Default for SignerRegistry {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_checkpoint_message_hash() {
        let cp = EpochCheckpoint {
            epoch_index: 1,
            height: 1024,
            block_hash: [0x11; 32],
            tree_root: [0x22; 32],
            nullifier_root: [0x33; 32],
            timestamp: 1234567890,
            signature: FrostSignature {
                r: [0; 32],
                s: [0; 32],
            },
            signer_set_id: [0; 32],
        };

        let hash1 = cp.message_hash();
        let hash2 = cp.message_hash();
        assert_eq!(hash1, hash2);

        // different checkpoint should have different hash
        let cp2 = EpochCheckpoint {
            epoch_index: 2,
            ..cp
        };
        assert_ne!(cp.message_hash(), cp2.message_hash());
    }

    #[test]
    fn test_frost_signature_present() {
        let empty = FrostSignature {
            r: [0; 32],
            s: [0; 32],
        };
        assert!(!empty.is_present());

        let present = FrostSignature {
            r: [1; 32],
            s: [0; 32],
        };
        assert!(present.is_present());
    }
}
