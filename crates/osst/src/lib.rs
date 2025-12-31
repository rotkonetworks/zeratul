//! One-Step Schnorr Threshold Identification (OSST)
//!
//! Implementation of the OSST protocol from:
//! "One-Step Schnorr Threshold Identification" by Foteinos Mergoupis-Anagnou (GRNET)
//!
//! # Properties
//!
//! - **Non-interactive**: provers generate proofs independently
//! - **Share-free**: verifier only needs group public key, not individual shares
//! - **Asynchronous**: provers submit proofs at their own pace
//! - **Threshold**: requires t-of-n provers to verify
//!
//! # Security
//!
//! Proven secure under (t-1)-OMDL assumption in the random oracle model.
//!
//! # Curve Backends
//!
//! - `ristretto255` (default): Polkadot/sr25519 compatible
//! - `pallas`: Zcash Orchard compatible
//!
//! # Example
//!
//! ```ignore
//! use osst::{SecretShare, verify};
//!
//! // After DKG, each custodian has a share
//! let share = SecretShare::new(index, scalar);
//!
//! // Generate contribution (Schnorr proof)
//! let contribution = share.contribute(&mut rng, &payload);
//!
//! // Verifier collects t contributions and verifies
//! let valid = verify(&group_pubkey, &contributions, threshold, &payload)?;
//! ```

#![cfg_attr(not(feature = "std"), no_std)]

extern crate alloc;

use alloc::vec;
use alloc::vec::Vec;
use sha2::{Digest, Sha512};

pub mod curve;
mod error;
mod lagrange;
pub mod liveness;
pub mod reshare;
mod types;

pub use curve::{OsstCurve, OsstPoint, OsstScalar};
pub use error::OsstError;
pub use lagrange::compute_lagrange_coefficients;
pub use types::*;

#[cfg(feature = "ristretto255")]
pub use curve::ristretto::Ristretto255;

#[cfg(feature = "pallas")]
pub use curve::pallas::PallasCurve;

#[cfg(feature = "secp256k1")]
pub use curve::secp256k1::Secp256k1Curve;

#[cfg(feature = "decaf377")]
pub use curve::decaf377::Decaf377Curve;

/// Hash a point and payload to a scalar challenge
/// H(u_i || payload) -> c_i
pub fn hash_to_challenge<S: OsstScalar, P: OsstPoint<Scalar = S>>(
    commitment: &P,
    payload: &[u8],
) -> S {
    let mut hasher = Sha512::new();
    hasher.update(commitment.compress());
    hasher.update(payload);
    let hash: [u8; 64] = hasher.finalize().into();
    S::from_bytes_wide(&hash)
}

/// A secret share from DKG
#[derive(Clone, Debug)]
pub struct SecretShare<S: OsstScalar> {
    /// Shareholder index (1-indexed, as per Shamir convention)
    pub index: u32,
    /// The secret scalar x_i
    pub scalar: S,
}

impl<S: OsstScalar> SecretShare<S> {
    pub fn new(index: u32, scalar: S) -> Self {
        assert!(index > 0, "index must be 1-indexed");
        Self { index, scalar }
    }

    /// Generate an OSST contribution (Schnorr proof for this share)
    ///
    /// Returns (u_i, s_i) where:
    /// - u_i = g^{r_i} (commitment)
    /// - s_i = r_i + c_i * x_i (response)
    /// - c_i = H(u_i || payload)
    pub fn contribute<P: OsstPoint<Scalar = S>, R: rand_core::RngCore + rand_core::CryptoRng>(
        &self,
        rng: &mut R,
        payload: &[u8],
    ) -> Contribution<P> {
        // Sample random nonce
        let r = S::random(rng);

        // Commitment u_i = g^r_i
        let commitment = P::generator().mul_scalar(&r);

        // Challenge c_i = H(u_i || payload)
        let challenge: S = hash_to_challenge(&commitment, payload);

        // Response s_i = r_i + c_i * x_i
        let response = r.add(&challenge.mul(&self.scalar));

        Contribution {
            index: self.index,
            commitment,
            response,
        }
    }

    /// Derive public key share y_i = g^{x_i}
    pub fn public_share<P: OsstPoint<Scalar = S>>(&self) -> P {
        P::generator().mul_scalar(&self.scalar)
    }
}

