//! Proactive secret sharing reshare protocol
//!
//! Allows transitioning threshold shares from one custodian set to another
//! without revealing the secret or changing the group public key.
//!
//! # Security Model
//!
//! - Assumes honest majority among dealers (t_old honest of n_old)
//! - Sub-shares must be encrypted in transit (not handled here)
//! - Commitments provide public verifiability
//! - Group public key Y = g^s is an invariant across reshares
//!
//! # Scalability
//!
//! Designed for O(1000) participants:
//! - Commitments: O(t) points per dealer, posted to chain
//! - Sub-shares: Encrypted, can be batched or posted to chain
//! - Verification: Batched for efficiency
//! - Aggregation: O(t) operations per player, parallelizable
//!
//! # Protocol Phases
//!
//! 1. **Dealing**: Dealers create polynomials, publish commitments
//! 2. **Distribution**: Sub-shares sent (encrypted) to players
//! 3. **Verification**: Players verify against commitments
//! 4. **Aggregation**: Players combine t_old sub-shares into new share

use alloc::vec;
use alloc::vec::Vec;
use core::marker::PhantomData;

use crate::curve::{OsstPoint, OsstScalar};
use crate::error::OsstError;
use crate::lagrange::compute_lagrange_coefficients;

// ============================================================================
// Core Types
// ============================================================================

/// Compressed dealer commitment for on-chain storage
///
/// Only stores the commitment points, index derived from position.
/// Size: 32 * threshold bytes per dealer
#[derive(Clone, Debug)]
pub struct DealerCommitment<P: OsstPoint> {
    /// Dealer's index in the old custodian set (1-indexed)
    pub dealer_index: u32,
    /// Polynomial commitments [g^{a_0}, g^{a_1}, ..., g^{a_{t-1}}]
    /// where a_0 = dealer's share
    pub coefficients: Vec<P>,
}

impl<P: OsstPoint> DealerCommitment<P> {
    /// Create commitment from polynomial coefficients
    pub fn from_polynomial(dealer_index: u32, coefficients: &[P::Scalar]) -> Self {
        debug_assert!(dealer_index > 0);
        debug_assert!(!coefficients.is_empty());

        let committed: Vec<P> = coefficients
            .iter()
            .map(|a| P::generator().mul_scalar(a))
            .collect();

        Self {
            dealer_index,
            coefficients: committed,
        }
    }

    /// Threshold (degree + 1) of the committed polynomial
    #[inline]
    pub fn threshold(&self) -> u32 {
        self.coefficients.len() as u32
    }

    /// Commitment to dealer's share: C_0 = g^{s_i}
    #[inline]
    pub fn share_commitment(&self) -> &P {
        &self.coefficients[0]
    }

    /// Evaluate commitment at player index j
    ///
    /// Returns g^{f(j)} = Π_{k=0}^{t-1} C_k^{j^k}
    ///
    /// Uses Horner's method for efficiency: O(t) scalar muls
    pub fn evaluate_at(&self, player_index: u32) -> P {
        debug_assert!(player_index > 0);

        let j = P::Scalar::from_u32(player_index);

        // Horner's method: ((C_{t-1} * j + C_{t-2}) * j + ...) * j + C_0
        let mut result = P::identity();
        for coeff in self.coefficients.iter().rev() {
            result = result.mul_scalar(&j);
            result = result.add(coeff);
        }
        result
    }

    /// Verify a sub-share against this commitment
    ///
    /// Checks: g^{sub_share} == g^{f(j)}
    #[inline]
    pub fn verify_subshare(&self, player_index: u32, sub_share: &P::Scalar) -> bool {
        if player_index == 0 {
            return false;
        }

        let expected = self.evaluate_at(player_index);
        let actual = P::generator().mul_scalar(sub_share);

        // Constant-time comparison via point equality
        actual == expected
    }

    /// Compressed byte size
    #[inline]
    pub fn byte_size(&self) -> usize {
        4 + self.coefficients.len() * 32
    }

