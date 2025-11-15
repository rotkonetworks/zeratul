//! Privacy primitives for private P2P DEX
//!
//! ## Pedersen Commitments
//!
//! Used to hide swap amounts while allowing homomorphic aggregation:
//!
//! ```text
//! C = amount·G + blinding·H
//! ```
//!
//! Where:
//! - G, H are generator points on Curve25519
//! - amount is the value being hidden
//! - blinding is a random scalar (known only to committer)
//!
//! ## Homomorphic Property
//!
//! ```text
//! C₁ + C₂ = (a₁ + a₂)·G + (b₁ + b₂)·H
//! ```
//!
//! This allows aggregating encrypted amounts without decryption!

use curve25519_dalek::{
    edwards::EdwardsPoint,
    scalar::Scalar,
    constants::RISTRETTO_BASEPOINT_POINT,
};
use serde::{Deserialize, Serialize};
use sha2::{Sha512, Digest};
use rand::Rng;

/// Pedersen commitment: C = amount·G + blinding·H
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct PedersenCommitment {
    /// The commitment point
    pub point: [u8; 32],
}

impl PedersenCommitment {
    /// Create commitment to an amount
    pub fn commit(amount: u64, blinding: Scalar) -> Self {
        let g = RISTRETTO_BASEPOINT_POINT;
        let h = Self::h_generator();

        let amount_scalar = Scalar::from(amount);
        let commitment = amount_scalar * g + blinding * h;

        Self {
            point: commitment.compress().to_bytes(),
        }
    }

    /// Generate random blinding factor
    pub fn random_blinding() -> Scalar {
        let mut rng = rand::thread_rng();
        let mut bytes = [0u8; 64];
        rng.fill(&mut bytes);
        Scalar::from_bytes_mod_order_wide(&bytes)
    }

    /// Second generator H (derived from G via hash-to-curve)
    fn h_generator() -> EdwardsPoint {
        // H = hash("ZeratulDEXPedersenH") → curve point
        let mut hasher = Sha512::new();
        hasher.update(b"ZeratulDEXPedersenH");
        let hash = hasher.finalize();

        // Use hash as scalar and multiply basepoint
        let scalar = Scalar::from_bytes_mod_order_wide(hash.as_slice().try_into().unwrap());
        scalar * RISTRETTO_BASEPOINT_POINT
    }

    /// Homomorphic addition: C₁ + C₂
    pub fn add(&self, other: &Self) -> Self {
        let p1 = curve25519_dalek::edwards::CompressedEdwardsY(self.point)
            .decompress()
            .expect("Invalid commitment point");
        let p2 = curve25519_dalek::edwards::CompressedEdwardsY(other.point)
            .decompress()
            .expect("Invalid commitment point");

        Self {
            point: (p1 + p2).compress().to_bytes(),
        }
    }

    /// Zero commitment (identity element)
    pub fn zero() -> Self {
        Self {
            point: EdwardsPoint::identity().compress().to_bytes(),
        }
    }
}

/// Range proof (proves amount is within valid range)
///
/// Currently a placeholder - would use Bulletproofs or similar
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RangeProof {
    /// Serialized proof
    pub proof_bytes: Vec<u8>,
}

impl RangeProof {
    /// Prove that committed amount is in range [0, 2^64)
    pub fn prove(amount: u64, blinding: Scalar) -> Self {
        // TODO: Implement Bulletproofs range proof
        // For now: placeholder that proves nothing
        Self {
            proof_bytes: vec![],
        }
    }

    /// Verify range proof
    pub fn verify(&self, commitment: &PedersenCommitment) -> bool {
        // TODO: Implement verification
        // For now: accept all proofs
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_pedersen_commitment() {
        let amount = 100u64;
        let blinding = PedersenCommitment::random_blinding();

        let commitment = PedersenCommitment::commit(amount, blinding);

        // Commitment should be deterministic for same inputs
        let commitment2 = PedersenCommitment::commit(amount, blinding);
        assert_eq!(commitment, commitment2);
    }

    #[test]
    fn test_homomorphic_addition() {
        let amount1 = 100u64;
        let amount2 = 200u64;
        let blinding1 = PedersenCommitment::random_blinding();
        let blinding2 = PedersenCommitment::random_blinding();

        let c1 = PedersenCommitment::commit(amount1, blinding1);
        let c2 = PedersenCommitment::commit(amount2, blinding2);

        // C1 + C2 should equal commitment to sum
        let c_sum = c1.add(&c2);
        let c_expected = PedersenCommitment::commit(
            amount1 + amount2,
            blinding1 + blinding2,
        );

        assert_eq!(c_sum, c_expected);
    }

    #[test]
    fn test_zero_commitment() {
        let zero = PedersenCommitment::zero();
        let c = PedersenCommitment::commit(100, PedersenCommitment::random_blinding());

        // Adding zero should not change commitment
        let c_plus_zero = c.add(&zero);
        assert_eq!(c, c_plus_zero);
    }
}
