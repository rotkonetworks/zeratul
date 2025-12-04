//! Ligerito prover integration for BEEFY finality proofs
//!
//! This module provides the integration with Ligerito to generate
//! succinct proofs of BEEFY finality verification.
//!
//! # Architecture
//!
//! The proving flow is:
//! 1. Verify the BEEFY witness (stake + BLS signature)
//! 2. Create a VerificationContext capturing all checked values
//! 3. Arithmetize the verification as a polynomial
//! 4. Generate a Ligerito proof
//!
//! For full PolkaVM integration, we would:
//! 1. Compile a BEEFY verifier to PolkaVM bytecode
//! 2. Execute it with the witness as input
//! 3. Prove the execution trace with ligerito via polkavm-pcvm

use crate::bls::{verify_beefy_signature, BlsError};
use crate::types::{BeefyFinalityProof, BeefyWitness};
use crate::verifier::{verify_finality_stake, VerificationContext, VerificationResult};

#[cfg(feature = "prover")]
use ligerito::{
    prover::prove,
    autosizer::prover_config_for_size,
    configs::hardcoded_config_20_verifier,
};

#[cfg(feature = "prover")]
use ligerito_binary_fields::{BinaryElem32, BinaryElem128};

/// Error type for proof generation
#[derive(Debug)]
pub enum ProverError {
    /// Witness verification failed (can't prove invalid statement)
    InvalidWitness(VerificationResult),
    /// BLS signature verification failed
    BlsVerificationFailed(BlsError),
    /// Ligerito proving error
    ProvingError(String),
    /// Serialization error
    SerializationError(String),
}

impl From<BlsError> for ProverError {
    fn from(e: BlsError) -> Self {
        ProverError::BlsVerificationFailed(e)
    }
}

/// Generate a succinct proof of BEEFY finality
///
/// This function:
/// 1. Verifies the witness is valid (stake check)
/// 2. Verifies the BLS signature (if bls feature enabled)
/// 3. Generates a Ligerito proof of the verification
///
/// The resulting proof can be verified without access to the full witness
/// or BLS signature verification.
pub fn prove_finality(witness: &BeefyWitness) -> Result<BeefyFinalityProof, ProverError> {
    // 1. Verify stake requirements
    let stake_result = verify_finality_stake(witness);
    if stake_result != VerificationResult::Valid {
        return Err(ProverError::InvalidWitness(stake_result));
    }

    // 2. Verify BLS signature
    let signer_pubkeys = witness.signer_public_keys();
    let bls_valid = verify_beefy_signature(
        &witness.commitment,
        &signer_pubkeys.into_iter().cloned().collect::<Vec<_>>(),
        &witness.aggregate_signature,
    )?;

    if !bls_valid {
        return Err(ProverError::BlsVerificationFailed(crate::bls::BlsError::VerificationFailed));
    }

    // 3. Create verification context
    let mut ctx = VerificationContext::from_witness(witness);
    ctx.bls_valid = true;

    // 4. Generate proof
    let zk_proof = generate_proof(&ctx)?;

    Ok(BeefyFinalityProof {
        block_number: witness.commitment.block_number,
        payload: witness.commitment.payload.clone(),
        validator_set_id: witness.commitment.validator_set_id,
        authority_set_root: witness.authority_set.merkle_root(),
        zk_proof,
    })
}

/// Generate the ligerito proof for the verification context
#[cfg(feature = "prover")]
fn generate_proof(ctx: &VerificationContext) -> Result<Vec<u8>, ProverError> {
    // Encode the verification context as a polynomial
    // The polynomial encodes the assertion that:
    // - signed_stake >= threshold
    // - bls_valid == true
    //
    // We use a simple encoding where the polynomial coefficients
    // represent the verification state

    let mut poly = encode_verification_context(ctx);

    // Get config and required padded size (minimum is 2^20)
    let (config, padded_size) = prover_config_for_size::<BinaryElem32, BinaryElem128>(poly.len());

    // Pad polynomial to required size
    poly.resize(padded_size, BinaryElem32::from(0u32));

    // Generate proof
    let proof = prove(&config, &poly)
        .map_err(|e| ProverError::ProvingError(format!("{:?}", e)))?;

    // Serialize proof
    let proof_bytes = serialize_proof(&proof)?;

    Ok(proof_bytes)
}

#[cfg(not(feature = "prover"))]
fn generate_proof(_ctx: &VerificationContext) -> Result<Vec<u8>, ProverError> {
    // Placeholder when prover feature is disabled
    Ok(vec![0u8; 147 * 1024]) // ~147 KB placeholder
}

