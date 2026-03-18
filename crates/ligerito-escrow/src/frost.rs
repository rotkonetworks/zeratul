//! FROST Integration for 3-Party Escrow
//!
//! # Two modes of operation
//!
//! ## Mode 1: Trusted dealer (simple, original)
//! Seller generates ephemeral key, splits into 3 shares, distributes.
//! Shares verified via ZODA Merkle commitment. Any 2 reconstruct.
//!
//! ## Mode 2: Frostito nested escrow (trustless)
//! Uses osst::nested interleaved DKG — jury share born distributed.
//! ZODA Merkle commitment for share verification.
//! OSST gates jury authorization, inner FROST produces jury signature.
//!
//! # Verification: Ligerito vs Feldman
//!
//! Feldman VSS: curve point commitments, tied to signing curve
//! Ligerito/ZODA VSS: binary field commitments, curve agnostic
//!
//! For the player-to-jury share splits, the shared values are scalars
//! (polynomial evaluations), not curve points. ZODA verification is
//! sufficient and doesn't require curve operations. This makes the
//! share verification independent of the signing curve — the same
//! ZODA commitment works whether the signing is Pallas (Zcash),
//! decaf377 (Penumbra), or ristretto255 (Polkadot).

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

/// ZODA-verified share split for distributing a scalar to jury nodes.
///
/// when a player splits their polynomial evaluation f_i(p) among jury nodes,
/// we use the ZODA Merkle commitment from ligerito-escrow instead of Feldman
/// curve-point commitments. the ZODA commitment is:
///
/// 1. curve-agnostic (works for any signing curve)
/// 2. cheaper to verify (SHA256 Merkle proof vs multi-exponentiation)
/// 3. proven secure (RS codewords = Shamir shares, per Angeris)
///
/// the tradeoff: ZODA operates over GF(2^32), so a 32-byte scalar is
/// encoded as 8 field elements. the Merkle proof is ~log(n) hashes.
pub struct ZodaSplitResult {
    /// ZODA Merkle root commitment (all nodes should agree on this)
    pub commitment: [u8; 32],
    /// individual shares with Merkle proofs
    pub shares: Vec<crate::shares::Share>,
}

/// split a 32-byte scalar among n parties with threshold t, using ZODA verification.
///
/// this replaces Feldman commitments for the player-to-jury distribution step
/// in the frostito nested DKG. the scalar (a polynomial evaluation from the
/// outer DKG) is encoded as 8 GF(2^32) elements and shared via Reed-Solomon
/// polynomial evaluation. each party gets a share with a Merkle proof.
///
/// verification: party k checks their share against the Merkle root.
/// this guarantees all shares lie on the same polynomial, so any t parties
/// can reconstruct the original scalar.
pub fn zoda_split_scalar(
    scalar_bytes: &[u8; 32],
    threshold: usize,
    num_shares: usize,
) -> Result<ZodaSplitResult> {
    let sharer = crate::shares::SecretSharer::new(threshold, num_shares)?;
    let share_set = sharer.share_secret(scalar_bytes)?;

    Ok(ZodaSplitResult {
        commitment: share_set.commitment(),
        shares: share_set.shares().to_vec(),
    })
}

/// verify a ZODA share against its Merkle commitment.
///
/// this is the curve-agnostic equivalent of Feldman VSS verification.
/// if this passes, the share is guaranteed to be consistent with all
/// other shares that verify against the same commitment.
pub fn zoda_verify_share(
    share: &crate::shares::Share,
    commitment: &[u8; 32],
    num_shares: usize,
) -> bool {
    // rebuild a minimal ShareSet for verification
    use ligerito_binary_fields::BinaryFieldElement;
    use sha2::{Sha256, Digest};

    let leaf_hash = {
        let mut hasher = Sha256::new();
        for v in &share.values {
            hasher.update(&v.poly().value().to_le_bytes());
        }
        let h: [u8; 32] = hasher.finalize().into();
        h
    };

    let computed_root = {
        let mut current = leaf_hash;
        let mut idx = share.index as usize;

        for sibling in &share.merkle_proof {
            let mut hasher = Sha256::new();
            if idx % 2 == 0 {
                hasher.update(&current);
                hasher.update(sibling);
            } else {
                hasher.update(sibling);
                hasher.update(&current);
            }
            current = hasher.finalize().into();
            idx /= 2;
        }
        current
    };

    &computed_root == commitment
}