    /// Serialize to bytes (for on-chain storage)
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut buf = Vec::with_capacity(self.byte_size());
        buf.extend_from_slice(&self.dealer_index.to_le_bytes());
        for c in &self.coefficients {
            buf.extend_from_slice(&c.compress());
        }
        buf
    }

    /// Deserialize from bytes
    pub fn from_bytes(bytes: &[u8], threshold: u32) -> Result<Self, OsstError> {
        let expected_len = 4 + (threshold as usize) * 32;
        if bytes.len() != expected_len {
            return Err(OsstError::InvalidCommitment);
        }

        let dealer_index = u32::from_le_bytes(bytes[0..4].try_into().unwrap());
        if dealer_index == 0 {
            return Err(OsstError::InvalidIndex);
        }

        let mut coefficients = Vec::with_capacity(threshold as usize);
        for i in 0..threshold as usize {
            let offset = 4 + i * 32;
            let point_bytes: [u8; 32] = bytes[offset..offset + 32].try_into().unwrap();
            let point = P::decompress(&point_bytes).ok_or(OsstError::InvalidCommitment)?;
            coefficients.push(point);
        }

        Ok(Self { dealer_index, coefficients })
    }
}

/// Sub-share from dealer to player
///
/// Should be encrypted before transmission. Size: 40 bytes
#[derive(Clone)]
pub struct SubShare<S: OsstScalar> {
    pub dealer_index: u32,
    pub player_index: u32,
    pub value: S,
}

impl<S: OsstScalar> SubShare<S> {
    #[inline]
    pub fn new(dealer_index: u32, player_index: u32, value: S) -> Self {
        debug_assert!(dealer_index > 0);
        debug_assert!(player_index > 0);
        Self { dealer_index, player_index, value }
    }

    pub fn to_bytes(&self) -> [u8; 40] {
        let mut buf = [0u8; 40];
        buf[0..4].copy_from_slice(&self.dealer_index.to_le_bytes());
        buf[4..8].copy_from_slice(&self.player_index.to_le_bytes());
        buf[8..40].copy_from_slice(&self.value.to_bytes());
        buf
    }

    pub fn from_bytes(bytes: &[u8; 40]) -> Result<Self, OsstError> {
        let dealer_index = u32::from_le_bytes(bytes[0..4].try_into().unwrap());
        let player_index = u32::from_le_bytes(bytes[4..8].try_into().unwrap());

        if dealer_index == 0 || player_index == 0 {
            return Err(OsstError::InvalidIndex);
        }

        let value_bytes: [u8; 32] = bytes[8..40].try_into().unwrap();
        let value = S::from_canonical_bytes(&value_bytes).ok_or(OsstError::InvalidResponse)?;

        Ok(Self { dealer_index, player_index, value })
    }
}

// Prevent Debug from leaking secrets
impl<S: OsstScalar> core::fmt::Debug for SubShare<S> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("SubShare")
            .field("dealer_index", &self.dealer_index)
            .field("player_index", &self.player_index)
            .field("value", &"[REDACTED]")
            .finish()
    }
}

// ============================================================================
// Dealer (Old Custodian)
// ============================================================================

/// Dealer generates sub-shares for new custodians
///
/// Holds secret polynomial, should be zeroized after use.
pub struct Dealer<P: OsstPoint> {
    index: u32,
    /// Polynomial coefficients [a_0=share, a_1, ..., a_{t-1}]
    polynomial: Vec<P::Scalar>,
    /// Cached commitment
    commitment: DealerCommitment<P>,
}

impl<P: OsstPoint> Dealer<P> {
    /// Create dealer from existing share
    ///
    /// Generates random polynomial with share as constant term.
    pub fn new<R: rand_core::RngCore + rand_core::CryptoRng>(
        index: u32,
        share: P::Scalar,
        new_threshold: u32,
        rng: &mut R,
    ) -> Self {
        assert!(index > 0, "dealer index must be 1-indexed");
        assert!(new_threshold > 0, "threshold must be positive");

        let mut polynomial = Vec::with_capacity(new_threshold as usize);
        polynomial.push(share);

        for _ in 1..new_threshold {
            polynomial.push(P::Scalar::random(rng));
        }

        let commitment = DealerCommitment::from_polynomial(index, &polynomial);

        Self { index, polynomial, commitment }
    }

    #[inline]
    pub fn index(&self) -> u32 {
        self.index
    }

    #[inline]
    pub fn commitment(&self) -> &DealerCommitment<P> {
        &self.commitment
    }

    /// Generate sub-share for a specific player
    ///
    /// Evaluates polynomial at player's index using Horner's method.
    pub fn generate_subshare(&self, player_index: u32) -> SubShare<P::Scalar> {
        debug_assert!(player_index > 0);

        let j = P::Scalar::from_u32(player_index);

        // Horner's method for polynomial evaluation
        let mut result = P::Scalar::zero();
        for coeff in self.polynomial.iter().rev() {
            result = result.mul(&j);
            result = result.add(coeff);
        }

        SubShare::new(self.index, player_index, result)
    }

