//! Distributed key generation (Feldman VSS)
//!
//! Trustless DKG where every participant acts as a dealer:
//! each generates a random polynomial, publishes commitments,
//! and sends sub-shares to all other participants.
//!
//! The group secret `s = sum(f_i(0))` is never known to anyone.
//! The group public key `Y = sum(g^{f_i(0)})` is publicly derivable.
//!
//! # Differences from reshare
//!
//! - Reshare: subset of dealers (t_old), Lagrange aggregation, group key invariant
//! - DKG: all n participants deal, direct summation, group key derived fresh
//!
//! # Protocol
//!
//! 1. Each participant i generates random polynomial f_i of degree t-1
//! 2. Each publishes commitment C_i = [g^{f_i(0)}, g^{f_i(1)}, ...]
//! 3. Each sends sub-share f_i(j) to participant j (encrypted)
//! 4. Participant j verifies each sub-share against commitments
//! 5. Participant j's final share: s_j = sum_i(f_i(j))
//! 6. Group public key: Y = sum_i(C_{i,0}) = g^{sum_i(f_i(0))}

use alloc::vec;
use alloc::vec::Vec;
use core::marker::PhantomData;

use crate::curve::{OsstPoint, OsstScalar};
use crate::error::OsstError;
use crate::reshare::{DealerCommitment, SubShare};

// ============================================================================
// Dealer
// ============================================================================

/// DKG dealer: generates random polynomial and sub-shares.
///
/// Unlike reshare::Dealer, the constant term is random (not an existing share).
pub struct Dealer<P: OsstPoint> {
    index: u32,
    polynomial: Vec<P::Scalar>,
    commitment: DealerCommitment<P>,
}

impl<P: OsstPoint> Drop for Dealer<P> {
    fn drop(&mut self) {
        for coeff in &mut self.polynomial {
            coeff.zeroize();
        }
    }
}

impl<P: OsstPoint> Dealer<P> {
    /// Create a new DKG dealer with a random secret.
    pub fn new<R: rand_core::RngCore + rand_core::CryptoRng>(
        index: u32,
        threshold: u32,
        rng: &mut R,
    ) -> Self {
        assert!(index > 0, "index must be 1-indexed");
        assert!(threshold > 0, "threshold must be positive");

        let mut polynomial = Vec::with_capacity(threshold as usize);
        for _ in 0..threshold {
            polynomial.push(P::Scalar::random(rng));
        }

        let commitment = DealerCommitment::from_polynomial(index, &polynomial);

        Self {
            index,
            polynomial,
            commitment,
        }
    }

    #[inline]
    pub fn index(&self) -> u32 {
        self.index
    }

    #[inline]
    pub fn commitment(&self) -> &DealerCommitment<P> {
        &self.commitment
    }

    /// Generate sub-share for player j: f_i(j)
    pub fn generate_subshare(&self, player_index: u32) -> SubShare<P::Scalar> {
        assert!(player_index > 0, "player_index must be 1-indexed");

        let j = P::Scalar::from_u32(player_index);

        // Horner's method
        let mut result = P::Scalar::zero();
        for coeff in self.polynomial.iter().rev() {
            result = result.mul(&j);
            result = result.add(coeff);
        }

        SubShare::new(self.index, player_index, result)
    }

    /// Generate sub-shares for all players 1..=n
    pub fn generate_subshares(&self, num_players: u32) -> Vec<SubShare<P::Scalar>> {
        (1..=num_players)
            .map(|j| self.generate_subshare(j))
            .collect()
    }
}

impl<P: OsstPoint> core::fmt::Debug for Dealer<P> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("dkg::Dealer")
            .field("index", &self.index)
            .field("polynomial", &"[REDACTED]")
            .field("commitment", &self.commitment)
            .finish()
    }
}

// ============================================================================
// Aggregator
// ============================================================================

/// DKG aggregator: collects sub-shares from all dealers, sums directly.
///
/// Unlike reshare::Aggregator, no Lagrange coefficients needed —
/// every participant deals, so the final share is a plain sum.
pub struct Aggregator<P: OsstPoint> {
    player_index: u32,
    /// Verified sub-share values keyed by dealer index
    subshares: Vec<(u32, P::Scalar)>,
    /// Constant-term commitments for group key derivation
    constant_commitments: Vec<P>,
    _marker: PhantomData<P>,
}

