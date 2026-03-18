//! FROST: Flexible Round-Optimized Schnorr Threshold signatures
//!
//! Two-round threshold signing protocol producing standard Schnorr
//! signatures verifiable with the group public key alone.
//!
//! Reference: Komlo & Goldberg, "FROST: Flexible Round-Optimized Schnorr
//! Threshold Signatures" (SAC 2020). Standardized as RFC 9591.
//!
//! # Why FROST (not OSST)
//!
//! OSST produces a *threshold identification proof* — it proves that t-of-n
//! parties cooperated, but the output is not a standard Schnorr signature.
//! FROST produces a standard (R, z) Schnorr signature indistinguishable from
//! a single-signer signature. This matters when the signature must be verified
//! by an external system (e.g., the zcash network verifying a spend
//! authorization) that only understands standard Schnorr.
//!
//! # Protocol
//!
//! ```text
//! Round 1 (commitment):
//!     Each signer i samples nonces (d_i, e_i), broadcasts (D_i, E_i).
//!
//! Round 2 (signing):
//!     Given message m and commitment list B:
//!       ρ_i = H_bind(i, m, B)                  (binding factor)
//!       R   = Σ (D_i + ρ_i · E_i)              (group commitment)
//!       c   = H_sig(R, Y, m)                    (Schnorr challenge)
//!       z_i = d_i + ρ_i · e_i + λ_i · c · s_i  (signature share)
//!
//! Aggregation:
//!       z = Σ z_i
//!       σ = (R, z)
//!
//! Verification:
//!       g^z == R + c · Y
//! ```
//!
//! # Security
//!
//! - **Nonce reuse is catastrophic.** Reusing a nonce pair across different
//!   messages leaks the signer's long-term secret. The [`Nonces`] type is
//!   consumed by [`sign`], preventing reuse at the type level.
//!
//! - **Binding factors** prevent a malicious signer from choosing their
//!   commitment adaptively after seeing others' commitments. Each signer's
//!   binding nonce is mixed with the full commitment list.
//!
//! - **Share verification** allows detecting a misbehaving signer before
//!   aggregation, given their public verification share `Y_i = g^{s_i}`.
//!
//! # Ciphersuite
//!
//! This module uses SHA-512 with domain separation for the binding factor
//! and challenge computations. For zcash RedPallas compatibility, a
//! ciphersuite adapter would override the challenge hash to match zcash's
//! BLAKE2b-based construction.

use alloc::collections::BTreeMap;
use alloc::vec::Vec;
use sha2::{Digest, Sha512};

use crate::curve::{OsstPoint, OsstScalar};
use crate::error::OsstError;
use crate::lagrange::compute_lagrange_coefficients;
use crate::SecretShare;

// ============================================================================
// Types
// ============================================================================

/// Secret nonce pair for a single signing session.
///
/// # Security
///
/// Each `Nonces` value MUST be used for exactly one call to [`sign`].
/// `sign()` takes ownership, preventing reuse. Nonces are zeroized on drop.
///
/// Nonce reuse across different messages leaks the signer's long-term
/// secret key: given two signatures (R, z) and (R, z') on messages m, m'
/// with the same R, an attacker recovers s_i from z - z'.
pub struct Nonces<S: OsstScalar> {
    hiding: S,
    binding: S,
}

impl<S: OsstScalar> Nonces<S> {
    /// Compute FROST Round 2 response: z_i = d_i + (rho_i * e_i) + (lambda_i * c * s_i)
    pub fn compute_response(
        self,
        rho: &S,
        lambda: &S,
        challenge: &S,
        secret: &S,
    ) -> S {
        // z_i = d_i + rho_i * e_i + lambda_i * c * s_i
        let rho_e = rho.mul(&self.binding);
        let lcs = lambda.mul(&challenge.mul(secret));
        self.hiding.add(&rho_e).add(&lcs)
    }
}

impl<S: OsstScalar> Drop for Nonces<S> {
    fn drop(&mut self) {
        self.hiding.zeroize();
        self.binding.zeroize();
    }
}

impl<S: OsstScalar> core::fmt::Debug for Nonces<S> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.write_str("Nonces([REDACTED])")
    }
}

