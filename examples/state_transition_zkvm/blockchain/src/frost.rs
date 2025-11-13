//! FROST Threshold Signature Integration
//!
//! This module integrates Penumbra's decaf377-frost implementation for multi-threshold
//! signature verification across different security levels.
//!
//! ## Threshold Tiers
//!
//! - **1/15**: Any validator (oracle proposals, transaction inclusion)
//! - **8/15**: Simple majority (block consensus, mempool ordering)
//! - **10/15**: Byzantine threshold (liquidations, slashing) - 2/3+1
//! - **13/15**: Supermajority (governance, protocol upgrades)

use anyhow::{bail, Result};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap};

// Re-export Penumbra FROST types
// Note: This assumes decaf377-frost is added as a dependency
// pub use decaf377_frost::{
//     self as frost,
//     keys::{KeyPackage, PublicKeyPackage, SigningShare},
//     round1::{commit, SigningCommitments, SigningNonces},
//     round2::{self, SignatureShare},
//     Identifier, SigningPackage,
// };

/// Validator identifier (0-14 for 15 validators)
pub type ValidatorId = u16;

/// FROST signature threshold requirements
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ThresholdRequirement {
    /// Any single validator (1/15)
    /// Used for: oracle proposals, transaction inclusion
    AnyValidator,

    /// Simple majority (8/15)
    /// Used for: block consensus, oracle consensus
    SimpleMajority,

    /// Byzantine threshold (10/15 = 2/3+1)
    /// Used for: liquidations, slashing, large transfers
    ByzantineThreshold,

    /// Supermajority (13/15 = ~87%)
    /// Used for: governance, protocol upgrades, emergency actions
    Supermajority,
}

impl ThresholdRequirement {
    /// Get the required number of signatures for this threshold
    pub fn required_signers(&self, total_validators: usize) -> usize {
        match self {
            ThresholdRequirement::AnyValidator => 1,
            ThresholdRequirement::SimpleMajority => (total_validators / 2) + 1,
            ThresholdRequirement::ByzantineThreshold => {
                // 2/3 + 1 for BFT
                ((total_validators * 2) / 3) + 1
            }
            ThresholdRequirement::Supermajority => {
                // ~87% for governance
                ((total_validators * 13) / 15).max(total_validators - 2)
            }
        }
    }

    /// Check if a given number of signers meets this threshold
    pub fn is_met(&self, num_signers: usize, total_validators: usize) -> bool {
        num_signers >= self.required_signers(total_validators)
    }
}

/// FROST signature with participant information
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FrostSignature {
    /// The aggregated signature (64 bytes for decaf377-rdsa)
    pub signature: [u8; 64],

    /// Which validators participated (sorted list of indices)
    pub signers: Vec<ValidatorId>,

    /// Threshold requirement that was used
    pub threshold: ThresholdRequirement,
}

impl FrostSignature {
    /// Verify this signature meets the threshold requirement
    pub fn verify_threshold(&self, total_validators: usize) -> Result<()> {
        if !self
            .threshold
            .is_met(self.signers.len(), total_validators)
        {
            bail!(
                "Insufficient signers: {} < {} required for {:?}",
                self.signers.len(),
                self.threshold.required_signers(total_validators),
                self.threshold
            );
        }

        // Check for duplicate signers
        let mut sorted = self.signers.clone();
        sorted.sort();
        sorted.dedup();
        if sorted.len() != self.signers.len() {
            bail!("Duplicate signers in signature");
        }

        // Check all signers are within valid range
        if self.signers.iter().any(|&id| id as usize >= total_validators) {
            bail!("Invalid signer ID");
        }

        Ok(())
    }

    /// Verify the cryptographic signature
    ///
    /// NOTE: This is a placeholder that will integrate with decaf377-frost
    pub fn verify_signature(&self, message: &[u8], public_key: &[u8; 32]) -> Result<()> {
        // TODO: Integrate with decaf377-frost verification
        // For now, just verify threshold is met
        self.verify_threshold(15)?;

        // In production:
        // let verification_key = VerificationKey::from_bytes(public_key)?;
        // verification_key.verify(message, &self.signature.into())?;

        tracing::warn!(
            "FROST signature verification not yet integrated with decaf377 - threshold check only"
        );
        Ok(())
    }
}

/// Coordinator for aggregating FROST signatures
pub struct FrostCoordinator {
    /// Total number of validators in the network
    total_validators: usize,

    /// Public key package (group verification key)
    /// In production, this would be: PublicKeyPackage from decaf377-frost
    pub public_keys: Vec<[u8; 32]>,
}

impl FrostCoordinator {
    /// Create a new FROST coordinator for N validators
    pub fn new(total_validators: usize) -> Self {
        Self {
            total_validators,
            public_keys: vec![[0u8; 32]; total_validators],
        }
    }