    /// Generate all sub-shares for a player range
    ///
    /// Returns sub-shares for players 1..=num_players
    pub fn generate_subshares(&self, num_players: u32) -> Vec<SubShare<P::Scalar>> {
        (1..=num_players)
            .map(|j| self.generate_subshare(j))
            .collect()
    }
}

// Prevent Debug from leaking polynomial
impl<P: OsstPoint> core::fmt::Debug for Dealer<P> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("Dealer")
            .field("index", &self.index)
            .field("polynomial", &"[REDACTED]")
            .field("commitment", &self.commitment)
            .finish()
    }
}

// ============================================================================
// Aggregator (New Custodian)
// ============================================================================

/// Verified sub-share with its commitment reference
struct VerifiedSubShare<S: OsstScalar> {
    dealer_index: u32,
    value: S,
}

/// Aggregator collects and combines sub-shares
///
/// Designed for async collection - sub-shares can arrive in any order.
pub struct Aggregator<P: OsstPoint> {
    player_index: u32,
    /// Verified sub-shares from dealers
    subshares: Vec<VerifiedSubShare<P::Scalar>>,
    /// Dealer commitments (for group key derivation)
    commitments: Vec<DealerCommitment<P>>,
    _marker: PhantomData<P>,
}

impl<P: OsstPoint> Aggregator<P> {
    pub fn new(player_index: u32) -> Self {
        assert!(player_index > 0, "player index must be 1-indexed");
        Self {
            player_index,
            subshares: Vec::new(),
            commitments: Vec::new(),
            _marker: PhantomData,
        }
    }

    #[inline]
    pub fn player_index(&self) -> u32 {
        self.player_index
    }

    /// Number of verified sub-shares collected
    #[inline]
    pub fn count(&self) -> usize {
        self.subshares.len()
    }

    /// Check if we have enough sub-shares
    #[inline]
    pub fn has_threshold(&self, old_threshold: u32) -> bool {
        self.subshares.len() >= old_threshold as usize
    }

    /// Add a sub-share with verification
    ///
    /// Returns Ok(true) if added, Ok(false) if duplicate, Err if invalid.
    pub fn add_subshare(
        &mut self,
        subshare: SubShare<P::Scalar>,
        commitment: DealerCommitment<P>,
    ) -> Result<bool, OsstError> {
        // Validate indices
        if subshare.player_index != self.player_index {
            return Err(OsstError::InvalidIndex);
        }
        if subshare.dealer_index != commitment.dealer_index {
            return Err(OsstError::InvalidIndex);
        }
        if subshare.dealer_index == 0 {
            return Err(OsstError::InvalidIndex);
        }

        // Check for duplicate
        if self.subshares.iter().any(|s| s.dealer_index == subshare.dealer_index) {
            return Ok(false);
        }

        // Verify sub-share against commitment
        if !commitment.verify_subshare(self.player_index, &subshare.value) {
            return Err(OsstError::InvalidResponse);
        }

        // Store
        self.subshares.push(VerifiedSubShare {
            dealer_index: subshare.dealer_index,
            value: subshare.value,
        });
        self.commitments.push(commitment);

        Ok(true)
    }

    /// Batch add sub-shares (more efficient for multiple)
    ///
    /// Verifies all, adds only valid ones. Returns count of added.
    pub fn add_subshares_batch(
        &mut self,
        subshares: Vec<SubShare<P::Scalar>>,
        commitments: Vec<DealerCommitment<P>>,
    ) -> usize {
        let mut added = 0;
        for (subshare, commitment) in subshares.into_iter().zip(commitments.into_iter()) {
            if let Ok(true) = self.add_subshare(subshare, commitment) {
                added += 1;
            }
        }
        added
    }