/// Public nonce commitments broadcast in Round 1.
///
/// Safe to transmit in the clear. Each signer broadcasts exactly one
/// `SigningCommitments` per signing session.
#[derive(Clone, Debug)]
pub struct SigningCommitments<P: OsstPoint> {
    /// Signer index (1-indexed, matching [`SecretShare`]).
    pub index: u32,
    /// Hiding commitment: D_i = g^{d_i}
    pub hiding: P,
    /// Binding commitment: E_i = g^{e_i}
    pub binding: P,
}

impl<P: OsstPoint> SigningCommitments<P> {
    /// Serialize to bytes: [index:4][D:32][E:32] = 68 bytes
    pub fn to_bytes(&self) -> [u8; 68] {
        let mut buf = [0u8; 68];
        buf[0..4].copy_from_slice(&self.index.to_le_bytes());
        buf[4..36].copy_from_slice(&self.hiding.compress());
        buf[36..68].copy_from_slice(&self.binding.compress());
        buf
    }

    /// Deserialize from bytes.
    pub fn from_bytes(bytes: &[u8; 68]) -> Result<Self, OsstError> {
        let index = u32::from_le_bytes(bytes[0..4].try_into().unwrap());
        if index == 0 {
            return Err(OsstError::InvalidIndex);
        }
        let hiding_bytes: [u8; 32] = bytes[4..36].try_into().unwrap();
        let binding_bytes: [u8; 32] = bytes[36..68].try_into().unwrap();
        let hiding = P::decompress(&hiding_bytes).ok_or(OsstError::InvalidCommitment)?;
        let binding = P::decompress(&binding_bytes).ok_or(OsstError::InvalidCommitment)?;
        Ok(Self {
            index,
            hiding,
            binding,
        })
    }
}

/// Collected commitments and message, distributed to signers for Round 2.
///
/// Constructed by any participant after collecting commitments from at
/// least t signers. The commitment ordering is deterministic (sorted by
/// index) to ensure all signers compute identical binding factors.
pub struct SigningPackage<P: OsstPoint> {
    message: Vec<u8>,
    commitments: BTreeMap<u32, SigningCommitments<P>>,
    /// cached encoded commitments for binding factor computation
    encoded_commitments: Vec<u8>,
}

impl<P: OsstPoint> SigningPackage<P> {
    /// Construct a signing package from a message and collected commitments.
    ///
    /// Commitments are stored sorted by index. Duplicate indices are rejected.
    pub fn new(
        message: Vec<u8>,
        commitments: Vec<SigningCommitments<P>>,
    ) -> Result<Self, OsstError> {
        let mut map = BTreeMap::new();
        for c in commitments {
            if c.index == 0 {
                return Err(OsstError::InvalidIndex);
            }
            if map.contains_key(&c.index) {
                return Err(OsstError::DuplicateIndex(c.index));
            }
            map.insert(c.index, c);
        }
        if map.is_empty() {
            return Err(OsstError::EmptyContributions);
        }

        let encoded = encode_commitments(&map);

        Ok(Self {
            message,
            commitments: map,
            encoded_commitments: encoded,
        })
    }

    /// The message being signed.
    #[inline]
    pub fn message(&self) -> &[u8] {
        &self.message
    }

    /// Number of signers in this session.
    #[inline]
    pub fn num_signers(&self) -> usize {
        self.commitments.len()
    }

    /// Signer indices in sorted order.
    pub fn signer_indices(&self) -> Vec<u32> {
        self.commitments.keys().copied().collect()
    }

    /// Get a signer's commitments by index.
    pub fn get_commitments(&self, index: u32) -> Option<&SigningCommitments<P>> {
        self.commitments.get(&index)
    }

    /// Compute the binding factor for signer i.
    ///
    /// ρ_i = H("frost-binding-v1" || index || message || encoded_commitments)
    fn binding_factor(&self, index: u32) -> P::Scalar {
        compute_binding_factor::<P::Scalar>(
            index,
            &self.message,
            &self.encoded_commitments,
        )
    }

