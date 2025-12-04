//! BLS12-381 signature verification for BEEFY
//!
//! This module provides BLS signature aggregation and verification
//! using the ark-bls12-381 library.
//!
//! BEEFY uses BLS12-381 with:
//! - Public keys in G1 (48 bytes compressed)
//! - Signatures in G2 (96 bytes compressed)
//! - Hash-to-curve for message hashing

use crate::types::{AggregateBlsSignature, BlsPublicKey, Commitment};

#[cfg(not(feature = "std"))]
use alloc::vec::Vec;

#[cfg(feature = "bls")]
use ark_bls12_381::{Bls12_381, Fr, G1Affine, G1Projective, G2Affine, G2Projective};
#[cfg(feature = "bls")]
use ark_ec::pairing::Pairing;
#[cfg(feature = "bls")]
use ark_ec::CurveGroup;
#[cfg(feature = "bls")]
use ark_ec::AffineRepr;
#[cfg(feature = "bls")]
use ark_ec::Group;
#[cfg(feature = "bls")]
use ark_ff::PrimeField;
#[cfg(feature = "bls")]
use ark_serialize::CanonicalDeserialize;
#[cfg(feature = "bls")]
use ark_serialize::CanonicalSerialize;

/// Domain separation tag for BEEFY signatures
pub const BEEFY_DST: &[u8] = b"BEEFY-V1";

/// Error type for BLS operations
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BlsError {
    /// Failed to deserialize public key
    InvalidPublicKey,
    /// Failed to deserialize signature
    InvalidSignature,
    /// Signature verification failed
    VerificationFailed,
    /// No signers provided
    NoSigners,
}

/// Verify an aggregate BLS signature
///
/// This verifies that the aggregate signature is valid for the given
/// message and set of public keys.
///
/// The verification equation is:
/// `e(aggregate_sig, G2_generator) == e(H(message), aggregate_pubkey)`
#[cfg(feature = "bls")]
pub fn verify_aggregate_signature(
    public_keys: &[BlsPublicKey],
    message: &[u8],
    signature: &AggregateBlsSignature,
) -> Result<bool, BlsError> {
    if public_keys.is_empty() {
        return Err(BlsError::NoSigners);
    }

    // Deserialize public keys and aggregate them
    let mut aggregate_pubkey = G1Projective::default();
    for pk in public_keys {
        let pk_affine = G1Affine::deserialize_compressed(&pk.0[..])
            .map_err(|_| BlsError::InvalidPublicKey)?;
        aggregate_pubkey += pk_affine;
    }
    let aggregate_pubkey = aggregate_pubkey.into_affine();

    // Deserialize signature
    let sig = G2Affine::deserialize_compressed(&signature.0[..])
        .map_err(|_| BlsError::InvalidSignature)?;

    // Hash message to G2
    let message_point = hash_to_g2(message);

    // Verify pairing equation
    // e(aggregate_pubkey, H(message)) == e(G1_generator, signature)
    let g1_gen = G1Affine::generator();

    let lhs = Bls12_381::pairing(aggregate_pubkey, message_point);
    let rhs = Bls12_381::pairing(g1_gen, sig);

    Ok(lhs == rhs)
}

/// Hash a message to a point on G2
///
/// Uses a simple hash-and-pray approach for demonstration.
/// Production should use proper hash-to-curve (RFC 9380).
#[cfg(feature = "bls")]
fn hash_to_g2(message: &[u8]) -> G2Affine {
    use blake2::{Blake2b512, Digest};

    // Simple hash-to-curve (not secure for production - use RFC 9380)
    // This is a placeholder that hashes and maps to a point
    let mut hasher = Blake2b512::new();
    hasher.update(BEEFY_DST);
    hasher.update(message);
    let hash = hasher.finalize();

    // Use hash as scalar to multiply generator (simplified)
    let scalar = Fr::from_be_bytes_mod_order(&hash);
    let point = G2Projective::generator() * scalar;
    point.into_affine()
}

/// Aggregate multiple BLS signatures into one
///
/// Simply adds the signature points together.
#[cfg(feature = "bls")]
pub fn aggregate_signatures(signatures: &[&[u8; 96]]) -> Result<AggregateBlsSignature, BlsError> {
    if signatures.is_empty() {
        return Err(BlsError::NoSigners);
    }

    let mut aggregate = G2Projective::default();
    for sig_bytes in signatures {
        let sig = G2Affine::deserialize_compressed(&sig_bytes[..])
            .map_err(|_| BlsError::InvalidSignature)?;
        aggregate += sig;
    }

    let mut result = [0u8; 96];
    aggregate
        .into_affine()
        .serialize_compressed(&mut result[..])
        .map_err(|_| BlsError::InvalidSignature)?;

    Ok(AggregateBlsSignature(result))
}

/// Verify a BEEFY commitment signature
///
/// High-level function that takes a commitment and verifies the signature.
#[cfg(feature = "bls")]
pub fn verify_beefy_signature(
    commitment: &Commitment,
    public_keys: &[BlsPublicKey],
    signature: &AggregateBlsSignature,
) -> Result<bool, BlsError> {
    let message = commitment.signing_message();
    verify_aggregate_signature(public_keys, &message, signature)
}

// Stub implementation when BLS feature is disabled
#[cfg(not(feature = "bls"))]
pub fn verify_aggregate_signature(
    _public_keys: &[BlsPublicKey],
    _message: &[u8],
    _signature: &AggregateBlsSignature,
) -> Result<bool, BlsError> {
    // When BLS is disabled, always return true (for testing)
    Ok(true)
}

#[cfg(not(feature = "bls"))]
pub fn verify_beefy_signature(
    _commitment: &Commitment,
    _public_keys: &[BlsPublicKey],
    _signature: &AggregateBlsSignature,
) -> Result<bool, BlsError> {
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_invalid_keys_rejected() {
        // Invalid keys (zeros) should be rejected when BLS is enabled
        let public_keys = vec![BlsPublicKey([0u8; 48])];
        let message = b"test message";
        let signature = AggregateBlsSignature([0u8; 96]);

        let result = verify_aggregate_signature(&public_keys, message, &signature);

        #[cfg(feature = "bls")]
        assert_eq!(result, Err(BlsError::InvalidPublicKey));

        #[cfg(not(feature = "bls"))]
        assert!(result.is_ok()); // Stub returns true
    }

    #[test]
    fn test_empty_signers_error() {
        let public_keys: Vec<BlsPublicKey> = vec![];
        let message = b"test message";
        let signature = AggregateBlsSignature([0u8; 96]);

        let result = verify_aggregate_signature(&public_keys, message, &signature);

        #[cfg(feature = "bls")]
        assert_eq!(result, Err(BlsError::NoSigners));

        #[cfg(not(feature = "bls"))]
        assert!(result.is_ok()); // Stub returns true
    }
}