    /// Aggregate sub-shares into final share
    ///
    /// Computes: s'_j = Σ λ_i * σ_{i,j}
    /// where λ_i are Lagrange coefficients for dealer indices
    pub fn aggregate(&self, old_threshold: u32) -> Result<P::Scalar, OsstError> {
        if !self.has_threshold(old_threshold) {
            return Err(OsstError::InsufficientContributions {
                got: self.subshares.len(),
                need: old_threshold as usize,
            });
        }

        let dealer_indices: Vec<u32> = self.subshares
            .iter()
            .map(|s| s.dealer_index)
            .collect();

        let lagrange = compute_lagrange_coefficients::<P::Scalar>(&dealer_indices)?;

        let mut new_share = P::Scalar::zero();
        for (subshare, lambda) in self.subshares.iter().zip(lagrange.iter()) {
            let term = lambda.mul(&subshare.value);
            new_share = new_share.add(&term);
        }

        Ok(new_share)
    }

    /// Derive group public key from dealer commitments
    ///
    /// Returns: Y = Σ λ_i * C_{i,0} = g^s
    ///
    /// This should equal the original group key (invariant check).
    pub fn derive_group_key(&self, old_threshold: u32) -> Result<P, OsstError> {
        if !self.has_threshold(old_threshold) {
            return Err(OsstError::InsufficientContributions {
                got: self.commitments.len(),
                need: old_threshold as usize,
            });
        }

        let dealer_indices: Vec<u32> = self.commitments
            .iter()
            .map(|c| c.dealer_index)
            .collect();

        let lagrange = compute_lagrange_coefficients::<P::Scalar>(&dealer_indices)?;

        let mut group_key = P::identity();
        for (commitment, lambda) in self.commitments.iter().zip(lagrange.iter()) {
            let term = commitment.share_commitment().mul_scalar(lambda);
            group_key = group_key.add(&term);
        }

        Ok(group_key)
    }

    /// Finalize reshare: aggregate share and verify group key
    ///
    /// Returns (new_share, derived_group_key) if successful.
    pub fn finalize(
        &self,
        old_threshold: u32,
        expected_group_key: &P,
    ) -> Result<P::Scalar, OsstError> {
        let derived_key = self.derive_group_key(old_threshold)?;

        if &derived_key != expected_group_key {
            return Err(OsstError::InvalidCommitment);
        }

        self.aggregate(old_threshold)
    }
}

impl<P: OsstPoint> core::fmt::Debug for Aggregator<P> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("Aggregator")
            .field("player_index", &self.player_index)
            .field("count", &self.subshares.len())
            .finish()
    }
}

// ============================================================================
// On-Chain Coordination Types
// ============================================================================

/// Reshare epoch state for on-chain storage
///
/// Tracks the progress of a reshare round.
#[derive(Clone, Debug)]
pub struct ReshareState<P: OsstPoint> {
    /// Epoch number being reshared into
    pub target_epoch: u64,
    /// Old threshold (required dealers)
    pub old_threshold: u32,
    /// New threshold (for new shares)
    pub new_threshold: u32,
    /// Number of new players
    pub new_player_count: u32,
    /// Collected dealer commitments (indexed by dealer_index - 1)
    pub commitments: Vec<Option<DealerCommitment<P>>>,
    /// Expected group public key (invariant)
    pub group_key: P,
}

impl<P: OsstPoint> ReshareState<P> {
    pub fn new(
        target_epoch: u64,
        old_dealer_count: u32,
        old_threshold: u32,
        new_threshold: u32,
        new_player_count: u32,
        group_key: P,
    ) -> Self {
        Self {
            target_epoch,
            old_threshold,
            new_threshold,
            new_player_count,
            commitments: vec![None; old_dealer_count as usize],
            group_key,
        }
    }

    /// Submit a dealer's commitment
    ///
    /// Returns true if this is a new commitment, false if duplicate.
    pub fn submit_commitment(&mut self, commitment: DealerCommitment<P>) -> Result<bool, OsstError> {
        let idx = commitment.dealer_index.checked_sub(1)
            .ok_or(OsstError::InvalidIndex)? as usize;

        if idx >= self.commitments.len() {
            return Err(OsstError::InvalidIndex);
        }

        if commitment.threshold() != self.new_threshold {
            return Err(OsstError::InvalidCommitment);
        }

        if self.commitments[idx].is_some() {
            return Ok(false);
        }

        self.commitments[idx] = Some(commitment);
        Ok(true)
    }

    /// Number of commitments received
    pub fn commitment_count(&self) -> usize {
        self.commitments.iter().filter(|c| c.is_some()).count()
    }

    /// Check if we have enough commitments to proceed
    pub fn has_quorum(&self) -> bool {
        self.commitment_count() >= self.old_threshold as usize
    }