    /// Compute the group commitment R = Σ (D_i + ρ_i · E_i).
    fn group_commitment(&self) -> P {
        let mut r = P::identity();
        for (_, c) in &self.commitments {
            let rho = self.binding_factor(c.index);
            // D_i + ρ_i · E_i
            let bound = c.binding.mul_scalar(&rho);
            r = r.add(&c.hiding);
            r = r.add(&bound);
        }
        r
    }

    /// Compute the Schnorr challenge c = H("frost-challenge-v1" || R || Y || m).
    fn challenge(&self, group_commitment: &P, group_pubkey: &P) -> P::Scalar {
        compute_challenge::<P>(group_commitment, group_pubkey, &self.message)
    }
}

/// A single signer's share of the aggregate signature.
///
/// z_i = d_i + ρ_i · e_i + λ_i · c · s_i
pub struct SignatureShare<S: OsstScalar> {
    /// Signer index.
    pub index: u32,
    /// Partial response.
    pub response: S,
}

impl<S: OsstScalar> SignatureShare<S> {
    /// Serialize: [index:4][z:32] = 36 bytes
    pub fn to_bytes(&self) -> [u8; 36] {
        let mut buf = [0u8; 36];
        buf[0..4].copy_from_slice(&self.index.to_le_bytes());
        buf[4..36].copy_from_slice(&self.response.to_bytes());
        buf
    }

    /// Deserialize.
    pub fn from_bytes(bytes: &[u8; 36]) -> Result<Self, OsstError> {
        let index = u32::from_le_bytes(bytes[0..4].try_into().unwrap());
        if index == 0 {
            return Err(OsstError::InvalidIndex);
        }
        let resp_bytes: [u8; 32] = bytes[4..36].try_into().unwrap();
        let response =
            S::from_canonical_bytes(&resp_bytes).ok_or(OsstError::InvalidResponse)?;
        Ok(Self { index, response })
    }
}

impl<S: OsstScalar> core::fmt::Debug for SignatureShare<S> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("SignatureShare")
            .field("index", &self.index)
            .field("response", &"[REDACTED]")
            .finish()
    }
}

/// Aggregate Schnorr signature.
///
/// Verifiable with standard Schnorr: g^z == R + H(R, Y, m) · Y
///
/// Indistinguishable from a single-signer Schnorr signature.
#[derive(Clone, Debug)]
pub struct Signature<P: OsstPoint> {
    /// Group commitment.
    pub r: P,
    /// Aggregate response.
    pub z: P::Scalar,
}

impl<P: OsstPoint> Signature<P> {
    /// Serialize: [R:32][z:32] = 64 bytes
    pub fn to_bytes(&self) -> [u8; 64] {
        let mut buf = [0u8; 64];
        buf[0..32].copy_from_slice(&self.r.compress());
        buf[32..64].copy_from_slice(&self.z.to_bytes());
        buf
    }

    /// Deserialize.
    pub fn from_bytes(bytes: &[u8; 64]) -> Result<Self, OsstError> {
        let r_bytes: [u8; 32] = bytes[0..32].try_into().unwrap();
        let z_bytes: [u8; 32] = bytes[32..64].try_into().unwrap();
        let r = P::decompress(&r_bytes).ok_or(OsstError::InvalidCommitment)?;
        let z = P::Scalar::from_canonical_bytes(&z_bytes)
            .ok_or(OsstError::InvalidResponse)?;
        Ok(Self { r, z })
    }
}

// ============================================================================
// Hash functions
// ============================================================================

/// Encode all commitments for binding factor input.
///
/// Deterministic encoding sorted by index (BTreeMap guarantees this).
fn encode_commitments<P: OsstPoint>(
    commitments: &BTreeMap<u32, SigningCommitments<P>>,
) -> Vec<u8> {
    let mut buf = Vec::with_capacity(commitments.len() * 68);
    for (_, c) in commitments {
        buf.extend_from_slice(&c.index.to_le_bytes());
        buf.extend_from_slice(&c.hiding.compress());
        buf.extend_from_slice(&c.binding.compress());
    }
    buf
}