impl<P: OsstPoint> Aggregator<P> {
    pub fn new(player_index: u32) -> Self {
        assert!(player_index > 0, "player_index must be 1-indexed");
        Self {
            player_index,
            subshares: Vec::new(),
            constant_commitments: Vec::new(),
            _marker: PhantomData,
        }
    }

    #[inline]
    pub fn player_index(&self) -> u32 {
        self.player_index
    }

    #[inline]
    pub fn count(&self) -> usize {
        self.subshares.len()
    }

    /// Add a verified sub-share. Returns Ok(true) if added, Ok(false) if duplicate.
    pub fn add_subshare(
        &mut self,
        subshare: SubShare<P::Scalar>,
        commitment: &DealerCommitment<P>,
    ) -> Result<bool, OsstError> {
        if subshare.player_index != self.player_index {
            return Err(OsstError::InvalidIndex);
        }
        if subshare.dealer_index != commitment.dealer_index {
            return Err(OsstError::InvalidIndex);
        }
        if subshare.dealer_index == 0 {
            return Err(OsstError::InvalidIndex);
        }

        // duplicate check
        if self
            .subshares
            .iter()
            .any(|(idx, _)| *idx == subshare.dealer_index)
        {
            return Ok(false);
        }

        // verify sub-share against commitment
        if !commitment.verify_subshare(self.player_index, subshare.value()) {
            return Err(OsstError::InvalidResponse);
        }

        self.subshares
            .push((subshare.dealer_index, subshare.value().clone()));
        self.constant_commitments
            .push(commitment.share_commitment().clone());

        Ok(true)
    }

    /// Derive group public key: Y = sum(C_{i,0})
    pub fn derive_group_key(&self) -> P {
        let mut key = P::identity();
        for c0 in &self.constant_commitments {
            key = key.add(c0);
        }
        key
    }

    /// Aggregate final share: s_j = sum_i(f_i(j))
    pub fn finalize(&self, num_dealers: u32) -> Result<P::Scalar, OsstError> {
        if (self.subshares.len() as u32) < num_dealers {
            return Err(OsstError::InsufficientContributions {
                got: self.subshares.len(),
                need: num_dealers as usize,
            });
        }

        let mut share = P::Scalar::zero();
        for (_, value) in &self.subshares {
            share = share.add(value);
        }

        Ok(share)
    }
}

impl<P: OsstPoint> core::fmt::Debug for Aggregator<P> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("dkg::Aggregator")
            .field("player_index", &self.player_index)
            .field("count", &self.subshares.len())
            .finish()
    }
}

// ============================================================================
// On-chain coordination
// ============================================================================

/// DKG round state for on-chain coordination.
///
/// Tracks commitments from all participants. Once all n commitments are in,
/// players can verify sub-shares and derive their final shares.
#[derive(Clone, Debug)]
pub struct DkgState<P: OsstPoint> {
    /// Epoch being generated
    pub epoch: u64,
    /// Threshold for the new key
    pub threshold: u32,
    /// Total number of participants (all are dealers)
    pub num_participants: u32,
    /// Collected commitments (indexed by dealer_index - 1)
    pub commitments: Vec<Option<DealerCommitment<P>>>,
}

impl<P: OsstPoint> DkgState<P> {
    pub fn new(epoch: u64, threshold: u32, num_participants: u32) -> Self {
        Self {
            epoch,
            threshold,
            num_participants,
            commitments: vec![None; num_participants as usize],
        }
    }