/// A single custodian's contribution to the threshold proof
#[derive(Clone, Debug)]
pub struct Contribution<P: OsstPoint> {
    /// Custodian's index (1-indexed)
    pub index: u32,
    /// Schnorr commitment u_i = g^{r_i}
    pub commitment: P,
    /// Schnorr response s_i = r_i + c_i * x_i
    pub response: P::Scalar,
}

impl<P: OsstPoint> Contribution<P> {
    pub fn new(index: u32, commitment: P, response: P::Scalar) -> Self {
        Self {
            index,
            commitment,
            response,
        }
    }

    /// Serialize for transmission
    pub fn to_bytes(&self) -> [u8; 68] {
        let mut buf = [0u8; 68];
        buf[0..4].copy_from_slice(&self.index.to_le_bytes());
        buf[4..36].copy_from_slice(&self.commitment.compress());
        buf[36..68].copy_from_slice(&self.response.to_bytes());
        buf
    }

    /// Deserialize
    pub fn from_bytes(bytes: &[u8; 68]) -> Result<Self, OsstError> {
        let index = u32::from_le_bytes(bytes[0..4].try_into().unwrap());

        let point_bytes: [u8; 32] = bytes[4..36].try_into().unwrap();
        let commitment = P::decompress(&point_bytes).ok_or(OsstError::InvalidCommitment)?;

        let response_bytes: [u8; 32] = bytes[36..68].try_into().unwrap();
        let response =
            P::Scalar::from_canonical_bytes(&response_bytes).ok_or(OsstError::InvalidResponse)?;

        Ok(Self {
            index,
            commitment,
            response,
        })
    }
}

/// Compute weights and normalizer for OSST verification
///
/// Given contributions with indices Q = {i_1, ..., i_k} and challenges c_i:
/// - c̄ = Π c_i (normalizer)
/// - μ_i = λ_i * Π_{j≠i} c_j (weight for index i)
///
/// Where λ_i are Lagrange coefficients for set Q
pub fn compute_weights<P: OsstPoint>(
    contributions: &[Contribution<P>],
    payload: &[u8],
) -> Result<(P::Scalar, Vec<P::Scalar>), OsstError> {
    if contributions.is_empty() {
        return Err(OsstError::EmptyContributions);
    }

    let k = contributions.len();

    // Compute challenges c_i = H(u_i || payload)
    let challenges: Vec<P::Scalar> = contributions
        .iter()
        .map(|c| hash_to_challenge(&c.commitment, payload))
        .collect();

    // Check no challenge is zero (overwhelming probability this doesn't happen)
    for c in &challenges {
        if c == &P::Scalar::zero() {
            return Err(OsstError::ZeroChallenge);
        }
    }

    // Compute normalizer c̄ = Π c_i
    let normalizer: P::Scalar = challenges
        .iter()
        .fold(P::Scalar::one(), |acc, c| acc.mul(c));

    // Get indices for Lagrange computation
    let indices: Vec<u32> = contributions.iter().map(|c| c.index).collect();

    // Compute Lagrange coefficients (these are curve-agnostic since they just use scalars)
    let lagrange = compute_lagrange_coefficients::<P::Scalar>(&indices)?;

    // Compute weights μ_i = λ_i * Π_{j≠i} c_j
    let mut weights: Vec<P::Scalar> = Vec::with_capacity(k);
    for i in 0..k {
        let mut weight = lagrange[i].clone();
        for (j, c_j) in challenges.iter().enumerate() {
            if i != j {
                weight = weight.mul(c_j);
            }
        }
        weights.push(weight);
    }

    Ok((normalizer, weights))
}