/// Binding factor: ρ_i = H("frost-binding-v1" || i || msg || commitments)
///
/// Mixes the signer's index with the full commitment list to prevent
/// adaptive commitment selection attacks.
fn compute_binding_factor<S: OsstScalar>(
    index: u32,
    message: &[u8],
    encoded_commitments: &[u8],
) -> S {
    let mut h = Sha512::new();
    h.update(b"frost-binding-v1");
    h.update(index.to_le_bytes());
    h.update((message.len() as u64).to_le_bytes());
    h.update(message);
    h.update(encoded_commitments);
    let hash: [u8; 64] = h.finalize().into();
    S::from_bytes_wide(&hash)
}

/// Schnorr challenge: c = H("frost-challenge-v1" || R || Y || m)
///
/// This is the standard Schnorr challenge computation with domain
/// separation. For zcash compatibility, replace with BLAKE2b-512
/// personalized with "Zcash_RedPallasH".
fn compute_challenge<P: OsstPoint>(
    group_commitment: &P,
    group_pubkey: &P,
    message: &[u8],
) -> P::Scalar {
    let mut h = Sha512::new();
    h.update(b"frost-challenge-v1");
    h.update(group_commitment.compress());
    h.update(group_pubkey.compress());
    h.update(message);
    let hash: [u8; 64] = h.finalize().into();
    P::Scalar::from_bytes_wide(&hash)
}

// ============================================================================
// Protocol
// ============================================================================

/// Round 1: sample nonces, return commitments for broadcast.
///
/// The returned [`Nonces`] MUST be passed to [`sign`] for exactly one
/// signing session. Store them securely until Round 2.
pub fn commit<P: OsstPoint, R: rand_core::RngCore + rand_core::CryptoRng>(
    index: u32,
    rng: &mut R,
) -> (Nonces<P::Scalar>, SigningCommitments<P>) {
    assert!(index > 0, "signer index must be 1-indexed");

    let hiding = P::Scalar::random(rng);
    let binding = P::Scalar::random(rng);

    let commitments = SigningCommitments {
        index,
        hiding: P::generator().mul_scalar(&hiding),
        binding: P::generator().mul_scalar(&binding),
    };

    (Nonces { hiding, binding }, commitments)
}

/// Round 2: produce a signature share.
///
/// Consumes the nonces to prevent reuse. Computes:
///
/// ```text
/// ρ_i = H_bind(i, m, B)
/// R   = Σ (D_j + ρ_j · E_j)
/// c   = H_sig(R, Y, m)
/// z_i = d_i + ρ_i · e_i + λ_i · c · s_i
/// ```
///
/// # Errors
///
/// Returns `InvalidIndex` if this signer's index is not in the package.
pub fn sign<P: OsstPoint>(
    package: &SigningPackage<P>,
    nonces: Nonces<P::Scalar>,
    share: &SecretShare<P::Scalar>,
    group_pubkey: &P,
) -> Result<SignatureShare<P::Scalar>, OsstError> {
    // verify our index is in the signing set
    if package.get_commitments(share.index).is_none() {
        return Err(OsstError::InvalidIndex);
    }

    // binding factor for this signer
    let rho = package.binding_factor(share.index);

    // group commitment R
    let group_commitment = package.group_commitment();

    // challenge c = H(R, Y, m)
    let challenge = package.challenge(&group_commitment, group_pubkey);

    // lagrange coefficient λ_i for this signer in the signing set
    let indices = package.signer_indices();
    let lagrange = compute_lagrange_coefficients::<P::Scalar>(&indices)?;
    let my_pos = indices
        .iter()
        .position(|&i| i == share.index)
        .ok_or(OsstError::InvalidIndex)?;
    let lambda = &lagrange[my_pos];

    // z_i = d_i + ρ_i · e_i + λ_i · c · s_i
    let response = nonces
        .hiding
        .add(&rho.mul(&nonces.binding))
        .add(&lambda.mul(&challenge).mul(share.scalar()));

    // nonces dropped here, zeroized

    Ok(SignatureShare {
        index: share.index,
        response,
    })
}

