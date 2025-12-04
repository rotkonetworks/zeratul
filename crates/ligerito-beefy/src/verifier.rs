//! BEEFY finality verification logic
//!
//! This module contains the core verification logic that will be proven
//! using Ligerito. The verification is designed to be efficiently
//! arithmetizable for the polynomial commitment scheme.

#[cfg(not(feature = "std"))]
use alloc::vec::Vec;

use crate::types::{BeefyWitness, BlsPublicKey, Commitment};

/// Result of BEEFY verification
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum VerificationResult {
    /// Verification succeeded
    Valid,
    /// Not enough stake signed (below 2/3 threshold)
    InsufficientStake { signed: u128, required: u128 },
    /// Validator set ID mismatch
    ValidatorSetMismatch { expected: u64, got: u64 },
    /// Wrong number of signatures
    SignatureCountMismatch { expected: usize, got: usize },
    /// BLS signature verification failed
    InvalidSignature,
}

/// Verify BEEFY finality witness (without BLS - for testing/integration)
///
/// This function verifies everything except the BLS signature.
/// Use `verify_finality_full` for complete verification including BLS.
pub fn verify_finality_stake(witness: &BeefyWitness) -> VerificationResult {
    // 1. Check validator set ID matches
    if witness.commitment.validator_set_id != witness.authority_set.id {
        return VerificationResult::ValidatorSetMismatch {
            expected: witness.commitment.validator_set_id,
            got: witness.authority_set.id,
        };
    }

    // 2. Check signature count matches validator count
    if witness.signed_by.len() != witness.authority_set.validators.len() {
        return VerificationResult::SignatureCountMismatch {
            expected: witness.authority_set.validators.len(),
            got: witness.signed_by.len(),
        };
    }

    // 3. Calculate signed stake
    let signed_stake = witness.signed_stake();
    let required_stake = witness.authority_set.threshold();

    // 4. Check supermajority
    if signed_stake < required_stake {
        return VerificationResult::InsufficientStake {
            signed: signed_stake,
            required: required_stake,
        };
    }

    VerificationResult::Valid
}

/// Aggregate public keys for BLS verification
///
/// Given the list of signers, aggregate their public keys for
/// signature verification. In BLS, we verify:
///
/// `e(aggregate_sig, G2_generator) == e(H(message), aggregate_pubkey)`
pub fn aggregate_public_keys(
    signed_by: &[bool],
    validators: &[crate::types::Validator],
) -> Vec<BlsPublicKey> {
    signed_by
        .iter()
        .zip(validators.iter())
        .filter_map(|(signed, validator)| {
            if *signed {
                Some(validator.bls_public_key.clone())
            } else {
                None
            }
        })
        .collect()
}

/// Compute the message that validators sign
///
/// BEEFY validators sign the SCALE-encoded commitment
pub fn compute_signing_message(commitment: &Commitment) -> Vec<u8> {
    commitment.signing_message()
}

/// Verification context for the ZK circuit
///
/// This struct holds all the values that need to be checked in the circuit.
/// It's designed to be easily arithmetizable.
#[derive(Clone, Debug)]
pub struct VerificationContext {
    /// Block number being finalized
    pub block_number: u32,
    /// Payload (MMR root)
    pub payload_hash: [u8; 32],
    /// Validator set ID
    pub validator_set_id: u64,
    /// Authority set merkle root
    pub authority_set_root: [u8; 32],
    /// Total signed stake
    pub signed_stake: u128,
    /// Required threshold
    pub threshold: u128,
    /// Number of signers
    pub signer_count: u32,
    /// BLS signature valid (set by host function in PolkaVM)
    pub bls_valid: bool,
}

impl VerificationContext {
    /// Create verification context from witness
    pub fn from_witness(witness: &BeefyWitness) -> Self {
        use blake2::{Blake2b512, Digest};

        // Hash the payload
        let mut payload_hash = [0u8; 32];
        let hash = Blake2b512::digest(&witness.commitment.payload);
        payload_hash.copy_from_slice(&hash[..32]);

        Self {
            block_number: witness.commitment.block_number,
            payload_hash,
            validator_set_id: witness.commitment.validator_set_id,
            authority_set_root: witness.authority_set.merkle_root(),
            signed_stake: witness.signed_stake(),
            threshold: witness.authority_set.threshold(),
            signer_count: witness.signed_by.iter().filter(|&&x| x).count() as u32,
            bls_valid: false, // Must be set by BLS verification
        }
    }

    /// Check if the context represents a valid finality proof
    pub fn is_valid(&self) -> bool {
        self.signed_stake >= self.threshold && self.bls_valid
    }
}

/// The arithmetized verification function for Ligerito
///
/// This is the core logic that gets proven. It's designed to be
/// efficient in the Ligerito circuit model.
///
/// Returns 1 if valid, 0 if invalid
pub fn arithmetized_verify(ctx: &VerificationContext) -> u64 {
    // All checks must pass
    let stake_check = if ctx.signed_stake >= ctx.threshold { 1u64 } else { 0u64 };
    let bls_check = if ctx.bls_valid { 1u64 } else { 0u64 };

    // AND all conditions
    stake_check * bls_check
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::*;

    fn mock_witness(num_signers: usize) -> BeefyWitness {
        let validators: Vec<_> = (0..4)
            .map(|i| Validator {
                bls_public_key: BlsPublicKey([i as u8; 48]),
                weight: 100,
            })
            .collect();

        let authority_set = AuthoritySet {
            id: 1,
            validators,
            total_stake: 400,
        };

        let mut signed_by = vec![false; 4];
        for i in 0..num_signers.min(4) {
            signed_by[i] = true;
        }

        BeefyWitness {
            commitment: Commitment {
                payload: vec![1, 2, 3, 4],
                block_number: 1000,
                validator_set_id: 1,
            },
            signed_by,
            aggregate_signature: AggregateBlsSignature([0u8; 96]),
            authority_set,
        }
    }

    #[test]
    fn test_verify_sufficient_stake() {
        let witness = mock_witness(3); // 3 signers = 300 stake
        let result = verify_finality_stake(&witness);
        assert_eq!(result, VerificationResult::Valid);
    }

    #[test]
    fn test_verify_insufficient_stake() {
        let witness = mock_witness(2); // 2 signers = 200 stake
        let result = verify_finality_stake(&witness);
        match result {
            VerificationResult::InsufficientStake { signed, required } => {
                assert_eq!(signed, 200);
                assert_eq!(required, 267);
            }
            _ => panic!("Expected InsufficientStake"),
        }
    }

    #[test]
    fn test_verification_context() {
        let witness = mock_witness(3);
        let ctx = VerificationContext::from_witness(&witness);

        assert_eq!(ctx.block_number, 1000);
        assert_eq!(ctx.validator_set_id, 1);
        assert_eq!(ctx.signed_stake, 300);
        assert_eq!(ctx.threshold, 267);
        assert_eq!(ctx.signer_count, 3);
    }

    #[test]
    fn test_arithmetized_verify() {
        let witness = mock_witness(3);
        let mut ctx = VerificationContext::from_witness(&witness);

        // Without BLS verification
        ctx.bls_valid = false;
        assert_eq!(arithmetized_verify(&ctx), 0);

        // With BLS verification
        ctx.bls_valid = true;
        assert_eq!(arithmetized_verify(&ctx), 1);
    }
}
