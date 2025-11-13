//! Verifier for state transition proofs
//!
//! This module implements fast proof verification using Ligerito PCS.

use anyhow::Result;
use ligerito::{hardcoded_config_20_verifier, verifier};

use crate::prover::StateTransitionProof;

/// Verify a state transition proof
///
/// This is FAST - only checks the PCS proof, not the actual constraints.
/// The polynomial commitment ensures that the prover knew a valid witness.
pub fn verify_transfer(proof: &StateTransitionProof) -> Result<bool> {
    // Get verifier config (much smaller than prover config)
    let config = if cfg!(test) {
        use ligerito::hardcoded_config_12_verifier;
        hardcoded_config_12_verifier()
    } else {
        hardcoded_config_20_verifier()
    };

    // Verify the Ligerito proof
    let valid = verifier(&config, &proof.pcs_proof)?;

    Ok(valid)
}

/// Verify proof and extract public inputs
///
/// Returns the commitments that should be stored in NOMT
pub fn verify_and_extract_commitments(
    proof: &StateTransitionProof,
) -> Result<VerifiedTransition> {
    // Verify the proof
    if !verify_transfer(proof)? {
        anyhow::bail!("Proof verification failed");
    }

    // Extract public inputs (commitments)
    Ok(VerifiedTransition {
        sender_commitment_old: proof.sender_commitment_old,
        sender_commitment_new: proof.sender_commitment_new,
        receiver_commitment_old: proof.receiver_commitment_old,
        receiver_commitment_new: proof.receiver_commitment_new,
    })
}

/// Verified state transition data
#[derive(Debug, Clone)]
pub struct VerifiedTransition {
    pub sender_commitment_old: [u8; 32],
    pub sender_commitment_new: [u8; 32],
    pub receiver_commitment_old: [u8; 32],
    pub receiver_commitment_new: [u8; 32],
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{AccountData, TransferInstance, prover::prove_transfer};

    fn random_salt() -> [u8; 32] {
        let mut salt = [0u8; 32];
        for i in 0..32 {
            salt[i] = (i * 7 + 13) as u8;
        }
        salt
    }

    #[test]
    #[ignore] // Skip this test in regular runs due to stack usage
    fn test_verify_transfer() {
        let sender = AccountData {
            id: 1,
            balance: 1000,
            nonce: 0,
            salt: random_salt(),
        };

        let receiver = AccountData {
            id: 2,
            balance: 500,
            nonce: 0,
            salt: random_salt(),
        };

        let instance = TransferInstance::new(
            sender,
            random_salt(),
            receiver,
            random_salt(),
            100,
        ).unwrap();

        // Generate proof
        let proof = prove_transfer(&instance).unwrap();

        // Verify proof
        let valid = verify_transfer(&proof).unwrap();
        assert!(valid, "Proof should be valid");

        // Extract commitments
        let verified = verify_and_extract_commitments(&proof).unwrap();
        assert_eq!(verified.sender_commitment_old, instance.sender_commitment_old);
        assert_eq!(verified.sender_commitment_new, instance.sender_commitment_new);
    }
}