/// Aggregate signature shares into a standard Schnorr signature.
///
/// If `verifier_shares` is provided (map of index → g^{s_i}), each
/// share is verified before aggregation. This detects misbehaving
/// signers — if verification fails, the offending index is reported
/// in the error.
///
/// # Errors
///
/// - `InsufficientContributions` if fewer shares than signers in package
/// - `InvalidIndex` if a share's index is not in the signing package
/// - `InvalidResponse` if a share fails verification
pub fn aggregate<P: OsstPoint>(
    package: &SigningPackage<P>,
    shares: &[SignatureShare<P::Scalar>],
    group_pubkey: &P,
    verifier_shares: Option<&BTreeMap<u32, P>>,
) -> Result<Signature<P>, OsstError> {
    if shares.len() < package.num_signers() {
        return Err(OsstError::InsufficientContributions {
            got: shares.len(),
            need: package.num_signers(),
        });
    }

    let group_commitment = package.group_commitment();
    let challenge = package.challenge(&group_commitment, group_pubkey);

    // optionally verify each share
    if let Some(vshares) = verifier_shares {
        let indices = package.signer_indices();
        let lagrange = compute_lagrange_coefficients::<P::Scalar>(&indices)?;

        for share in shares {
            let pos = indices
                .iter()
                .position(|&i| i == share.index)
                .ok_or(OsstError::InvalidIndex)?;

            let yi = vshares
                .get(&share.index)
                .ok_or(OsstError::InvalidIndex)?;

            let rho = package.binding_factor(share.index);
            let comm = package
                .get_commitments(share.index)
                .ok_or(OsstError::InvalidIndex)?;

            // expected: g^{z_i} == D_i + ρ_i·E_i + λ_i·c·Y_i
            let lhs = P::generator().mul_scalar(&share.response);

            let rhs = comm
                .hiding
                .add(&comm.binding.mul_scalar(&rho))
                .add(&yi.mul_scalar(&lagrange[pos].mul(&challenge)));

            if lhs != rhs {
                return Err(OsstError::InvalidResponse);
            }
        }
    }

    // z = Σ z_i
    let mut z = P::Scalar::zero();
    for share in shares {
        z = z.add(&share.response);
    }

    Ok(Signature {
        r: group_commitment,
        z,
    })
}

/// Verify a standard Schnorr signature against a group public key.
///
/// Checks: g^z == R + H(R, Y, m) · Y
pub fn verify_signature<P: OsstPoint>(
    group_pubkey: &P,
    message: &[u8],
    signature: &Signature<P>,
) -> bool {
    let challenge = compute_challenge::<P>(&signature.r, group_pubkey, message);

    // lhs = g^z
    let lhs = P::generator().mul_scalar(&signature.z);

    // rhs = R + c · Y
    let rhs = signature.r.add(&group_pubkey.mul_scalar(&challenge));

    lhs == rhs
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(all(test, feature = "ristretto255"))]
mod tests {
    use super::*;
    use crate::SecretShare;
    use curve25519_dalek::{ristretto::RistrettoPoint, scalar::Scalar};
    use rand::rngs::OsRng;

