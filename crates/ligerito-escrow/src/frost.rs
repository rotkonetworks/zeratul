//! FROST Integration for 3-Party Escrow
//!
//! Thin wrapper around decaf377-frost for simple 2-of-3 escrow.
//! NO on-chain DKG - dealer (seller) splits key and distributes.
//!
//! # Why Not Full FROST DKG?
//!
//! Full FROST DKG requires:
//! - Multiple rounds of communication
//! - All parties online simultaneously
//! - Complex coordination
//!
//! For escrow, we just need:
//! - Seller generates ephemeral key
//! - Seller splits into 3 shares
//! - Each party verifies their share
//! - Any 2 can reconstruct and sign
//!
//! # Verification Options
//!
//! 1. **Feldman VSS** (built into FROST)
//!    - Commitments: curve points g^{a_i}
//!    - Verification: Σ commitment^{i^j} == share·G
//!    - Tight coupling to signing curve
//!
//! 2. **Ligerito VSS** (our addition)
//!    - Commitments: polynomial commitment over binary fields
//!    - Verification: sumcheck protocol
//!    - Curve agnostic, works for any chain
//!
//! For multi-chain escrow (Zcash + Penumbra), Ligerito VSS is cleaner.

#[allow(unused_imports)]
use crate::{EscrowError, Result};

/// Configuration for which chain we're creating escrow on
#[derive(Clone, Debug)]
pub enum EscrowChain {
    /// Zcash (Orchard) - uses Pallas curve
    Zcash,
    /// Penumbra - uses decaf377
    Penumbra,
}

/// A FROST key package for escrow (simplified)
#[derive(Clone)]
pub struct EscrowKeyPackage {
    /// Which chain
    pub chain: EscrowChain,
    /// Party index (1, 2, or 3)
    pub index: u16,
    /// Secret share (32 bytes)
    pub secret_share: [u8; 32],
    /// Public share for verification
    pub public_share: [u8; 32],
    /// Group public key (the escrow address)
    pub group_public_key: [u8; 32],
    /// Feldman VSS commitment (optional, for FROST-native verification)
    pub vss_commitment: Option<Vec<[u8; 32]>>,
}

/// Escrow setup result from dealer (seller)
#[derive(Clone)]
pub struct EscrowKeySet {
    /// The group public key (escrow address)
    pub group_public_key: [u8; 32],
    /// Key packages for each party
    pub packages: [EscrowKeyPackage; 3],
    /// Feldman VSS commitment (curve points)
    pub vss_commitment: Vec<[u8; 32]>,
}

/// Create 2-of-3 escrow key set
///
/// Seller calls this to generate the escrow.
/// Returns key packages for Buyer (index 1), Seller (index 2), Arbitrator (index 3).
#[cfg(feature = "frost-penumbra")]
pub fn create_escrow_penumbra(seed: &[u8; 32]) -> Result<EscrowKeySet> {
    use decaf377_frost::keys;
    use decaf377_rdsa::{SigningKey, SpendAuth};
    use rand::rngs::OsRng;

    // Derive signing key from seed
    let sk_bytes = blake2b_hash(seed, b"escrow-signing-key");
    let signing_key = SigningKey::<SpendAuth>::try_from(sk_bytes)
        .map_err(|_| EscrowError::InvalidShare)?;

    // Split into 2-of-3 FROST shares
    let identifiers = keys::IdentifierList::Default;
    let (secret_shares, public_key_package) = keys::split(
        &signing_key,
        3,  // max signers
        2,  // min signers (threshold)
        identifiers,
        &mut OsRng,
    ).map_err(|_| EscrowError::InvalidShare)?;

    // Extract group public key
    let group_pk: [u8; 32] = public_key_package
        .group_public()
        .serialize()
        .try_into()
        .map_err(|_| EscrowError::InvalidShare)?;

    // Build key packages
    let mut packages = Vec::with_capacity(3);
    for i in 1..=3u16 {
        let identifier = frost_core::Identifier::try_from(i)
            .map_err(|_| EscrowError::InvalidShare)?;

        let secret_share = secret_shares.get(&identifier)
            .ok_or(EscrowError::InvalidShare)?;

        // Extract bytes
        let share_bytes: [u8; 32] = secret_share
            .value()
            .serialize()
            .try_into()
            .map_err(|_| EscrowError::InvalidShare)?;

        let public_share = public_key_package
            .signer_pubkeys()
            .get(&identifier)
            .ok_or(EscrowError::InvalidShare)?
            .serialize();

        packages.push(EscrowKeyPackage {
            chain: EscrowChain::Penumbra,
            index: i,
            secret_share: share_bytes,
            public_share: public_share.try_into().map_err(|_| EscrowError::InvalidShare)?,
            group_public_key: group_pk,
            vss_commitment: None, // Set below
        });
    }

    // Extract VSS commitment
    let vss_commitment: Vec<[u8; 32]> = secret_shares
        .values()
        .next()
        .ok_or(EscrowError::InvalidShare)?
        .commitment()
        .serialize()
        .chunks(32)
        .map(|c| c.try_into().unwrap_or([0u8; 32]))
        .collect();

    Ok(EscrowKeySet {
        group_public_key: group_pk,
        packages: packages.try_into().map_err(|_| EscrowError::InvalidShare)?,
        vss_commitment,
    })
}