    /// Submit a dealer's commitment. Returns true if new, false if duplicate.
    pub fn submit_commitment(
        &mut self,
        commitment: DealerCommitment<P>,
    ) -> Result<bool, OsstError> {
        let idx = commitment
            .dealer_index
            .checked_sub(1)
            .ok_or(OsstError::InvalidIndex)? as usize;

        if idx >= self.commitments.len() {
            return Err(OsstError::InvalidIndex);
        }

        if commitment.threshold() != self.threshold {
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

    /// True when all participants have submitted commitments
    pub fn is_complete(&self) -> bool {
        self.commitment_count() == self.num_participants as usize
    }

    /// Derive group public key from all commitments: Y = sum(C_{i,0})
    pub fn derive_group_key(&self) -> Result<P, OsstError> {
        if !self.is_complete() {
            return Err(OsstError::InsufficientContributions {
                got: self.commitment_count(),
                need: self.num_participants as usize,
            });
        }

        let mut key = P::identity();
        for commitment in self.commitments.iter().flatten() {
            key = key.add(commitment.share_commitment());
        }

        Ok(key)
    }

    /// Derive public verification share for player j.
    ///
    /// Y_j = g^{s_j} = Σ_i C_i.evaluate_at(j)
    ///
    /// These are needed for FROST share verification — detecting which
    /// signer produced a bad signature share without revealing secrets.
    pub fn derive_verification_share(&self, player_index: u32) -> Result<P, OsstError> {
        if player_index == 0 {
            return Err(OsstError::InvalidIndex);
        }
        if !self.is_complete() {
            return Err(OsstError::InsufficientContributions {
                got: self.commitment_count(),
                need: self.num_participants as usize,
            });
        }

        let mut vshare = P::identity();
        for commitment in self.commitments.iter().flatten() {
            vshare = vshare.add(&commitment.evaluate_at(player_index));
        }

        Ok(vshare)
    }

    /// Derive all verification shares for players 1..=num_participants.
    ///
    /// Returns a BTreeMap suitable for passing to [`frost::aggregate`].
    pub fn derive_all_verification_shares(
        &self,
    ) -> Result<alloc::collections::BTreeMap<u32, P>, OsstError> {
        let mut map = alloc::collections::BTreeMap::new();
        for j in 1..=self.num_participants {
            map.insert(j, self.derive_verification_share(j)?);
        }
        Ok(map)
    }

    /// Get all submitted commitments
    pub fn get_commitments(&self) -> Vec<&DealerCommitment<P>> {
        self.commitments.iter().filter_map(|c| c.as_ref()).collect()
    }
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(all(test, feature = "ristretto255"))]
mod tests {
    use super::*;
    use crate::{verify, Contribution, SecretShare};
    use curve25519_dalek::{ristretto::RistrettoPoint, scalar::Scalar};
    use rand::rngs::OsRng;

    #[test]
    fn test_basic_dkg() {
        let mut rng = OsRng;
        let n = 5u32;
        let t = 3u32;

        // phase 1: each participant creates a dealer
        let dealers: Vec<Dealer<RistrettoPoint>> =
            (1..=n).map(|i| Dealer::new(i, t, &mut rng)).collect();

        // phase 2: collect commitments
        let commitments: Vec<&DealerCommitment<RistrettoPoint>> =
            dealers.iter().map(|d| d.commitment()).collect();

        // phase 3: each player collects sub-shares from all dealers
        let mut shares = Vec::new();
        for j in 1..=n {
            let mut agg: Aggregator<RistrettoPoint> = Aggregator::new(j);
            for dealer in &dealers {
                let subshare = dealer.generate_subshare(j);
                agg.add_subshare(subshare, commitments[(dealer.index() - 1) as usize])
                    .unwrap();
            }
            let share = agg.finalize(n).unwrap();
            let group_key = agg.derive_group_key();
            shares.push((share, group_key));
        }

        // all players should derive the same group key
        let group_key = shares[0].1;
        for (_, gk) in &shares {
            assert_eq!(*gk, group_key);
        }

        // verify: t shares should produce valid OSST proof
        let secret_shares: Vec<SecretShare<Scalar>> = shares
            .iter()
            .enumerate()
            .map(|(i, (s, _))| SecretShare::new((i + 1) as u32, *s))
            .collect();

        let payload = b"dkg test verification";
        let contributions: Vec<Contribution<RistrettoPoint>> = secret_shares[0..t as usize]
            .iter()
            .map(|s| s.contribute(&mut rng, payload))
            .collect();

        let valid = verify(&group_key, &contributions, t, payload).unwrap();
        assert!(valid, "OSST verification with DKG shares should succeed");
    }

    #[test]
    fn test_dkg_state() {
        let mut rng = OsRng;
        let n = 5u32;
        let t = 3u32;

        let dealers: Vec<Dealer<RistrettoPoint>> =
            (1..=n).map(|i| Dealer::new(i, t, &mut rng)).collect();

        let mut state: DkgState<RistrettoPoint> = DkgState::new(1, t, n);

        assert!(!state.is_complete());

        for dealer in &dealers {
            state
                .submit_commitment(dealer.commitment().clone())
                .unwrap();
        }

        assert!(state.is_complete());
        assert_eq!(state.commitment_count(), n as usize);

        // derive group key from state
        let state_key = state.derive_group_key().unwrap();

        // derive group key from aggregator
        let mut agg: Aggregator<RistrettoPoint> = Aggregator::new(1);
        for dealer in &dealers {
            let subshare = dealer.generate_subshare(1);
            agg.add_subshare(subshare, dealer.commitment()).unwrap();
        }
        let agg_key = agg.derive_group_key();

        assert_eq!(state_key, agg_key);
    }

    #[test]
    fn test_dkg_bad_subshare_rejected() {
        let mut rng = OsRng;
        let n = 3u32;
        let t = 2u32;

        let dealers: Vec<Dealer<RistrettoPoint>> =
            (1..=n).map(|i| Dealer::new(i, t, &mut rng)).collect();

        let mut agg: Aggregator<RistrettoPoint> = Aggregator::new(1);

        // good sub-share
        let subshare = dealers[0].generate_subshare(1);
        assert!(agg.add_subshare(subshare, dealers[0].commitment()).is_ok());

        // tampered sub-share (wrong value)
        let bad = SubShare::new(2, 1, Scalar::random(&mut rng));
        let result = agg.add_subshare(bad, dealers[1].commitment());
        assert!(matches!(result, Err(OsstError::InvalidResponse)));
    }

    #[test]
    fn test_dkg_duplicate_rejected() {
        let mut rng = OsRng;
        let dealer: Dealer<RistrettoPoint> = Dealer::new(1, 2, &mut rng);

        let mut agg: Aggregator<RistrettoPoint> = Aggregator::new(1);
        let subshare = dealer.generate_subshare(1);
        assert!(agg.add_subshare(subshare, dealer.commitment()).unwrap());

        let subshare2 = dealer.generate_subshare(1);
        assert!(!agg.add_subshare(subshare2, dealer.commitment()).unwrap());
    }

    #[test]
    fn test_dkg_non_consecutive_subset_verifies() {
        let mut rng = OsRng;
        let n = 7u32;
        let t = 4u32;

        let dealers: Vec<Dealer<RistrettoPoint>> =
            (1..=n).map(|i| Dealer::new(i, t, &mut rng)).collect();

        let commitments: Vec<&DealerCommitment<RistrettoPoint>> =
            dealers.iter().map(|d| d.commitment()).collect();

        // collect shares for all players
        let mut secret_shares = Vec::new();
        let mut group_key = None;
        for j in 1..=n {
            let mut agg: Aggregator<RistrettoPoint> = Aggregator::new(j);
            for dealer in &dealers {
                let subshare = dealer.generate_subshare(j);
                agg.add_subshare(subshare, commitments[(dealer.index() - 1) as usize])
                    .unwrap();
            }
            if group_key.is_none() {
                group_key = Some(agg.derive_group_key());
            }
            secret_shares.push(SecretShare::new(j, agg.finalize(n).unwrap()));
        }

        let group_key = group_key.unwrap();
        let payload = b"non-consecutive subset test";

        // use shares 1, 3, 5, 7 (non-consecutive, at threshold)
        let contributions: Vec<Contribution<RistrettoPoint>> = [0, 2, 4, 6]
            .iter()
            .map(|&i| secret_shares[i].contribute(&mut rng, payload))
            .collect();

        assert!(verify(&group_key, &contributions, t, payload).unwrap());
    }
}

#[cfg(all(test, feature = "pallas"))]
mod pallas_tests {
    use super::*;
    use crate::{verify, Contribution, SecretShare};
    use pasta_curves::pallas::Point;
    use rand::rngs::OsRng;

    #[test]
    fn test_pallas_dkg() {
        let mut rng = OsRng;
        let n = 5u32;
        let t = 3u32;

        let dealers: Vec<Dealer<Point>> =
            (1..=n).map(|i| Dealer::new(i, t, &mut rng)).collect();

        let commitments: Vec<&DealerCommitment<Point>> =
            dealers.iter().map(|d| d.commitment()).collect();

        let mut shares = Vec::new();
        let mut group_key = None;
        for j in 1..=n {
            let mut agg: Aggregator<Point> = Aggregator::new(j);
            for dealer in &dealers {
                let subshare = dealer.generate_subshare(j);
                agg.add_subshare(subshare, commitments[(dealer.index() - 1) as usize])
                    .unwrap();
            }
            if group_key.is_none() {
                group_key = Some(agg.derive_group_key());
            }
            shares.push(SecretShare::new(j, agg.finalize(n).unwrap()));
        }

        let group_key = group_key.unwrap();
        let payload = b"pallas dkg test";

        let contributions: Vec<Contribution<Point>> = shares[0..t as usize]
            .iter()
            .map(|s| s.contribute(&mut rng, payload))
            .collect();

        assert!(verify(&group_key, &contributions, t, payload).unwrap());
    }
}