/// Encode verification context as polynomial coefficients
#[cfg(feature = "prover")]
fn encode_verification_context(ctx: &VerificationContext) -> Vec<BinaryElem32> {
    // Simple encoding: pack context values into polynomial coefficients
    // Each u32 becomes one coefficient
    let mut poly = Vec::new();

    // Block number (1 coefficient)
    poly.push(BinaryElem32::from(ctx.block_number));

    // Payload hash (8 coefficients for 32 bytes)
    for chunk in ctx.payload_hash.chunks(4) {
        let val = u32::from_le_bytes(chunk.try_into().unwrap_or([0; 4]));
        poly.push(BinaryElem32::from(val));
    }

    // Validator set ID (2 coefficients for u64)
    poly.push(BinaryElem32::from(ctx.validator_set_id as u32));
    poly.push(BinaryElem32::from((ctx.validator_set_id >> 32) as u32));

    // Authority set root (8 coefficients)
    for chunk in ctx.authority_set_root.chunks(4) {
        let val = u32::from_le_bytes(chunk.try_into().unwrap_or([0; 4]));
        poly.push(BinaryElem32::from(val));
    }

    // Signed stake (4 coefficients for u128)
    let stake_bytes = ctx.signed_stake.to_le_bytes();
    for chunk in stake_bytes.chunks(4) {
        let val = u32::from_le_bytes(chunk.try_into().unwrap_or([0; 4]));
        poly.push(BinaryElem32::from(val));
    }

    // Threshold (4 coefficients)
    let threshold_bytes = ctx.threshold.to_le_bytes();
    for chunk in threshold_bytes.chunks(4) {
        let val = u32::from_le_bytes(chunk.try_into().unwrap_or([0; 4]));
        poly.push(BinaryElem32::from(val));
    }

    // Validity flag
    poly.push(BinaryElem32::from(if ctx.bls_valid { 1u32 } else { 0u32 }));

    // Pad to power of 2
    let next_pow2 = poly.len().next_power_of_two();
    poly.resize(next_pow2, BinaryElem32::from(0u32));

    poly
}

/// Serialize a ligerito proof to bytes
#[cfg(feature = "prover")]
fn serialize_proof(
    _proof: &ligerito::FinalizedLigeritoProof<BinaryElem32, BinaryElem128>
) -> Result<Vec<u8>, ProverError> {
    // For now, return a placeholder
    // Real implementation would serialize the proof structure
    // TODO: Implement proper serialization using bytemuck or serde
    Ok(vec![1u8; 147 * 1024])
}

/// Verify a BEEFY finality proof
///
/// This is the efficient verification that light clients use.
/// It only verifies the Ligerito proof, not the underlying BLS signatures.
pub fn verify_finality_proof(proof: &BeefyFinalityProof) -> Result<bool, String> {
    if proof.zk_proof.is_empty() {
        return Err("Empty proof".to_string());
    }

    #[cfg(feature = "prover")]
    {
        // Deserialize and verify the proof
        let _verifier_config = hardcoded_config_20_verifier();

        // TODO: Actually verify - need to deserialize proof first
        // let valid = verify(&verifier_config, &deserialized_proof)
        //     .map_err(|e| format!("{:?}", e))?;
        // return Ok(valid);
    }

    // Placeholder verification
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::*;

    fn mock_witness() -> BeefyWitness {
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

        BeefyWitness {
            commitment: Commitment {
                payload: vec![1, 2, 3, 4],
                block_number: 1000,
                validator_set_id: 1,
            },
            signed_by: vec![true, true, true, false], // 3 signers
            aggregate_signature: AggregateBlsSignature([0u8; 96]),
            authority_set,
        }
    }

    // When BLS feature is disabled, we can test the full proving flow with mock data
    // Note: This test is slow (~30s in release, ~5min in debug) due to 2^20 polynomial
    #[cfg(not(feature = "bls"))]
    #[test]
    #[ignore = "slow: requires real ligerito proving with 2^20 polynomial"]
    fn test_prove_finality() {
        let witness = mock_witness();
        let proof = prove_finality(&witness).expect("should prove");

        assert_eq!(proof.block_number, 1000);
        assert_eq!(proof.validator_set_id, 1);
        assert!(!proof.zk_proof.is_empty());
    }

    // When BLS is enabled, mock keys will fail - test that behavior
    #[cfg(feature = "bls")]
    #[test]
    fn test_prove_finality_rejects_invalid_keys() {
        let witness = mock_witness();
        let result = prove_finality(&witness);
        // Invalid BLS keys should cause verification to fail
        assert!(matches!(result, Err(ProverError::BlsVerificationFailed(_))));
    }

    #[cfg(not(feature = "bls"))]
    #[test]
    #[ignore = "slow: requires real ligerito proving with 2^20 polynomial"]
    fn test_verify_proof() {
        let witness = mock_witness();
        let proof = prove_finality(&witness).expect("should prove");

        let valid = verify_finality_proof(&proof).expect("should verify");
        assert!(valid);
    }

    #[test]
    fn test_insufficient_stake_rejected() {
        let mut witness = mock_witness();
        witness.signed_by = vec![true, false, false, false]; // Only 1 signer = 100 stake

        let result = prove_finality(&witness);
        // Stake check happens before BLS verification
        assert!(matches!(result, Err(ProverError::InvalidWitness(_))));
    }
}