    fn shamir_split(secret: &Scalar, n: u32, t: u32) -> Vec<SecretShare<Scalar>> {
        let mut rng = OsRng;
        let mut coeffs = vec![*secret];
        for _ in 1..t {
            coeffs.push(Scalar::random(&mut rng));
        }
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

    fn public_share(share: &SecretShare<Scalar>) -> RistrettoPoint {
        RistrettoPoint::generator().mul_scalar(share.scalar())
    }

    #[test]
    fn test_frost_basic() {
        let mut rng = OsRng;

        let secret = Scalar::random(&mut rng);
        let group_pubkey: RistrettoPoint =
            RistrettoPoint::generator().mul_scalar(&secret);

        let n = 5u32;
        let t = 3u32;
        let shares = shamir_split(&secret, n, t);
        let message = b"the signed zcash transaction goes here";

        // round 1: each signer commits
        let mut all_nonces = Vec::new();
        let mut all_commitments = Vec::new();
        for share in &shares[0..t as usize] {
            let (nonces, commitments) = commit::<RistrettoPoint, _>(share.index, &mut rng);
            all_nonces.push(nonces);
            all_commitments.push(commitments);
        }

        // build signing package
        let package =
            SigningPackage::new(message.to_vec(), all_commitments).unwrap();

        // round 2: each signer produces a share
        let mut sig_shares = Vec::new();
        for (share, nonces) in shares[0..t as usize]
            .iter()
            .zip(all_nonces.into_iter())
        {
            let sig_share =
                sign::<RistrettoPoint>(&package, nonces, share, &group_pubkey)
                    .unwrap();
            sig_shares.push(sig_share);
        }

        // aggregate without share verification
        let signature = aggregate::<RistrettoPoint>(
            &package,
            &sig_shares,
            &group_pubkey,
            None,
        )
        .unwrap();

        // verify
        assert!(
            verify_signature(&group_pubkey, message, &signature),
            "FROST signature should verify"
        );
    }

    #[test]
    fn test_frost_with_share_verification() {
        let mut rng = OsRng;

        let secret = Scalar::random(&mut rng);
        let group_pubkey: RistrettoPoint =
            RistrettoPoint::generator().mul_scalar(&secret);

        let n = 7u32;
        let t = 4u32;
        let shares = shamir_split(&secret, n, t);
        let message = b"withdrawal tx bytes";

        // build verifier share map
        let mut vshares = BTreeMap::new();
        for s in &shares {
            vshares.insert(s.index, public_share(s));
        }

        // use non-consecutive signers: 1, 3, 5, 7
        let active: Vec<&SecretShare<Scalar>> =
            vec![&shares[0], &shares[2], &shares[4], &shares[6]];

        // round 1
        let mut nonces_vec = Vec::new();
        let mut commitments_vec = Vec::new();
        for s in &active {
            let (n, c) = commit::<RistrettoPoint, _>(s.index, &mut rng);
            nonces_vec.push(n);
            commitments_vec.push(c);
        }

        let package =
            SigningPackage::new(message.to_vec(), commitments_vec).unwrap();

        // round 2
        let mut sig_shares = Vec::new();
        for (s, nonces) in active.iter().zip(nonces_vec.into_iter()) {
            sig_shares.push(
                sign::<RistrettoPoint>(&package, nonces, s, &group_pubkey)
                    .unwrap(),
            );
        }

        // aggregate with verification
        let signature = aggregate::<RistrettoPoint>(
            &package,
            &sig_shares,
            &group_pubkey,
            Some(&vshares),
        )
        .unwrap();

        assert!(verify_signature(&group_pubkey, message, &signature));
    }

    #[test]
    fn test_frost_wrong_message_fails() {
        let mut rng = OsRng;

        let secret = Scalar::random(&mut rng);
        let group_pubkey: RistrettoPoint =
            RistrettoPoint::generator().mul_scalar(&secret);

        let shares = shamir_split(&secret, 5, 3);
        let message = b"correct message";

        let mut nonces_vec = Vec::new();
        let mut commitments_vec = Vec::new();
        for s in &shares[0..3] {
            let (n, c) = commit::<RistrettoPoint, _>(s.index, &mut rng);
            nonces_vec.push(n);
            commitments_vec.push(c);
        }

        let package =
            SigningPackage::new(message.to_vec(), commitments_vec).unwrap();

        let mut sig_shares = Vec::new();
        for (s, nonces) in shares[0..3].iter().zip(nonces_vec.into_iter()) {
            sig_shares.push(
                sign::<RistrettoPoint>(&package, nonces, s, &group_pubkey)
                    .unwrap(),
            );
        }

        let signature =
            aggregate::<RistrettoPoint>(&package, &sig_shares, &group_pubkey, None)
                .unwrap();

        assert!(verify_signature(&group_pubkey, message, &signature));
        assert!(
            !verify_signature(&group_pubkey, b"wrong message", &signature),
            "wrong message should not verify"
        );
    }

    #[test]
    fn test_frost_wrong_pubkey_fails() {
        let mut rng = OsRng;

        let secret = Scalar::random(&mut rng);
        let group_pubkey: RistrettoPoint =
            RistrettoPoint::generator().mul_scalar(&secret);

        let shares = shamir_split(&secret, 5, 3);
        let message = b"test";

        let mut nonces_vec = Vec::new();
        let mut commitments_vec = Vec::new();
        for s in &shares[0..3] {
            let (n, c) = commit::<RistrettoPoint, _>(s.index, &mut rng);
            nonces_vec.push(n);
            commitments_vec.push(c);
        }

        let package =
            SigningPackage::new(message.to_vec(), commitments_vec).unwrap();

        let mut sig_shares = Vec::new();
        for (s, nonces) in shares[0..3].iter().zip(nonces_vec.into_iter()) {
            sig_shares.push(
                sign::<RistrettoPoint>(&package, nonces, s, &group_pubkey)
                    .unwrap(),
            );
        }

        let signature =
            aggregate::<RistrettoPoint>(&package, &sig_shares, &group_pubkey, None)
                .unwrap();

        let wrong_pubkey: RistrettoPoint =
            RistrettoPoint::generator().mul_scalar(&Scalar::random(&mut rng));
        assert!(!verify_signature(&wrong_pubkey, message, &signature));
    }

    #[test]
    fn test_frost_signature_serialization() {
        let mut rng = OsRng;

        let secret = Scalar::random(&mut rng);
        let group_pubkey: RistrettoPoint =
            RistrettoPoint::generator().mul_scalar(&secret);

        let shares = shamir_split(&secret, 3, 2);
        let message = b"roundtrip test";

        let mut nonces_vec = Vec::new();
        let mut commitments_vec = Vec::new();
        for s in &shares[0..2] {
            let (n, c) = commit::<RistrettoPoint, _>(s.index, &mut rng);
            nonces_vec.push(n);
            commitments_vec.push(c);
        }

        let package =
            SigningPackage::new(message.to_vec(), commitments_vec).unwrap();

        let mut sig_shares = Vec::new();
        for (s, nonces) in shares[0..2].iter().zip(nonces_vec.into_iter()) {
            sig_shares.push(
                sign::<RistrettoPoint>(&package, nonces, s, &group_pubkey)
                    .unwrap(),
            );
        }

        let signature =
            aggregate::<RistrettoPoint>(&package, &sig_shares, &group_pubkey, None)
                .unwrap();

        let bytes = signature.to_bytes();
        let recovered =
            Signature::<RistrettoPoint>::from_bytes(&bytes).unwrap();

        assert!(verify_signature(&group_pubkey, message, &recovered));
    }

    #[test]
    fn test_frost_bad_share_detected() {
        let mut rng = OsRng;

        let secret = Scalar::random(&mut rng);
        let group_pubkey: RistrettoPoint =
            RistrettoPoint::generator().mul_scalar(&secret);

        let shares = shamir_split(&secret, 5, 3);

        let mut vshares = BTreeMap::new();
        for s in &shares {
            vshares.insert(s.index, public_share(s));
        }

        let message = b"detect misbehaver";

        let mut nonces_vec = Vec::new();
        let mut commitments_vec = Vec::new();
        for s in &shares[0..3] {
            let (n, c) = commit::<RistrettoPoint, _>(s.index, &mut rng);
            nonces_vec.push(n);
            commitments_vec.push(c);
        }

        let package =
            SigningPackage::new(message.to_vec(), commitments_vec).unwrap();

        let mut sig_shares = Vec::new();
        for (s, nonces) in shares[0..3].iter().zip(nonces_vec.into_iter()) {
            sig_shares.push(
                sign::<RistrettoPoint>(&package, nonces, s, &group_pubkey)
                    .unwrap(),
            );
        }

        // tamper with one share
        sig_shares[1] = SignatureShare {
            index: shares[1].index,
            response: Scalar::random(&mut rng),
        };

        let result = aggregate::<RistrettoPoint>(
            &package,
            &sig_shares,
            &group_pubkey,
            Some(&vshares),
        );
        assert!(
            matches!(result, Err(OsstError::InvalidResponse)),
            "tampered share should be detected"
        );
    }

    #[test]
    fn test_frost_duplicate_commitments_rejected() {
        let mut rng = OsRng;
        let (_, c1) = commit::<RistrettoPoint, _>(1, &mut rng);
        let (_, c2) = commit::<RistrettoPoint, _>(1, &mut rng); // same index
        let result =
            SigningPackage::<RistrettoPoint>::new(b"test".to_vec(), vec![c1, c2]);
        assert!(matches!(result, Err(OsstError::DuplicateIndex(1))));
    }
}

#[cfg(all(test, feature = "pallas"))]
mod pallas_tests {
    use super::*;
    use crate::SecretShare;
    use alloc::collections::BTreeMap;
    use pasta_curves::group::ff::Field;
    use pasta_curves::pallas::{Point, Scalar};
    use rand::rngs::OsRng;