/// Verify an OSST proof
///
/// Given:
/// - Group public key Y = g^x
/// - Contributions {(i, u_i, s_i)} for indices in Q where |Q| ≥ t
/// - Payload that was signed
///
/// Verify equation (3.3) from the paper:
/// ```text
/// g^{Σ μ_i·s_i} = Y^{c̄} · Π u_i^{μ_i}
/// ```
///
/// Where:
/// - c_i = H(u_i || payload)
/// - c̄ = Π c_i (normalizer)
/// - λ_i = Lagrange coefficient for index i in set Q
/// - μ_i = λ_i · Π_{j≠i} c_j (weight)
pub fn verify<P: OsstPoint>(
    group_pubkey: &P,
    contributions: &[Contribution<P>],
    threshold: u32,
    payload: &[u8],
) -> Result<bool, OsstError> {
    // Check threshold
    if contributions.len() < threshold as usize {
        return Err(OsstError::InsufficientContributions {
            got: contributions.len(),
            need: threshold as usize,
        });
    }

    // Check for duplicate indices
    let mut indices: Vec<u32> = contributions.iter().map(|c| c.index).collect();
    indices.sort();
    for i in 1..indices.len() {
        if indices[i] == indices[i - 1] {
            return Err(OsstError::DuplicateIndex(indices[i]));
        }
    }

    // Compute weights and normalizer
    let (normalizer, weights) = compute_weights(contributions, payload)?;

    // LHS: g^{Σ μ_i·s_i}
    let mut lhs_exponent = P::Scalar::zero();
    for (c, μ) in contributions.iter().zip(weights.iter()) {
        let term = μ.mul(&c.response);
        lhs_exponent = lhs_exponent.add(&term);
    }

    let lhs = P::generator().mul_scalar(&lhs_exponent);

    // RHS: Y^{c̄} · Π u_i^{μ_i}
    // Use multiscalar multiplication for efficiency
    let mut scalars = vec![normalizer];
    let mut points = vec![group_pubkey.clone()];

    for (c, μ) in contributions.iter().zip(weights.iter()) {
        scalars.push(μ.clone());
        points.push(c.commitment.clone());
    }

    let rhs = P::multiscalar_mul(&scalars, &points);

    Ok(lhs == rhs)
}

/// Incremental verification: check if adding a new contribution preserves validity
///
/// Given an already-verified set Q, check if adding contribution j maintains
/// the OSST validity condition.
pub fn verify_incremental<P: OsstPoint>(
    group_pubkey: &P,
    existing: &[Contribution<P>],
    new_contribution: &Contribution<P>,
    threshold: u32,
    payload: &[u8],
) -> Result<bool, OsstError> {
    // Check new index doesn't duplicate
    for c in existing {
        if c.index == new_contribution.index {
            return Err(OsstError::DuplicateIndex(new_contribution.index));
        }
    }

    // Combine and verify full set
    let mut all: Vec<Contribution<P>> = existing.to_vec();
    all.push(new_contribution.clone());

    verify(group_pubkey, &all, threshold, payload)
}

// ============================================================================
// Ristretto255-specific convenience exports (default)
// ============================================================================

#[cfg(feature = "ristretto255")]
pub mod ristretto255 {
    //! Ristretto255 (Polkadot-compatible) OSST types
    use curve25519_dalek::{ristretto::RistrettoPoint, scalar::Scalar};

    /// Secret share with ristretto255 scalar
    pub type SecretShare = super::SecretShare<Scalar>;

    /// Contribution with ristretto255 point
    pub type Contribution = super::Contribution<RistrettoPoint>;

    /// The generator point G
    pub const G: RistrettoPoint = curve25519_dalek::constants::RISTRETTO_BASEPOINT_POINT;
}

#[cfg(feature = "ristretto255")]
pub use ristretto255::{Contribution as RistrettoContribution, SecretShare as RistrettoSecretShare};

// ============================================================================
// Pallas-specific convenience exports
// ============================================================================

#[cfg(feature = "pallas")]
pub mod pallas {
    //! Pallas (Zcash Orchard-compatible) OSST types
    use pasta_curves::pallas::{Point, Scalar};

    /// Secret share with Pallas scalar
    pub type SecretShare = super::SecretShare<Scalar>;

    /// Contribution with Pallas point
    pub type Contribution = super::Contribution<Point>;
}

#[cfg(feature = "pallas")]
pub use pallas::{Contribution as PallasContribution, SecretShare as PallasSecretShare};

// ============================================================================
// Tests (Pallas)
// ============================================================================

#[cfg(all(test, feature = "pallas"))]
mod pallas_tests {
    use super::*;
    use pasta_curves::pallas::{Point, Scalar};
    use pasta_curves::group::ff::Field;
    use rand::rngs::OsRng;

    use crate::curve::OsstPoint;

