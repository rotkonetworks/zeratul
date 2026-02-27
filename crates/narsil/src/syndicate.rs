//! high-level syndicate API
//!
//! a syndicate is a group that collectively holds assets via threshold
//! custody with internal bft consensus for governance.

use alloc::vec::Vec;

use osst::{OsstPoint, SecretShare};

use crate::bft::{FinalizedBlock, Round, RoundError, proposer_for_height};
use crate::state::{NullifierSet, StateRoot, StateTransition};

/// syndicate configuration
#[derive(Clone, Debug)]
pub struct SyndicateConfig {
    /// number of members
    pub members: u32,
    /// threshold for signing (t-of-n)
    pub threshold: u32,
}

impl SyndicateConfig {
    pub fn new(members: u32, threshold: u32) -> Self {
        assert!(threshold > 0, "threshold must be positive");
        assert!(threshold <= members, "threshold cannot exceed members");
        Self { members, threshold }
    }

    /// 2-of-3 is common for small groups
    pub fn two_of_three() -> Self {
        Self::new(3, 2)
    }

    /// majority threshold
    pub fn majority(members: u32) -> Self {
        let threshold = members / 2 + 1;
        Self::new(members, threshold)
    }

    /// supermajority (2/3+1)
    pub fn supermajority(members: u32) -> Self {
        let threshold = (members * 2 / 3) + 1;
        Self::new(members, threshold)
    }
}

/// a syndicate member's local view
#[derive(Clone, Debug)]
pub struct Member<P: OsstPoint> {
    /// member's index (1-indexed)
    pub index: u32,
    /// member's secret share
    share: SecretShare<P::Scalar>,
    /// group public key
    pub group_pubkey: P,
    /// syndicate config
    pub config: SyndicateConfig,
}

impl<P: OsstPoint> Member<P> {
    pub fn new(
        index: u32,
        share: SecretShare<P::Scalar>,
        group_pubkey: P,
        config: SyndicateConfig,
    ) -> Self {
        assert!(index > 0 && index <= config.members, "invalid member index");
        Self {
            index,
            share,
            group_pubkey,
            config,
        }
    }

    /// check if this member is proposer for given height
    pub fn is_proposer(&self, height: u64) -> bool {
        proposer_for_height(height, self.config.members) == self.index
    }

    /// contribute to a round
    pub fn contribute<R: rand_core::RngCore + rand_core::CryptoRng>(
        &self,
        round: &Round<P>,
        rng: &mut R,
    ) -> osst::Contribution<P> {
        round.contribute(&self.share, rng)
    }
}

/// syndicate state machine
#[derive(Clone, Debug)]
pub struct Syndicate<P: OsstPoint> {
    /// syndicate configuration
    pub config: SyndicateConfig,
    /// group public key
    pub group_pubkey: P,
    /// current block height
    pub height: u64,
    /// current state root
    pub state_root: StateRoot,
    /// finalized blocks (audit log)
    blocks: Vec<FinalizedBlock<P>>,
    /// consumed nullifiers
    nullifiers: NullifierSet,
}

impl<P: OsstPoint> Syndicate<P> {
    /// create new syndicate with initial state
    pub fn new(config: SyndicateConfig, group_pubkey: P, initial_state: &[u8]) -> Self {
        Self {
            config,
            group_pubkey,
            height: 0,
            state_root: StateRoot::compute(initial_state),
            blocks: Vec::new(),
            nullifiers: NullifierSet::new(),
        }
    }

    /// get current proposer index
    pub fn current_proposer(&self) -> u32 {
        proposer_for_height(self.height + 1, self.config.members)
    }

    /// start new round (proposer only)
    pub fn propose(&self, transition: StateTransition) -> Result<Round<P>, &'static str> {
        // verify state chain
        if transition.prev_root != self.state_root {
            return Err("prev_root mismatch");
        }

        // check nullifiers not already consumed
        for n in &transition.nullifiers {
            if self.nullifiers.contains(n) {
                return Err("nullifier already consumed");
            }
        }

        let height = self.height + 1;
        let proposer = proposer_for_height(height, self.config.members);

        Ok(Round::propose(height, proposer, transition.to_bytes()))
    }

    /// apply finalized block to syndicate state
    pub fn apply(&mut self, block: FinalizedBlock<P>) -> Result<(), RoundError> {
        // verify block
        if !block.verify(&self.group_pubkey, self.config.threshold)? {
            return Err(RoundError::InvalidState("block verification failed"));
        }

        // verify height
        if block.height != self.height + 1 {
            return Err(RoundError::InvalidState("height mismatch"));
        }

        // parse and apply transition
        let transition = StateTransition::from_bytes(&block.payload)
            .ok_or(RoundError::InvalidState("invalid transition payload"))?;

        // verify state chain
        if transition.prev_root != self.state_root {
            return Err(RoundError::InvalidState("state root mismatch"));
        }

        // consume nullifiers
        if !self.nullifiers.insert_all(&transition.nullifiers) {
            return Err(RoundError::InvalidState("nullifier already consumed"));
        }

        // update state
        self.state_root = transition.new_root;
        self.height = block.height;
        self.blocks.push(block);

        Ok(())
    }

    /// get audit log (all finalized blocks)
    pub fn blocks(&self) -> &[FinalizedBlock<P>] {
        &self.blocks
    }

    /// get block at specific height
    pub fn block_at(&self, height: u64) -> Option<&FinalizedBlock<P>> {
        if height == 0 || height > self.height {
            return None;
        }
        self.blocks.get((height - 1) as usize)
    }
}