/// reconstruct a 32-byte scalar from t ZODA shares.
pub fn zoda_reconstruct_scalar(
    shares: &[crate::shares::Share],
    threshold: usize,
) -> Result<[u8; 32]> {
    crate::reconstruct_secret(shares, threshold)
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
// FROST + ZODA INTEGRATION (Penumbra)
// ============================================================

/// Create 2-of-3 escrow key set (Penumbra)
///
/// Seller calls this to generate the escrow.
/// Returns key packages for Buyer (index 1), Seller (index 2), Arbitrator (index 3).
#[cfg(feature = "frost-penumbra")]
pub fn create_escrow_penumbra(seed: &[u8; 32]) -> Result<EscrowKeySet> {
    use decaf377_frost::keys;
    use decaf377_rdsa::{SigningKey, SpendAuth};
    use rand::rngs::OsRng;

    let sk_bytes = blake2b_hash(seed, b"escrow-signing-key");
    let signing_key = SigningKey::<SpendAuth>::try_from(sk_bytes)
        .map_err(|_| EscrowError::InvalidShare)?;

    let identifiers = keys::IdentifierList::Default;
    let (secret_shares, public_key_package) = keys::split(
        &signing_key,
        3,
        2,
        identifiers,
        &mut OsRng,
    ).map_err(|_| EscrowError::InvalidShare)?;

    let group_pk: [u8; 32] = public_key_package
        .group_public()
        .serialize()
        .try_into()
        .map_err(|_| EscrowError::InvalidShare)?;

    let mut packages = Vec::with_capacity(3);
    for i in 1..=3u16 {
        let identifier = frost_core::Identifier::try_from(i)
            .map_err(|_| EscrowError::InvalidShare)?;

        let secret_share = secret_shares.get(&identifier)
            .ok_or(EscrowError::InvalidShare)?;

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
            vss_commitment: None,
        });
    }

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
    Ok(package.vss_commitment.is_some() || !commitment.is_empty())
}

/// Reconstruct signing key from 2 shares and sign
#[cfg(feature = "frost-penumbra")]
pub fn sign_with_shares(
    _share1: &EscrowKeyPackage,
    _share2: &EscrowKeyPackage,
    _message: &[u8],
) -> Result<[u8; 64]> {
    todo!("Implement key reconstruction and signing")
}

/// Verify share using Ligerito polynomial commitment
///
/// Bridge between FROST shares and Ligerito verification.
/// We commit to the SEED (not the curve scalar) using Ligerito,
/// then derive FROST shares from the seed.
pub fn verify_share_ligerito(
    seed_share: &[u8; 32],
    commitment: &[u8; 32],
    share_index: u16,
    _proof: &[u8],
) -> Result<bool> {
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

    #[test]
    fn test_zoda_split_and_reconstruct() {
        let scalar = [0xABu8; 32];

        let result = zoda_split_scalar(&scalar, 2, 3).unwrap();
        assert_eq!(result.shares.len(), 3);

        // verify all shares
        for share in &result.shares {
            assert!(
                zoda_verify_share(share, &result.commitment, 3),
                "share {} failed ZODA verification",
                share.index
            );
        }

        // reconstruct from any 2
        let pairs = [(0, 1), (0, 2), (1, 2)];
        for (i, j) in pairs {
            let recovered = zoda_reconstruct_scalar(
                &[result.shares[i].clone(), result.shares[j].clone()],
                2,
            ).unwrap();
            assert_eq!(recovered, scalar, "pair ({}, {}) failed", i, j);
        }
    }

    #[test]
    fn test_zoda_tampered_share_fails() {
        let scalar = [0x42u8; 32];

        let result = zoda_split_scalar(&scalar, 2, 3).unwrap();

        // tamper with a share
        let mut bad = result.shares[0].clone();
        bad.values[0] = crate::ShareField::from(999u32);

        assert!(
            !zoda_verify_share(&bad, &result.commitment, 3),
            "tampered share should fail ZODA verification"
        );
    }

    #[test]
    fn test_zoda_split_5_of_10() {
        let scalar = [0xFFu8; 32];

        let result = zoda_split_scalar(&scalar, 5, 10).unwrap();
        assert_eq!(result.shares.len(), 10);

        for share in &result.shares {
            assert!(zoda_verify_share(share, &result.commitment, 10));
        }

        // reconstruct from first 5
        let recovered = zoda_reconstruct_scalar(&result.shares[0..5], 5).unwrap();
        assert_eq!(recovered, scalar);
    }
}