    /// Simulate Shamir secret sharing for testing
    fn shamir_split(secret: &Scalar, n: u32, t: u32) -> Vec<SecretShare<Scalar>> {
        assert!(t <= n);
        assert!(t > 0);

        let mut rng = OsRng;

        let mut coeffs = vec![*secret];
        for _ in 1..t {
            coeffs.push(<Scalar as Field>::random(&mut rng));
        }

        (1..=n)
            .map(|i| {
                let x = Scalar::from(i as u64);
                let mut y = Scalar::ZERO;
                let mut x_pow = Scalar::ONE;

                for coeff in &coeffs {
                    y += coeff * x_pow;
                    x_pow *= x;
                }

                SecretShare::new(i, y)
            })
            .collect()
    }

    #[test]
    fn test_pallas_basic_osst() {
        let mut rng = OsRng;

        let secret = <Scalar as Field>::random(&mut rng);
        let group_pubkey: Point = Point::generator().mul_scalar(&secret);

        let n = 5u32;
        let t = 3u32;
        let shares = shamir_split(&secret, n, t);

        let payload = b"test pallas osst verification";

        let contributions: Vec<Contribution<Point>> = shares[0..t as usize]
            .iter()
            .map(|s| s.contribute(&mut rng, payload))
            .collect();

        let result = verify(&group_pubkey, &contributions, t, payload);
        assert!(result.is_ok());
        assert!(result.unwrap(), "Pallas OSST verification should succeed");
    }

    #[test]
    fn test_pallas_wrong_payload() {
        let mut rng = OsRng;

        let secret = <Scalar as Field>::random(&mut rng);
        let group_pubkey: Point = Point::generator().mul_scalar(&secret);

        let n = 5u32;
        let t = 3u32;
        let shares = shamir_split(&secret, n, t);

        let payload = b"correct payload";
        let wrong_payload = b"wrong payload";

        let contributions: Vec<Contribution<Point>> = shares[0..t as usize]
            .iter()
            .map(|s| s.contribute(&mut rng, payload))
            .collect();

        let result = verify(&group_pubkey, &contributions, t, wrong_payload);
        assert!(result.is_ok());
        assert!(
            !result.unwrap(),
            "Pallas verification with wrong payload should fail"
        );
    }

    #[test]
    fn test_pallas_serialization() {
        let mut rng = OsRng;

        let secret = <Scalar as Field>::random(&mut rng);
        let shares = shamir_split(&secret, 3, 2);

        let payload = b"pallas serialization test";
        let original: Contribution<Point> = shares[0].contribute(&mut rng, payload);

        let bytes = original.to_bytes();
        let recovered = Contribution::<Point>::from_bytes(&bytes).unwrap();

        assert_eq!(original.index, recovered.index);
        assert_eq!(original.commitment, recovered.commitment);
        assert_eq!(original.response, recovered.response);
    }
}

// ============================================================================
// Tests (ristretto255)
// ============================================================================

#[cfg(all(test, feature = "ristretto255"))]
mod tests {
    use super::*;
    use curve25519_dalek::{ristretto::RistrettoPoint, scalar::Scalar};
    use rand::rngs::OsRng;

    /// Simulate Shamir secret sharing for testing
    fn shamir_split(secret: &Scalar, n: u32, t: u32) -> Vec<SecretShare<Scalar>> {
        assert!(t <= n);
        assert!(t > 0);

        let mut rng = OsRng;

        // Generate random polynomial coefficients a_0, a_1, ..., a_{t-1}
        // where a_0 = secret
        let mut coeffs = vec![*secret];
        for _ in 1..t {
            coeffs.push(Scalar::random(&mut rng));
        }

        // Evaluate polynomial at points 1, 2, ..., n
        (1..=n)
            .map(|i| {
                let x = Scalar::from(i);
                let mut y = Scalar::ZERO;
                let mut x_pow = Scalar::ONE;

                for coeff in &coeffs {
                    y += coeff * x_pow;
                    x_pow *= x;
                }

                SecretShare::new(i, y)
            })
            .collect()
    }

    #[test]
    fn test_basic_osst() {
        let mut rng = OsRng;

        // Setup: create group key and shares
        let secret = Scalar::random(&mut rng);
        let group_pubkey: RistrettoPoint = RistrettoPoint::generator().mul_scalar(&secret);

        let n = 5u32;
        let t = 3u32;
        let shares = shamir_split(&secret, n, t);

        // Payload to sign
        let payload = b"test payload for osst verification";

        // Generate contributions from t shareholders
        let contributions: Vec<Contribution<RistrettoPoint>> = shares[0..t as usize]
            .iter()
            .map(|s| s.contribute(&mut rng, payload))
            .collect();

        // Verify
        let result = verify(&group_pubkey, &contributions, t, payload);
        assert!(result.is_ok());
        assert!(result.unwrap(), "OSST verification should succeed");
    }