    /// Get all submitted commitments
    pub fn get_commitments(&self) -> Vec<&DealerCommitment<P>> {
        self.commitments.iter().filter_map(|c| c.as_ref()).collect()
    }

    /// Verify group key from submitted commitments
    pub fn verify_group_key(&self) -> Result<bool, OsstError> {
        if !self.has_quorum() {
            return Err(OsstError::InsufficientContributions {
                got: self.commitment_count(),
                need: self.old_threshold as usize,
            });
        }

        let commitments: Vec<&DealerCommitment<P>> = self.get_commitments();
        let dealer_indices: Vec<u32> = commitments.iter().map(|c| c.dealer_index).collect();
        let lagrange = compute_lagrange_coefficients::<P::Scalar>(&dealer_indices)?;

        let mut derived_key = P::identity();
        for (commitment, lambda) in commitments.iter().zip(lagrange.iter()) {
            let term = commitment.share_commitment().mul_scalar(lambda);
            derived_key = derived_key.add(&term);
        }

        Ok(derived_key == self.group_key)
    }
}

// ============================================================================
// Batch Operations (for efficiency)
// ============================================================================

/// Batch verify multiple sub-shares against their commitments
///
/// More efficient than individual verification when verifying many.
/// Uses randomized linear combination for batch verification.
pub fn batch_verify_subshares<P: OsstPoint, R: rand_core::RngCore + rand_core::CryptoRng>(
    player_index: u32,
    subshares: &[SubShare<P::Scalar>],
    commitments: &[DealerCommitment<P>],
    rng: &mut R,
) -> bool {
    if subshares.len() != commitments.len() || subshares.is_empty() {
        return false;
    }

    // Generate random weights for linear combination
    let weights: Vec<P::Scalar> = (0..subshares.len())
        .map(|_| P::Scalar::random(rng))
        .collect();

    // LHS: g^{Σ w_i * σ_i}
    let mut lhs_exponent = P::Scalar::zero();
    for (subshare, w) in subshares.iter().zip(weights.iter()) {
        if subshare.player_index != player_index {
            return false;
        }
        let term = w.mul(&subshare.value);
        lhs_exponent = lhs_exponent.add(&term);
    }
    let lhs = P::generator().mul_scalar(&lhs_exponent);

    // RHS: Σ w_i * C_i(j)
    let mut rhs = P::identity();
    for (commitment, w) in commitments.iter().zip(weights.iter()) {
        let eval = commitment.evaluate_at(player_index);
        rhs = rhs.add(&eval.mul_scalar(w));
    }

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

    #[test]
    fn test_basic_reshare() {
        let mut rng = OsRng;

        let secret = Scalar::random(&mut rng);
        let group_pubkey: RistrettoPoint = RistrettoPoint::generator().mul_scalar(&secret);

        // Old: 5/3, New: 7/5
        let old_shares = shamir_split(&secret, 5, 3);
        let new_n = 7u32;
        let new_t = 5u32;

        // Create dealers
        let dealers: Vec<Dealer<RistrettoPoint>> = old_shares
            .iter()
            .map(|s| Dealer::new(s.index, s.scalar.clone(), new_t, &mut rng))
            .collect();

        // Create aggregators for new players
        let mut aggregators: Vec<Aggregator<RistrettoPoint>> = (1..=new_n)
            .map(Aggregator::new)
            .collect();

        // Distribute sub-shares
        for dealer in &dealers {
            let commitment = dealer.commitment().clone();
            for agg in &mut aggregators {
                let subshare = dealer.generate_subshare(agg.player_index());
                assert!(agg.add_subshare(subshare, commitment.clone()).unwrap());
            }
        }

        // Finalize and verify
        let mut new_shares = Vec::new();
        for agg in &aggregators {
            let share = agg.finalize(3, &group_pubkey).unwrap();
            new_shares.push(share);
        }

        // Verify new shares reconstruct secret
        let indices: Vec<u32> = (1..=new_t).collect();
        let lagrange = compute_lagrange_coefficients::<Scalar>(&indices).unwrap();

        let mut reconstructed = Scalar::ZERO;
        for (i, lambda) in lagrange.iter().enumerate() {
            reconstructed += lambda * new_shares[i];
        }

        assert_eq!(reconstructed, secret);
    }

    #[test]
    fn test_threshold_subset_dealers() {
        let mut rng = OsRng;

        let secret = Scalar::random(&mut rng);
        let group_pubkey: RistrettoPoint = RistrettoPoint::generator().mul_scalar(&secret);

        let old_shares = shamir_split(&secret, 5, 3);
        let new_t = 3u32;

        // Only 3 of 5 dealers participate
        let active_dealers: Vec<Dealer<RistrettoPoint>> = old_shares[0..3]
            .iter()
            .map(|s| Dealer::new(s.index, s.scalar.clone(), new_t, &mut rng))
            .collect();

        let mut agg: Aggregator<RistrettoPoint> = Aggregator::new(1);

        for dealer in &active_dealers {
            let commitment = dealer.commitment().clone();
            let subshare = dealer.generate_subshare(1);
            agg.add_subshare(subshare, commitment).unwrap();
        }

        // Should succeed with threshold dealers
        let share = agg.finalize(3, &group_pubkey).unwrap();

        // Verify public share matches
        let public_share: RistrettoPoint = RistrettoPoint::generator().mul_scalar(&share);
        let derived_key = agg.derive_group_key(3).unwrap();

        // This player's public share contributes to group key
        assert_eq!(derived_key, group_pubkey);
    }

    #[test]
    fn test_batch_verification() {
        let mut rng = OsRng;

        let secret = Scalar::random(&mut rng);
        let old_shares = shamir_split(&secret, 5, 3);

        let dealers: Vec<Dealer<RistrettoPoint>> = old_shares
            .iter()
            .map(|s| Dealer::new(s.index, s.scalar.clone(), 3, &mut rng))
            .collect();

        let player_index = 1u32;
        let subshares: Vec<SubShare<Scalar>> = dealers
            .iter()
            .map(|d| d.generate_subshare(player_index))
            .collect();
        let commitments: Vec<DealerCommitment<RistrettoPoint>> = dealers
            .iter()
            .map(|d| d.commitment().clone())
            .collect();

        // Batch verify should succeed
        assert!(batch_verify_subshares(
            player_index,
            &subshares,
            &commitments,
            &mut rng
        ));

        // Tamper with one sub-share
        let mut bad_subshares = subshares.clone();
        bad_subshares[0] = SubShare::new(1, 1, Scalar::random(&mut rng));

        // Should fail
        assert!(!batch_verify_subshares(
            player_index,
            &bad_subshares,
            &commitments,
            &mut rng
        ));
    }

    #[test]
    fn test_reshare_state() {
        let mut rng = OsRng;

        let secret = Scalar::random(&mut rng);
        let group_pubkey: RistrettoPoint = RistrettoPoint::generator().mul_scalar(&secret);

        let old_shares = shamir_split(&secret, 5, 3);

        let mut state: ReshareState<RistrettoPoint> = ReshareState::new(
            1, // epoch
            5, // old dealers
            3, // old threshold
            3, // new threshold
            5, // new players
            group_pubkey,
        );

        // Submit commitments
        for share in &old_shares {
            let dealer: Dealer<RistrettoPoint> = Dealer::new(
                share.index,
                share.scalar.clone(),
                3,
                &mut rng,
            );
            state.submit_commitment(dealer.commitment().clone()).unwrap();
        }

        assert!(state.has_quorum());
        assert!(state.verify_group_key().unwrap());
    }

    #[test]
    fn test_commitment_serialization() {
        let mut rng = OsRng;
        let dealer: Dealer<RistrettoPoint> = Dealer::new(1, Scalar::random(&mut rng), 3, &mut rng);

        let original = dealer.commitment().clone();
        let bytes = original.to_bytes();
        let recovered = DealerCommitment::<RistrettoPoint>::from_bytes(&bytes, 3).unwrap();

        assert_eq!(original.dealer_index, recovered.dealer_index);
        assert_eq!(original.coefficients.len(), recovered.coefficients.len());
        for (a, b) in original.coefficients.iter().zip(recovered.coefficients.iter()) {
            assert_eq!(a, b);
        }
    }

    #[test]
    fn test_horner_evaluation() {
        let mut rng = OsRng;
        let dealer: Dealer<RistrettoPoint> = Dealer::new(1, Scalar::random(&mut rng), 5, &mut rng);

        // Verify commitment evaluation matches sub-share
        for j in 1..=10u32 {
            let subshare = dealer.generate_subshare(j);
            let eval = dealer.commitment().evaluate_at(j);
            let expected: RistrettoPoint = RistrettoPoint::generator().mul_scalar(&subshare.value);
            assert_eq!(eval, expected);
        }
    }
}