#[cfg(all(test, feature = "ristretto255"))]
mod tests {
    use super::*;
    use curve25519_dalek::{ristretto::RistrettoPoint, scalar::Scalar};
    use osst::OsstPoint;
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
    fn test_syndicate_basic_flow() {
        let mut rng = OsRng;

        // setup
        let secret = Scalar::random(&mut rng);
        let group_pubkey = RistrettoPoint::generator().mul_scalar(&secret);
        let config = SyndicateConfig::new(5, 3);
        let shares = shamir_split(&secret, 5, 3);

        // create members
        let members: Vec<Member<RistrettoPoint>> = shares
            .into_iter()
            .enumerate()
            .map(|(i, share)| {
                Member::new((i + 1) as u32, share, group_pubkey.clone(), config.clone())
            })
            .collect();

        // create syndicate
        let initial_state = b"genesis";
        let mut syndicate = Syndicate::new(config.clone(), group_pubkey.clone(), initial_state);

        assert_eq!(syndicate.height, 0);
        // at height 0, next block (height 1) has proposer = (1 % 5) + 1 = 2
        assert_eq!(syndicate.current_proposer(), 2);

        // create state transition
        let new_state = b"after transfer";
        let transition = StateTransition::new(
            syndicate.state_root,
            StateRoot::compute(new_state),
            b"transfer 100".to_vec(),
        );

        // propose round
        let mut round = syndicate.propose(transition).unwrap();

        // members contribute
        for member in members.iter().take(3) {
            let contribution: osst::Contribution<RistrettoPoint> = member.contribute(&round, &mut rng);
            round.add_contribution(contribution).unwrap();
        }

        // finalize
        let block = round.finalize(config.threshold, &group_pubkey).unwrap();

        // apply to syndicate
        syndicate.apply(block).unwrap();

        assert_eq!(syndicate.height, 1);
        assert_eq!(syndicate.state_root, StateRoot::compute(new_state));
        // at height 1, next block (height 2) has proposer = (2 % 5) + 1 = 3
        assert_eq!(syndicate.current_proposer(), 3);
    }

    #[test]
    fn test_member_proposer_check() {
        let mut rng = OsRng;

        let secret = Scalar::random(&mut rng);
        let group_pubkey = RistrettoPoint::generator().mul_scalar(&secret);
        let config = SyndicateConfig::new(3, 2);
        let shares = shamir_split(&secret, 3, 2);

        let member1 = Member::new(1, shares[0].clone(), group_pubkey.clone(), config.clone());
        let member2 = Member::new(2, shares[1].clone(), group_pubkey.clone(), config.clone());
        let member3 = Member::new(3, shares[2].clone(), group_pubkey.clone(), config.clone());

        // height 0 -> proposer for height 1 is member 2
        assert!(!member1.is_proposer(1));
        assert!(member2.is_proposer(1));
        assert!(!member3.is_proposer(1));

        // height 2 -> proposer is member 3
        assert!(!member1.is_proposer(2));
        assert!(!member2.is_proposer(2));
        assert!(member3.is_proposer(2));

        // height 3 -> wraps to member 1
        assert!(member1.is_proposer(3));
    }

    #[test]
    fn test_syndicate_nullifier_replay_prevention() {
        let mut rng = OsRng;

        let secret = Scalar::random(&mut rng);
        let group_pubkey = RistrettoPoint::generator().mul_scalar(&secret);
        let config = SyndicateConfig::two_of_three();
        let shares = shamir_split(&secret, 3, 2);

        let members: Vec<Member<RistrettoPoint>> = shares
            .into_iter()
            .enumerate()
            .map(|(i, share)| {
                Member::new((i + 1) as u32, share, group_pubkey.clone(), config.clone())
            })
            .collect();

        let mut syndicate = Syndicate::new(config.clone(), group_pubkey.clone(), b"genesis");

        // first transition with nullifier
        let nullifier = [42u8; 32];
        let transition1 = StateTransition::new(
            syndicate.state_root,
            StateRoot::compute(b"state1"),
            b"action1".to_vec(),
        )
        .with_nullifiers(vec![nullifier]);

        let mut round = syndicate.propose(transition1).unwrap();
        for member in members.iter().take(2) {
            let contribution: osst::Contribution<RistrettoPoint> = member.contribute(&round, &mut rng);
            round.add_contribution(contribution).unwrap();
        }
        let block = round.finalize(config.threshold, &group_pubkey).unwrap();
        syndicate.apply(block).unwrap();

        // try to reuse same nullifier
        let transition2 = StateTransition::new(
            syndicate.state_root,
            StateRoot::compute(b"state2"),
            b"action2".to_vec(),
        )
        .with_nullifiers(vec![nullifier]);

        let result = syndicate.propose(transition2);
        assert!(result.is_err());
        assert_eq!(result.unwrap_err(), "nullifier already consumed");
    }
}