    #[test]
    fn test_osst_with_more_than_threshold() {
        let mut rng = OsRng;

        let secret = Scalar::random(&mut rng);
        let group_pubkey: RistrettoPoint = RistrettoPoint::generator().mul_scalar(&secret);

        let n = 7u32;
        let t = 4u32;
        let shares = shamir_split(&secret, n, t);

        let payload = b"threshold exceeded test";

        // Use 5 contributors (more than threshold of 4)
        let contributions: Vec<Contribution<RistrettoPoint>> = shares[0..5]
            .iter()
            .map(|s| s.contribute(&mut rng, payload))
            .collect();

        let result = verify(&group_pubkey, &contributions, t, payload);
        assert!(result.is_ok());
        assert!(
            result.unwrap(),
            "verification with >t contributors should work"
        );
    }

    #[test]
    fn test_osst_insufficient_threshold() {
        let mut rng = OsRng;

        let secret = Scalar::random(&mut rng);
        let group_pubkey: RistrettoPoint = RistrettoPoint::generator().mul_scalar(&secret);

        let n = 5u32;
        let t = 3u32;
        let shares = shamir_split(&secret, n, t);

        let payload = b"insufficient threshold test";

        // Only 2 contributors (less than threshold of 3)
        let contributions: Vec<Contribution<RistrettoPoint>> = shares[0..2]
            .iter()
            .map(|s| s.contribute(&mut rng, payload))
            .collect();

        let result = verify(&group_pubkey, &contributions, t, payload);
        assert!(matches!(
            result,
            Err(OsstError::InsufficientContributions { .. })
        ));
    }

    #[test]
    fn test_osst_wrong_payload() {
        let mut rng = OsRng;

        let secret = Scalar::random(&mut rng);
        let group_pubkey: RistrettoPoint = RistrettoPoint::generator().mul_scalar(&secret);

        let n = 5u32;
        let t = 3u32;
        let shares = shamir_split(&secret, n, t);

        let payload = b"correct payload";
        let wrong_payload = b"wrong payload";

        // Generate contributions with correct payload
        let contributions: Vec<Contribution<RistrettoPoint>> = shares[0..t as usize]
            .iter()
            .map(|s| s.contribute(&mut rng, payload))
            .collect();

        // Verify with wrong payload
        let result = verify(&group_pubkey, &contributions, t, wrong_payload);
        assert!(result.is_ok());
        assert!(
            !result.unwrap(),
            "verification with wrong payload should fail"
        );
    }

    #[test]
    fn test_osst_wrong_pubkey() {
        let mut rng = OsRng;

        let secret = Scalar::random(&mut rng);
        let group_pubkey: RistrettoPoint = RistrettoPoint::generator().mul_scalar(&secret);

        // Different public key
        let wrong_secret = Scalar::random(&mut rng);
        let wrong_pubkey: RistrettoPoint = RistrettoPoint::generator().mul_scalar(&wrong_secret);

        let n = 5u32;
        let t = 3u32;
        let shares = shamir_split(&secret, n, t);

        let payload = b"test";

        let contributions: Vec<Contribution<RistrettoPoint>> = shares[0..t as usize]
            .iter()
            .map(|s| s.contribute(&mut rng, payload))
            .collect();

        // Verify against wrong public key
        let result = verify(&wrong_pubkey, &contributions, t, payload);
        assert!(result.is_ok());
        assert!(
            !result.unwrap(),
            "verification with wrong pubkey should fail"
        );
    }