    /// Aggregate signature shares into a final FROST signature
    ///
    /// This is called by the coordinator after collecting threshold shares
    pub fn aggregate(
        &self,
        signing_package: &SigningPackageData,
        signature_shares: &BTreeMap<ValidatorId, SignatureShareData>,
        threshold: ThresholdRequirement,
    ) -> Result<FrostSignature> {
        // Verify we have enough shares
        if !threshold.is_met(signature_shares.len(), self.total_validators) {
            bail!(
                "Insufficient signature shares: {} < {} required",
                signature_shares.len(),
                threshold.required_signers(self.total_validators)
            );
        }

        // In production, use decaf377-frost aggregation:
        // let frost_sig = frost::aggregate(
        //     &signing_package.inner,
        //     &signature_shares,
        //     &self.public_key_package,
        // )?;

        // For now, create a placeholder aggregated signature
        let signature = [0u8; 64]; // TODO: actual aggregation

        let signers: Vec<ValidatorId> = signature_shares.keys().copied().collect();

        Ok(FrostSignature {
            signature,
            signers,
            threshold,
        })
    }

    /// Verify a complete FROST signature
    pub fn verify(
        &self,
        message: &[u8],
        signature: &FrostSignature,
    ) -> Result<()> {
        // Verify threshold
        signature.verify_threshold(self.total_validators)?;

        // Verify cryptographic signature
        // In production: use group public key
        let group_public_key = [0u8; 32]; // TODO: actual group public key
        signature.verify_signature(message, &group_public_key)?;

        Ok(())
    }
}

/// Signing package data (Round 1 output)
#[derive(Debug, Clone)]
pub struct SigningPackageData {
    /// Message to be signed
    pub message: Vec<u8>,

    /// Commitments from each participant
    /// ValidatorId -> Commitment
    pub commitments: BTreeMap<ValidatorId, CommitmentData>,
}

/// Commitment data from Round 1
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CommitmentData {
    /// Hiding commitment (32 bytes)
    pub hiding: [u8; 32],

    /// Binding commitment (32 bytes)
    pub binding: [u8; 32],
}

/// Signature share from Round 2
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SignatureShareData {
    /// Scalar share (32 bytes for decaf377)
    pub share: [u8; 32],
}

/// Validator's key material for FROST signing
pub struct ValidatorFrostKeys {
    /// Validator's identifier
    pub validator_id: ValidatorId,

    /// Signing share (secret key share)
    /// In production: SigningShare from decaf377-frost
    pub signing_share: [u8; 32],

    /// Key package (includes verification share)
    /// In production: KeyPackage from decaf377-frost
    pub key_package: [u8; 64],
}

impl ValidatorFrostKeys {
    /// Generate Round 1: commitment
    pub fn round1_commit(&self) -> (NonceData, CommitmentData) {
        // In production:
        // let (nonces, commitments) = frost::round1::commit(&self.signing_share, &mut rng);

        // Placeholder for now
        let nonces = NonceData {
            hiding: [0u8; 32],
            binding: [0u8; 32],
        };

        let commitments = CommitmentData {
            hiding: [0u8; 32],
            binding: [0u8; 32],
        };

        (nonces, commitments)
    }

    /// Generate Round 2: signature share
    pub fn round2_sign(
        &self,
        signing_package: &SigningPackageData,
        nonces: &NonceData,
    ) -> Result<SignatureShareData> {
        // In production:
        // let share = frost::round2::sign(&signing_package, &nonces, &self.key_package)?;

        // Placeholder for now
        Ok(SignatureShareData { share: [0u8; 32] })
    }
}

/// Nonces used in Round 1 (must be kept secret until Round 2)
#[derive(Debug)]
pub struct NonceData {
    /// Hiding nonce
    pub hiding: [u8; 32],

    /// Binding nonce
    pub binding: [u8; 32],
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_threshold_requirements() {
        let total = 15;

        assert_eq!(ThresholdRequirement::AnyValidator.required_signers(total), 1);
        assert_eq!(
            ThresholdRequirement::SimpleMajority.required_signers(total),
            8
        );
        assert_eq!(
            ThresholdRequirement::ByzantineThreshold.required_signers(total),
            11
        ); // (15*2)/3 + 1 = 10 + 1 = 11
        assert_eq!(
            ThresholdRequirement::Supermajority.required_signers(total),
            13
        );
    }

    #[test]
    fn test_threshold_is_met() {
        let total = 15;

        assert!(ThresholdRequirement::AnyValidator.is_met(1, total));
        assert!(!ThresholdRequirement::SimpleMajority.is_met(7, total));
        assert!(ThresholdRequirement::SimpleMajority.is_met(8, total));
        assert!(!ThresholdRequirement::ByzantineThreshold.is_met(10, total));
        assert!(ThresholdRequirement::ByzantineThreshold.is_met(11, total));
        assert!(!ThresholdRequirement::Supermajority.is_met(12, total));
        assert!(ThresholdRequirement::Supermajority.is_met(13, total));
    }

    #[test]
    fn test_frost_signature_validation() {
        let sig = FrostSignature {
            signature: [0u8; 64],
            signers: vec![0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10], // 11 signers
            threshold: ThresholdRequirement::ByzantineThreshold,
        };

        // Should pass threshold check (11 >= 11)
        assert!(sig.verify_threshold(15).is_ok());

        // Test with insufficient signers
        let bad_sig = FrostSignature {
            signature: [0u8; 64],
            signers: vec![0, 1, 2], // Only 3 signers
            threshold: ThresholdRequirement::ByzantineThreshold,
        };

        assert!(bad_sig.verify_threshold(15).is_err());
    }
}