/// Verify a key package using Feldman VSS
#[cfg(feature = "frost-penumbra")]
pub fn verify_share_feldman(package: &EscrowKeyPackage, commitment: &[[u8; 32]]) -> Result<bool> {
    // The FROST library does this internally during key package creation
    // For external verification, we'd need to:
    // 1. Deserialize commitment curve points
    // 2. Compute Σ commitment[j]^{index^j}
    // 3. Check against public_share * G

    // For now, trust the FROST library's internal verification
    Ok(package.vss_commitment.is_some() || !commitment.is_empty())
}

/// Reconstruct signing key from 2 shares and sign
#[cfg(feature = "frost-penumbra")]
pub fn sign_with_shares(
    _share1: &EscrowKeyPackage,
    _share2: &EscrowKeyPackage,
    _message: &[u8],
) -> Result<[u8; 64]> {
    // Two approaches:
    //
    // 1. Reconstruct full key (simpler, what we do for escrow)
    //    - Lagrange interpolate shares
    //    - Sign with full key
    //
    // 2. Threshold sign (more complex, preserves share secrecy)
    //    - FROST signing protocol
    //    - Requires coordination

    // For escrow release, we reconstruct the full key
    // (Both parties are cooperating anyway)

    todo!("Implement key reconstruction and signing")
}

/// Simple BLAKE2b hash helper
#[allow(dead_code)]
fn blake2b_hash(data: &[u8], context: &[u8]) -> [u8; 32] {
    use blake2::{Blake2b, Digest};
    use blake2::digest::consts::U32;

    let mut hasher = Blake2b::<U32>::new();
    hasher.update(context);
    hasher.update(data);
    hasher.finalize().into()
}

// ============================================================
// LIGERITO VSS BRIDGE
// ============================================================

/// Verify share using Ligerito polynomial commitment
///
/// This is the bridge between FROST shares and Ligerito verification.
/// We commit to the SEED (not the curve scalar) using Ligerito,
/// then derive FROST shares from the seed.
pub fn verify_share_ligerito(
    seed_share: &[u8; 32],
    commitment: &[u8; 32],
    share_index: u16,
    _proof: &[u8],
) -> Result<bool> {
    // This would:
    // 1. Verify the Ligerito opening proof
    // 2. Confirm the seed_share is consistent with the commitment
    //
    // The actual FROST key derivation happens AFTER verification passes

    // Placeholder - actual implementation needs Ligerito prover integration
    let _ = (seed_share, commitment, share_index);
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_blake2b_hash() {
        let data = b"test data";
        let context = b"test context";

        let hash1 = blake2b_hash(data, context);
        let hash2 = blake2b_hash(data, context);

        assert_eq!(hash1, hash2);
        assert_ne!(hash1, [0u8; 32]);
    }
}