    #[test]
    fn test_incremental_verification() {
        let mut rng = OsRng;

        let secret = Scalar::random(&mut rng);
        let group_pubkey: RistrettoPoint = RistrettoPoint::generator().mul_scalar(&secret);

        let n = 5u32;
        let t = 3u32;
        let shares = shamir_split(&secret, n, t);

        let payload = b"incremental test";

        // Start with threshold contributions
        let contributions: Vec<Contribution<RistrettoPoint>> = shares[0..t as usize]
            .iter()
            .map(|s| s.contribute(&mut rng, payload))
            .collect();

        // Verify initial set
        assert!(verify(&group_pubkey, &contributions, t, payload).unwrap());

        // Add one more contribution incrementally
        let new_contrib = shares[t as usize].contribute(&mut rng, payload);
        let result = verify_incremental(&group_pubkey, &contributions, &new_contrib, t, payload);
        assert!(result.is_ok());
        assert!(result.unwrap(), "incremental verification should succeed");
    }

    #[test]
    fn test_contribution_serialization() {
        let mut rng = OsRng;

        let secret = Scalar::random(&mut rng);
        let shares = shamir_split(&secret, 3, 2);

        let payload = b"serialization test";
        let original: Contribution<RistrettoPoint> = shares[0].contribute(&mut rng, payload);

        let bytes = original.to_bytes();
        let recovered = Contribution::<RistrettoPoint>::from_bytes(&bytes).unwrap();

        assert_eq!(original.index, recovered.index);
        assert_eq!(original.commitment, recovered.commitment);
        assert_eq!(original.response, recovered.response);
    }

    #[test]
    fn test_large_threshold() {
        let mut rng = OsRng;

        let secret = Scalar::random(&mut rng);
        let group_pubkey: RistrettoPoint = RistrettoPoint::generator().mul_scalar(&secret);

        // Simulate n=1023, t=683 (as discussed)
        // For test speed, use smaller values
        let n = 100u32;
        let t = 67u32; // ~2/3

        let shares = shamir_split(&secret, n, t);
        let payload = b"large threshold test";

        let contributions: Vec<Contribution<RistrettoPoint>> = shares[0..t as usize]
            .iter()
            .map(|s| s.contribute(&mut rng, payload))
            .collect();

        let result = verify(&group_pubkey, &contributions, t, payload);
        assert!(result.is_ok());
        assert!(
            result.unwrap(),
            "large threshold verification should succeed"
        );
    }
}

/// secp256k1 (bitcoin) curve tests
#[cfg(all(test, feature = "secp256k1"))]
mod secp256k1_tests {
    use super::*;
    use k256::ProjectivePoint;
    use k256::Scalar;
    use rand::rngs::OsRng;

    fn shamir_split_secp(secret: &Scalar, n: u32, t: u32) -> Vec<SecretShare<Scalar>> {
        let mut rng = OsRng;
        let mut coefficients = vec![*secret];
        for _ in 1..t {
            coefficients.push(<Scalar as OsstScalar>::random(&mut rng));
        }

        (1..=n)
            .map(|i| {
                let x = <Scalar as OsstScalar>::from_u32(i);
                let mut y = <Scalar as OsstScalar>::zero();
                let mut x_pow = <Scalar as OsstScalar>::one();
                for coeff in &coefficients {
                    y = y.add(&coeff.mul(&x_pow));
                    x_pow = x_pow.mul(&x);
                }
                SecretShare::new(i, y)
            })
            .collect()
    }

    #[test]
    fn test_secp256k1_basic_osst() {
        let mut rng = OsRng;

        let secret = <Scalar as OsstScalar>::random(&mut rng);
        let group_pubkey: ProjectivePoint = ProjectivePoint::GENERATOR.mul_scalar(&secret);

        let n = 5u32;
        let t = 3u32;
        let shares = shamir_split_secp(&secret, n, t);

        let payload = b"bitcoin custody test";

        let contributions: Vec<Contribution<ProjectivePoint>> = shares[0..t as usize]
            .iter()
            .map(|s| s.contribute(&mut rng, payload))
            .collect();

        let result = verify(&group_pubkey, &contributions, t, payload);
        assert!(result.is_ok(), "secp256k1 verification should not error");
        assert!(result.unwrap(), "secp256k1 verification should succeed");
    }

    #[test]
    fn test_secp256k1_point_serialization() {
        let mut rng = OsRng;

        let scalar = <Scalar as OsstScalar>::random(&mut rng);
        let point: ProjectivePoint = ProjectivePoint::GENERATOR.mul_scalar(&scalar);

        // test 32-byte compression (x-coord only)
        let compressed = point.compress();
        let decompressed = ProjectivePoint::decompress(&compressed);
        // note: may not match exactly due to y-ambiguity
        assert!(decompressed.is_some(), "should decompress 32-byte form");

        // test full 33-byte compression
        let full_compressed = point.compress_vec();
        assert_eq!(full_compressed.len(), 33, "secp256k1 should be 33 bytes");

        let full_decompressed = ProjectivePoint::decompress_slice(&full_compressed);
        assert!(full_decompressed.is_some(), "should decompress 33-byte form");
        assert_eq!(point, full_decompressed.unwrap(), "roundtrip should match");
    }