    fn shamir_split(secret: &Scalar, n: u32, t: u32) -> Vec<SecretShare<Scalar>> {
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
    fn test_pallas_frost() {
        let mut rng = OsRng;

        let secret = <Scalar as Field>::random(&mut rng);
        let group_pubkey: Point = Point::generator().mul_scalar(&secret);

        let n = 5u32;
        let t = 3u32;
        let shares = shamir_split(&secret, n, t);
        let message = b"pallas frost withdrawal";

        // round 1
        let mut nonces_vec = Vec::new();
        let mut commitments_vec = Vec::new();
        for s in &shares[0..t as usize] {
            let (nonces, commitments) = commit::<Point, _>(s.index, &mut rng);
            nonces_vec.push(nonces);
            commitments_vec.push(commitments);
        }

        let package =
            SigningPackage::new(message.to_vec(), commitments_vec).unwrap();

        // round 2
        let mut sig_shares = Vec::new();
        for (s, nonces) in shares[0..t as usize]
            .iter()
            .zip(nonces_vec.into_iter())
        {
            sig_shares.push(
                sign::<Point>(&package, nonces, s, &group_pubkey).unwrap(),
            );
        }

        let signature =
            aggregate::<Point>(&package, &sig_shares, &group_pubkey, None)
                .unwrap();

        assert!(verify_signature(&group_pubkey, message, &signature));
    }

    #[test]
    fn test_pallas_frost_with_dkg() {
        use crate::dkg;

        let mut rng = OsRng;
        let n = 5u32;
        let t = 3u32;

        // DKG
        let dealers: Vec<dkg::Dealer<Point>> =
            (1..=n).map(|i| dkg::Dealer::new(i, t, &mut rng)).collect();

        let commitments: Vec<&crate::reshare::DealerCommitment<Point>> =
            dealers.iter().map(|d| d.commitment()).collect();

        let mut secret_shares = Vec::new();
        let mut group_key = None;
        let mut vshares = BTreeMap::new();

        for j in 1..=n {
            let mut agg: dkg::Aggregator<Point> = dkg::Aggregator::new(j);
            for dealer in &dealers {
                let subshare = dealer.generate_subshare(j);
                agg.add_subshare(subshare, commitments[(dealer.index() - 1) as usize])
                    .unwrap();
            }
            let share_scalar = agg.finalize(n).unwrap();
            if group_key.is_none() {
                group_key = Some(agg.derive_group_key());
            }
            let ss = SecretShare::new(j, share_scalar);
            vshares.insert(j, Point::generator().mul_scalar(ss.scalar()));
            secret_shares.push(ss);
        }

        let group_key = group_key.unwrap();
        let message = b"dkg + frost integration test";

        // FROST sign with 3 of 5
        let active = &secret_shares[0..t as usize];

        let mut nonces_vec = Vec::new();
        let mut commitments_vec = Vec::new();
        for s in active {
            let (n, c) = commit::<Point, _>(s.index, &mut rng);
            nonces_vec.push(n);
            commitments_vec.push(c);
        }

        let package =
            SigningPackage::new(message.to_vec(), commitments_vec).unwrap();

        let mut sig_shares = Vec::new();
        for (s, nonces) in active.iter().zip(nonces_vec.into_iter()) {
            sig_shares.push(
                sign::<Point>(&package, nonces, s, &group_key).unwrap(),
            );
        }

        // aggregate with share verification
        let signature = aggregate::<Point>(
            &package,
            &sig_shares,
            &group_key,
            Some(&vshares),
        )
        .unwrap();

        assert!(
            verify_signature(&group_key, message, &signature),
            "DKG + FROST should produce valid signature"
        );
    }
}
