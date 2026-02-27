//! bft consensus for syndicate operations
//!
//! aura-style instant finality using osst threshold signatures.
//! the threshold signature itself serves as the finality proof -
//! no separate voting/commit phases needed.
//!
//! # round lifecycle
//!
//! 1. rotating proposer submits state transition
//! 2. members verify payload and generate osst contributions
//! 3. once t contributions collected, round is finalized
//! 4. the aggregated osst proof IS the finality certificate

use alloc::vec::Vec;
use sha2::{Digest, Sha256};

use osst::{Contribution, OsstError, OsstPoint, SecretShare};

/// errors during bft round execution
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RoundError {
    /// not enough contributions to finalize
    InsufficientContributions { got: usize, need: usize },
    /// osst verification failed
    VerificationFailed(OsstError),
    /// invalid round state
    InvalidState(&'static str),
    /// proposer mismatch
    WrongProposer { expected: u32, got: u32 },
    /// duplicate contribution from same member
    DuplicateContribution(u32),
}

impl From<OsstError> for RoundError {
    fn from(e: OsstError) -> Self {
        RoundError::VerificationFailed(e)
    }
}

/// a single consensus round
#[derive(Clone, Debug)]
pub struct Round<P: OsstPoint> {
    /// block height
    pub height: u64,
    /// proposer index (rotating based on height % n)
    pub proposer: u32,
    /// payload (state transition to apply)
    pub payload: Vec<u8>,
    /// collected contributions
    contributions: Vec<Contribution<P>>,
    /// round state
    state: RoundState,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RoundState {
    /// waiting for contributions
    Open,
    /// round finalized with valid proof
    Finalized,
    /// round failed (timeout, invalid proof, etc)
    Failed,
}

impl<P: OsstPoint> Round<P> {
    /// proposer creates new round with state transition payload
    pub fn propose(height: u64, proposer: u32, payload: Vec<u8>) -> Self {
        Self {
            height,
            proposer,
            payload,
            contributions: Vec::new(),
            state: RoundState::Open,
        }
    }

    /// compute the signing payload (height || proposer || payload_hash)
    pub fn signing_payload(&self) -> Vec<u8> {
        let mut hasher = Sha256::new();
        hasher.update(self.height.to_le_bytes());
        hasher.update(self.proposer.to_le_bytes());
        hasher.update(&self.payload);
        let hash: [u8; 32] = hasher.finalize().into();

        let mut buf = Vec::with_capacity(8 + 4 + 32);
        buf.extend_from_slice(&self.height.to_le_bytes());
        buf.extend_from_slice(&self.proposer.to_le_bytes());
        buf.extend_from_slice(&hash);
        buf
    }

    /// member generates contribution for this round
    pub fn contribute<R: rand_core::RngCore + rand_core::CryptoRng>(
        &self,
        share: &SecretShare<P::Scalar>,
        rng: &mut R,
    ) -> Contribution<P> {
        let payload = self.signing_payload();
        share.contribute(rng, &payload)
    }

    /// add contribution to round
    pub fn add_contribution(&mut self, contribution: Contribution<P>) -> Result<(), RoundError> {
        if self.state != RoundState::Open {
            return Err(RoundError::InvalidState("round not open"));
        }

        // check for duplicate
        if self.contributions.iter().any(|c| c.index == contribution.index) {
            return Err(RoundError::DuplicateContribution(contribution.index));
        }

        self.contributions.push(contribution);
        Ok(())
    }

    /// check if round can be finalized
    pub fn can_finalize(&self, threshold: u32) -> bool {
        self.state == RoundState::Open && self.contributions.len() >= threshold as usize
    }

    /// number of contributions collected
    pub fn contribution_count(&self) -> usize {
        self.contributions.len()
    }

    /// finalize round once t contributions collected
    /// verifies the aggregated osst proof and returns finalized block
    pub fn finalize(
        mut self,
        threshold: u32,
        group_pubkey: &P,
    ) -> Result<FinalizedBlock<P>, RoundError> {
        if self.state != RoundState::Open {
            return Err(RoundError::InvalidState("round not open"));
        }

        if self.contributions.len() < threshold as usize {
            return Err(RoundError::InsufficientContributions {
                got: self.contributions.len(),
                need: threshold as usize,
            });
        }

        let payload = self.signing_payload();

        // verify osst proof
        let valid = osst::verify(group_pubkey, &self.contributions, threshold, &payload)?;

        if !valid {
            self.state = RoundState::Failed;
            return Err(RoundError::InvalidState("osst verification failed"));
        }

        self.state = RoundState::Finalized;

        Ok(FinalizedBlock {
            height: self.height,
            proposer: self.proposer,
            payload: self.payload,
            contributions: self.contributions,
        })
    }
}

/// a finalized block with osst proof
#[derive(Clone, Debug)]
pub struct FinalizedBlock<P: OsstPoint> {
    /// block height
    pub height: u64,
    /// proposer who created this block
    pub proposer: u32,
    /// state transition payload
    pub payload: Vec<u8>,
    /// osst contributions that form the finality proof
    pub contributions: Vec<Contribution<P>>,
}

impl<P: OsstPoint> FinalizedBlock<P> {
    /// verify this block against group public key
    pub fn verify(&self, group_pubkey: &P, threshold: u32) -> Result<bool, OsstError> {
        let mut hasher = Sha256::new();
        hasher.update(self.height.to_le_bytes());
        hasher.update(self.proposer.to_le_bytes());
        hasher.update(&self.payload);
        let hash: [u8; 32] = hasher.finalize().into();

        let mut payload = Vec::with_capacity(8 + 4 + 32);
        payload.extend_from_slice(&self.height.to_le_bytes());
        payload.extend_from_slice(&self.proposer.to_le_bytes());
        payload.extend_from_slice(&hash);

        osst::verify(group_pubkey, &self.contributions, threshold, &payload)
    }

    /// compute block hash (for chaining)
    pub fn hash(&self) -> [u8; 32] {
        let mut hasher = Sha256::new();
        hasher.update(self.height.to_le_bytes());
        hasher.update(self.proposer.to_le_bytes());
        hasher.update(&self.payload);
        // include contribution commitments
        for c in &self.contributions {
            hasher.update(&c.index.to_le_bytes());
            hasher.update(c.commitment.compress());
        }
        hasher.finalize().into()
    }
}

/// compute proposer for given height (aura-style rotation)
pub fn proposer_for_height(height: u64, num_members: u32) -> u32 {
    // 1-indexed to match osst convention
    ((height % num_members as u64) as u32) + 1
}

#[cfg(all(test, feature = "ristretto255"))]
mod tests {
    use super::*;
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
    fn test_round_basic() {
        let mut rng = OsRng;

        // setup syndicate keys
        let secret = Scalar::random(&mut rng);
        let group_pubkey = RistrettoPoint::generator().mul_scalar(&secret);

        let n = 5u32;
        let t = 3u32;
        let shares = shamir_split(&secret, n, t);

        // create round
        let mut round: Round<RistrettoPoint> = Round::propose(
            1,
            proposer_for_height(1, n),
            b"transfer 100 to alice".to_vec(),
        );

        // members contribute
        for share in shares.iter().take(t as usize) {
            let contribution = round.contribute(share, &mut rng);
            round.add_contribution(contribution).unwrap();
        }

        assert!(round.can_finalize(t));

        // finalize
        let block = round.finalize(t, &group_pubkey).unwrap();
        assert_eq!(block.height, 1);
        assert!(block.verify(&group_pubkey, t).unwrap());
    }

    #[test]
    fn test_round_insufficient_contributions() {
        let mut rng = OsRng;

        let secret = Scalar::random(&mut rng);
        let group_pubkey = RistrettoPoint::generator().mul_scalar(&secret);

        let n = 5u32;
        let t = 3u32;
        let shares = shamir_split(&secret, n, t);

        let mut round: Round<RistrettoPoint> = Round::propose(1, 1, b"test".to_vec());

        // only 2 contributions (need 3)
        for share in shares.iter().take(2) {
            let contribution = round.contribute(share, &mut rng);
            round.add_contribution(contribution).unwrap();
        }

        assert!(!round.can_finalize(t));

        let result = round.finalize(t, &group_pubkey);
        assert!(matches!(
            result,
            Err(RoundError::InsufficientContributions { got: 2, need: 3 })
        ));
    }

    #[test]
    fn test_duplicate_contribution_rejected() {
        let mut rng = OsRng;

        let secret = Scalar::random(&mut rng);
        let shares = shamir_split(&secret, 5, 3);

        let mut round: Round<RistrettoPoint> = Round::propose(1, 1, b"test".to_vec());

        let c1 = round.contribute(&shares[0], &mut rng);
        round.add_contribution(c1).unwrap();

        // try to add another contribution from same member
        let c2 = round.contribute(&shares[0], &mut rng);
        let result = round.add_contribution(c2);

        assert!(matches!(result, Err(RoundError::DuplicateContribution(1))));
    }

    #[test]
    fn test_proposer_rotation() {
        assert_eq!(proposer_for_height(0, 5), 1);
        assert_eq!(proposer_for_height(1, 5), 2);
        assert_eq!(proposer_for_height(4, 5), 5);
        assert_eq!(proposer_for_height(5, 5), 1); // wraps
        assert_eq!(proposer_for_height(6, 5), 2);
    }

    #[test]
    fn test_block_hash_deterministic() {
        let mut rng = OsRng;

        let secret = Scalar::random(&mut rng);
        let group_pubkey = RistrettoPoint::generator().mul_scalar(&secret);

        let shares = shamir_split(&secret, 5, 3);

        // create and finalize round
        let mut round: Round<RistrettoPoint> = Round::propose(1, 1, b"test".to_vec());
        for share in shares.iter().take(3) {
            let contribution = round.contribute(share, &mut rng);
            round.add_contribution(contribution).unwrap();
        }
        let block = round.finalize(3, &group_pubkey).unwrap();

        // hash should be deterministic
        let h1 = block.hash();
        let h2 = block.hash();
        assert_eq!(h1, h2);
    }
}