    #[test]
    fn test_secp256k1_threshold_2_of_3() {
        let mut rng = OsRng;

        let secret = <Scalar as OsstScalar>::random(&mut rng);
        let group_pubkey: ProjectivePoint = ProjectivePoint::GENERATOR.mul_scalar(&secret);

        let n = 3u32;
        let t = 2u32;
        let shares = shamir_split_secp(&secret, n, t);

        let payload = b"2-of-3 multisig";

        // use shares 1 and 3 (not consecutive)
        let contributions: Vec<Contribution<ProjectivePoint>> = vec![
            shares[0].contribute(&mut rng, payload),
            shares[2].contribute(&mut rng, payload),
        ];

        let result = verify(&group_pubkey, &contributions, t, payload);
        assert!(result.is_ok());
        assert!(result.unwrap(), "2-of-3 secp256k1 should verify");
    }
}

/// decaf377 (penumbra) curve tests
#[cfg(all(test, feature = "decaf377"))]
mod decaf377_tests {
    use super::*;
    use ::decaf377::{Element, Fr};
    use rand::rngs::OsRng;

    fn shamir_split_decaf(secret: &Fr, n: u32, t: u32) -> Vec<SecretShare<Fr>> {
        let mut rng = OsRng;
        let mut coefficients = vec![*secret];
        for _ in 1..t {
            coefficients.push(<Fr as OsstScalar>::random(&mut rng));
        }

        (1..=n)
            .map(|i| {
                let x = <Fr as OsstScalar>::from_u32(i);
                let mut y = <Fr as OsstScalar>::zero();
                let mut x_pow = <Fr as OsstScalar>::one();
                for coeff in &coefficients {
                    y = y.add(&coeff.mul(&x_pow));
                    x_pow = x_pow.mul(&x);
                }
                SecretShare::new(i, y)
            })
            .collect()
    }

    #[test]
    fn test_decaf377_basic_osst() {
        let mut rng = OsRng;

        let secret = <Fr as OsstScalar>::random(&mut rng);
        let group_pubkey: Element = Element::GENERATOR * secret;

        let n = 5u32;
        let t = 3u32;
        let shares = shamir_split_decaf(&secret, n, t);

        let payload = b"penumbra custody test";

        let contributions: Vec<Contribution<Element>> = shares[0..t as usize]
            .iter()
            .map(|s| s.contribute(&mut rng, payload))
            .collect();

        let result = verify(&group_pubkey, &contributions, t, payload);
        assert!(result.is_ok(), "decaf377 verification should not error");
        assert!(result.unwrap(), "decaf377 verification should succeed");
    }

    #[test]
    fn test_decaf377_point_serialization() {
        let mut rng = OsRng;

        let scalar = <Fr as OsstScalar>::random(&mut rng);
        let point: Element = Element::GENERATOR * scalar;

        let compressed = point.compress();
        let decompressed = Element::decompress(&compressed);
        assert!(decompressed.is_some(), "should decompress");
        assert_eq!(point, decompressed.unwrap(), "roundtrip should match");
    }

    #[test]
    fn test_decaf377_threshold_2_of_3() {
        let mut rng = OsRng;

        let secret = <Fr as OsstScalar>::random(&mut rng);
        let group_pubkey: Element = Element::GENERATOR * secret;

        let n = 3u32;
        let t = 2u32;
        let shares = shamir_split_decaf(&secret, n, t);

        let payload = b"2-of-3 penumbra multisig";

        // use shares 1 and 3 (not consecutive)
        let contributions: Vec<Contribution<Element>> = vec![
            shares[0].contribute(&mut rng, payload),
            shares[2].contribute(&mut rng, payload),
        ];

        let result = verify(&group_pubkey, &contributions, t, payload);
        assert!(result.is_ok());
        assert!(result.unwrap(), "2-of-3 decaf377 should verify");
    }
}
