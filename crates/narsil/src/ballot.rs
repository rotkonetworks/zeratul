//! secret ballot voting via commit-reveal
//!
//! adapted from penumbra's governance model but without zk-SNARKs.
//! uses commit-reveal with nullifiers for anonymous double-vote prevention.
//!
//! # protocol
//!
//! ```text
//! phase 1 (commit):
//!   voter computes nullifier = hash(voter_secret || proposal_id)
//!   voter picks random blinding
//!   voter publishes commitment = hash(vote || blinding || nullifier)
//!   commitment is signed by ed25519 key and gossipped to network
//!
//! phase 2 (reveal):
//!   voter publishes (vote, blinding, nullifier)
//!   anyone verifies: hash(vote || blinding || nullifier) == commitment
//!   tally is computed from valid reveals
//!
//! phase 3 (tally):
//!   aggregate votes weighted by stake
//!   check quorum + threshold
//!   outcome: pass / fail / slash
//! ```
//!
//! # privacy properties
//!
//! - during commit phase: nobody knows any votes
//! - after reveal: votes are public but were locked before outcome was knowable
//! - nullifier prevents double-voting without revealing voter pubkey to tally
//! - gossip-level: ed25519 authenticated but only your direct peer sees origin
//! - upgrade path: replace reveal with threshold ElGamal for permanent secrecy
//!
//! # penumbra differences
//!
//! penumbra uses groth16 proofs over shielded notes + merkle trees to prove
//! stake ownership without revealing identity. we skip that and use nullifiers
//! derived from a shared secret. the tradeoff: the coordinator who receives
//! reveals can link nullifiers to voters (if they track who sent what). full
//! privacy requires the threshold ElGamal upgrade.

use alloc::collections::BTreeMap;
use alloc::vec::Vec;
use sha2::{Digest, Sha256};

use crate::wire::Hash32;

/// vote choices (same as penumbra: yes/no/abstain)
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Vote {
    Yes,
    No,
    Abstain,
}

impl Vote {
    fn as_byte(&self) -> u8 {
        match self {
            Vote::Yes => 1,
            Vote::No => 2,
            Vote::Abstain => 3,
        }
    }

    #[allow(dead_code)]
    fn from_byte(b: u8) -> Option<Self> {
        match b {
            1 => Some(Vote::Yes),
            2 => Some(Vote::No),
            3 => Some(Vote::Abstain),
            _ => None,
        }
    }
}

/// nullifier: hash(voter_secret || proposal_id) - unique per voter per proposal
/// prevents double-voting without revealing voter identity in the tally
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct Nullifier(pub Hash32);

impl Nullifier {
    /// derive nullifier from voter's secret key and proposal id
    pub fn derive(voter_secret: &[u8; 32], proposal_id: u64) -> Self {
        let mut h = Sha256::new();
        h.update(b"narsil.ballot.nullifier");
        h.update(voter_secret);
        h.update(proposal_id.to_le_bytes());
        let hash = h.finalize();
        let mut nf = [0u8; 32];
        nf.copy_from_slice(&hash);
        Nullifier(nf)
    }
}

/// commitment to a vote: hash(vote || blinding || nullifier)
/// published in phase 1 before anyone reveals
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct VoteCommitment(pub Hash32);

impl VoteCommitment {
    /// create commitment from vote, blinding, and nullifier
    pub fn compute(vote: Vote, blinding: &[u8; 32], nullifier: &Nullifier) -> Self {
        let mut h = Sha256::new();
        h.update(b"narsil.ballot.commitment");
        h.update([vote.as_byte()]);
        h.update(blinding);
        h.update(nullifier.0);
        let hash = h.finalize();
        let mut cm = [0u8; 32];
        cm.copy_from_slice(&hash);
        VoteCommitment(cm)
    }

    /// verify that a reveal matches this commitment
    pub fn verify(&self, vote: Vote, blinding: &[u8; 32], nullifier: &Nullifier) -> bool {
        Self::compute(vote, blinding, nullifier) == *self
    }
}

/// a sealed vote (phase 1 output)
#[derive(Clone, Debug)]
pub struct SealedVote {
    /// commitment to the vote
    pub commitment: VoteCommitment,
    /// nullifier (reveals which "slot" this vote fills, not who)
    pub nullifier: Nullifier,
    /// voting power (stake amount)
    pub power: u64,
    /// ed25519 signature over (commitment || nullifier || power || proposal_id)
    pub signature: [u8; 64],
    /// ed25519 public key of the signer
    pub signer: Hash32,
}

/// a revealed vote (phase 2 output)
#[derive(Clone, Debug)]
pub struct RevealedVote {
    /// the actual vote
    pub vote: Vote,
    /// blinding used in commitment
    pub blinding: [u8; 32],
    /// nullifier (must match the sealed vote)
    pub nullifier: Nullifier,
}

/// tally: aggregated vote counts weighted by stake
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Tally {
    pub yes: u64,
    pub no: u64,
    pub abstain: u64,
}

impl Tally {
    pub fn total(&self) -> u64 {
        self.yes + self.no + self.abstain
    }

    /// add a vote with given power
    pub fn add(&mut self, vote: Vote, power: u64) {
        match vote {
            Vote::Yes => self.yes += power,
            Vote::No => self.no += power,
            Vote::Abstain => self.abstain += power,
        }
    }

    /// yes ratio as (numerator, denominator) - abstains excluded from ratio
    pub fn yes_ratio(&self) -> (u64, u64) {
        let denom = self.yes + self.no;
        if denom == 0 {
            (0, 1)
        } else {
            (self.yes, denom)
        }
    }

    /// check if quorum met (total votes / total power >= quorum threshold)
    /// quorum_bps: required quorum in basis points (5000 = 50%)
    pub fn meets_quorum(&self, total_power: u64, quorum_bps: u32) -> bool {
        if total_power == 0 {
            return false;
        }
        // total_voted * 10000 >= total_power * quorum_bps
        (self.total() as u128 * 10_000) >= (total_power as u128 * quorum_bps as u128)
    }

    /// check outcome given parameters
    /// pass_bps: yes threshold in basis points (6700 = 67%)
    /// slash_bps: no threshold for slashing (8000 = 80%, like penumbra)
    pub fn outcome(
        &self,
        total_power: u64,
        quorum_bps: u32,
        pass_bps: u32,
        slash_bps: u32,
    ) -> BallotOutcome {
        if !self.meets_quorum(total_power, quorum_bps) {
            return BallotOutcome::Fail;
        }

        // check slash: no / total_voted > slash threshold
        let total_voted = self.total();
        if total_voted > 0 && (self.no as u128 * 10_000) > (total_voted as u128 * slash_bps as u128) {
            return BallotOutcome::Slash;
        }

        // check pass: yes / (yes + no) > pass threshold
        let (yes, denom) = self.yes_ratio();
        if denom > 0 && (yes as u128 * 10_000) >= (denom as u128 * pass_bps as u128) {
            BallotOutcome::Pass
        } else {
            BallotOutcome::Fail
        }
    }
}

/// ballot outcome (adapted from penumbra)
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BallotOutcome {
    /// proposal passed
    Pass,
    /// proposal failed
    Fail,
    /// proposal slashed (>80% no votes, deposit burned)
    Slash,
}

/// ballot phase
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BallotPhase {
    /// accepting commitments
    Commit,
    /// accepting reveals
    Reveal,
    /// tallying complete
    Tallied,
    /// cancelled
    Cancelled,
}

/// a ballot for a single proposal
#[derive(Clone, Debug)]
pub struct Ballot {
    /// proposal id
    pub proposal_id: u64,
    /// current phase
    pub phase: BallotPhase,
    /// sealed votes indexed by nullifier
    sealed: BTreeMap<Nullifier, SealedVote>,
    /// revealed votes indexed by nullifier
    revealed: BTreeMap<Nullifier, RevealedVote>,
    /// final tally (set after tallying)
    pub tally: Tally,
    /// commit phase deadline
    pub commit_deadline: u64,
    /// reveal phase deadline
    pub reveal_deadline: u64,
}

/// ballot errors
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum BallotError {
    /// wrong phase for this operation
    WrongPhase { expected: BallotPhase, got: BallotPhase },
    /// nullifier already used (double vote)
    DuplicateNullifier,
    /// reveal doesn't match commitment
    InvalidReveal,
    /// no sealed vote for this nullifier
    NoCommitment,
    /// voting power must be positive
    ZeroPower,
}

impl Ballot {
    /// create new ballot
    pub fn new(proposal_id: u64, commit_deadline: u64, reveal_deadline: u64) -> Self {
        Self {
            proposal_id,
            phase: BallotPhase::Commit,
            sealed: BTreeMap::new(),
            revealed: BTreeMap::new(),
            tally: Tally::default(),
            commit_deadline,
            reveal_deadline,
        }
    }

    /// submit a sealed vote (commit phase)
    pub fn commit(&mut self, sealed: SealedVote) -> Result<(), BallotError> {
        if self.phase != BallotPhase::Commit {
            return Err(BallotError::WrongPhase {
                expected: BallotPhase::Commit,
                got: self.phase,
            });
        }
        if sealed.power == 0 {
            return Err(BallotError::ZeroPower);
        }
        if self.sealed.contains_key(&sealed.nullifier) {
            return Err(BallotError::DuplicateNullifier);
        }
        self.sealed.insert(sealed.nullifier, sealed);
        Ok(())
    }

    /// advance from commit to reveal phase
    pub fn start_reveal(&mut self) {
        if self.phase == BallotPhase::Commit {
            self.phase = BallotPhase::Reveal;
        }
    }

    /// submit a revealed vote (reveal phase)
    pub fn reveal(&mut self, revealed: RevealedVote) -> Result<(), BallotError> {
        if self.phase != BallotPhase::Reveal {
            return Err(BallotError::WrongPhase {
                expected: BallotPhase::Reveal,
                got: self.phase,
            });
        }
        let sealed = self.sealed.get(&revealed.nullifier)
            .ok_or(BallotError::NoCommitment)?;

        // verify commitment matches reveal
        if !sealed.commitment.verify(revealed.vote, &revealed.blinding, &revealed.nullifier) {
            return Err(BallotError::InvalidReveal);
        }

        self.revealed.insert(revealed.nullifier, revealed);
        Ok(())
    }

    /// tally all revealed votes
    pub fn finalize(&mut self) -> &Tally {
        let mut tally = Tally::default();

        for (nf, reveal) in &self.revealed {
            if let Some(sealed) = self.sealed.get(nf) {
                tally.add(reveal.vote, sealed.power);
            }
        }

        self.tally = tally;
        self.phase = BallotPhase::Tallied;
        &self.tally
    }

    /// number of commitments received
    pub fn commit_count(&self) -> usize {
        self.sealed.len()
    }

    /// number of reveals received
    pub fn reveal_count(&self) -> usize {
        self.revealed.len()
    }

    /// unrevealed commitments (voters who didn't reveal - can be penalized)
    pub fn unrevealed(&self) -> Vec<&SealedVote> {
        self.sealed.iter()
            .filter(|(nf, _)| !self.revealed.contains_key(nf))
            .map(|(_, sv)| sv)
            .collect()
    }
}

/// staked ballot: ties ballot to NPoS election state
///
/// voting power comes from stake. validators vote on behalf of their
/// nominators (like penumbra), but any nominator who votes directly
/// overrides their portion of the validator's vote.
pub struct StakedBallot {
    /// inner ballot
    pub ballot: Ballot,
    /// validator pubkey -> total backing stake (from election)
    validator_stakes: BTreeMap<Hash32, u64>,
    /// nominator pubkey -> (stake, delegated_validator)
    nominator_stakes: BTreeMap<Hash32, (u64, Hash32)>,
    /// who has voted (pubkey) - tracked to compute overrides
    voted: BTreeMap<Nullifier, Hash32>,
    /// total voting power (sum of all elected validator stakes)
    total_power: u64,
}

impl StakedBallot {
    /// create from current election result
    pub fn from_election(
        proposal_id: u64,
        commit_deadline: u64,
        reveal_deadline: u64,
        election: &crate::election::Election,
    ) -> Option<Self> {
        let result = election.current_result.as_ref()?;

        let mut validator_stakes = BTreeMap::new();
        let mut nominator_stakes = BTreeMap::new();

        for elected in &result.elected {
            validator_stakes.insert(elected.pubkey, elected.total_stake);
            for (nom_pk, nom_stake) in &elected.backers {
                nominator_stakes.insert(*nom_pk, (*nom_stake, elected.pubkey));
            }
        }

        Some(Self {
            ballot: Ballot::new(proposal_id, commit_deadline, reveal_deadline),
            total_power: result.total_stake,
            validator_stakes,
            nominator_stakes,
            voted: BTreeMap::new(),
        })
    }

    /// get voting power for a pubkey (validator or nominator)
    pub fn power_of(&self, pubkey: &Hash32) -> u64 {
        if let Some(&stake) = self.validator_stakes.get(pubkey) {
            stake
        } else if let Some(&(stake, _)) = self.nominator_stakes.get(pubkey) {
            stake
        } else {
            0
        }
    }

    /// commit a vote using stake-derived power
    pub fn commit(
        &mut self,
        voter_pubkey: Hash32,
        voter_secret: &[u8; 32],
        vote: Vote,
        blinding: [u8; 32],
        signature: [u8; 64],
    ) -> Result<(), BallotError> {
        let power = self.power_of(&voter_pubkey);
        if power == 0 {
            return Err(BallotError::ZeroPower);
        }

        let nf = Nullifier::derive(voter_secret, self.ballot.proposal_id);
        let commitment = VoteCommitment::compute(vote, &blinding, &nf);

        let sealed = SealedVote {
            commitment,
            nullifier: nf,
            power,
            signature,
            signer: voter_pubkey,
        };

        self.ballot.commit(sealed)?;
        self.voted.insert(nf, voter_pubkey);
        Ok(())
    }

    /// reveal a vote
    pub fn reveal(&mut self, revealed: RevealedVote) -> Result<(), BallotError> {
        self.ballot.reveal(revealed)
    }

    /// finalize with nominator override: if a nominator voted directly,
    /// subtract their portion from the validator they delegated to
    pub fn finalize(&mut self) -> &Tally {
        // first do normal tally
        self.ballot.finalize();

        // now apply overrides: find nominators who voted and reduce
        // their validator's effective vote by the nominator's stake
        let mut validator_overrides: BTreeMap<Hash32, u64> = BTreeMap::new();

        for (_nf, voter_pk) in &self.voted {
            if let Some(&(nom_stake, validator)) = self.nominator_stakes.get(voter_pk) {
                *validator_overrides.entry(validator).or_insert(0) += nom_stake;
            }
        }

        // rebuild tally accounting for overrides
        let mut tally = Tally::default();
        for (nf, reveal) in &self.ballot.revealed {
            if let Some(sealed) = self.ballot.sealed.get(nf) {
                let voter = self.voted.get(nf);
                let mut power = sealed.power;

                // if this is a validator, reduce by nominator overrides
                if let Some(pk) = voter {
                    if self.validator_stakes.contains_key(pk) {
                        let override_amount = validator_overrides.get(pk).copied().unwrap_or(0);
                        power = power.saturating_sub(override_amount);
                    }
                }

                if power > 0 {
                    tally.add(reveal.vote, power);
                }
            }
        }

        self.ballot.tally = tally;
        &self.ballot.tally
    }

    /// total voting power across all elected validators
    pub fn total_power(&self) -> u64 {
        self.total_power
    }

    /// check outcome using governance thresholds
    pub fn outcome(
        &self,
        quorum_bps: u32,
        pass_bps: u32,
        slash_bps: u32,
    ) -> BallotOutcome {
        self.ballot.tally.outcome(self.total_power, quorum_bps, pass_bps, slash_bps)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn voter_secret(n: u8) -> [u8; 32] {
        let mut s = [0u8; 32];
        s[0] = n;
        s
    }

    fn random_blinding(n: u8) -> [u8; 32] {
        let mut b = [0u8; 32];
        b[0] = n;
        b[31] = 0xff;
        b
    }

    fn make_sealed(
        voter: u8,
        proposal_id: u64,
        vote: Vote,
        power: u64,
        blinding: &[u8; 32],
    ) -> (SealedVote, RevealedVote) {
        let nf = Nullifier::derive(&voter_secret(voter), proposal_id);
        let commitment = VoteCommitment::compute(vote, blinding, &nf);

        let sealed = SealedVote {
            commitment,
            nullifier: nf,
            power,
            signature: [0u8; 64], // skipping ed25519 in unit tests
            signer: voter_secret(voter),
        };

        let revealed = RevealedVote {
            vote,
            blinding: *blinding,
            nullifier: nf,
        };

        (sealed, revealed)
    }

    #[test]
    fn test_commitment_verify() {
        let nf = Nullifier::derive(&voter_secret(1), 42);
        let blinding = random_blinding(1);
        let cm = VoteCommitment::compute(Vote::Yes, &blinding, &nf);

        assert!(cm.verify(Vote::Yes, &blinding, &nf));
        assert!(!cm.verify(Vote::No, &blinding, &nf));
        assert!(!cm.verify(Vote::Yes, &random_blinding(2), &nf));
    }

    #[test]
    fn test_nullifier_unique_per_proposal() {
        let secret = voter_secret(1);
        let nf1 = Nullifier::derive(&secret, 1);
        let nf2 = Nullifier::derive(&secret, 2);
        assert_ne!(nf1, nf2);
    }

    #[test]
    fn test_nullifier_unique_per_voter() {
        let nf1 = Nullifier::derive(&voter_secret(1), 42);
        let nf2 = Nullifier::derive(&voter_secret(2), 42);
        assert_ne!(nf1, nf2);
    }

    #[test]
    fn test_full_ballot_flow() {
        let proposal_id = 1;
        let mut ballot = Ballot::new(proposal_id, 100, 200);

        let b1 = random_blinding(1);
        let b2 = random_blinding(2);
        let b3 = random_blinding(3);

        // 3 voters: alice (40 stake yes), bob (35 stake no), carol (25 stake yes)
        let (s1, r1) = make_sealed(1, proposal_id, Vote::Yes, 40, &b1);
        let (s2, r2) = make_sealed(2, proposal_id, Vote::No, 35, &b2);
        let (s3, r3) = make_sealed(3, proposal_id, Vote::Yes, 25, &b3);

        // commit phase
        ballot.commit(s1).unwrap();
        ballot.commit(s2).unwrap();
        ballot.commit(s3).unwrap();
        assert_eq!(ballot.commit_count(), 3);

        // reveal phase
        ballot.start_reveal();
        ballot.reveal(r1).unwrap();
        ballot.reveal(r2).unwrap();
        ballot.reveal(r3).unwrap();
        assert_eq!(ballot.reveal_count(), 3);

        // tally
        let tally = ballot.finalize();
        assert_eq!(tally.yes, 65);  // alice + carol
        assert_eq!(tally.no, 35);   // bob
        assert_eq!(tally.abstain, 0);

        // outcome: 65/100 yes ratio, quorum met
        let outcome = tally.outcome(100, 5000, 5100, 8000);
        assert_eq!(outcome, BallotOutcome::Pass);
    }

    #[test]
    fn test_double_vote_prevented() {
        let mut ballot = Ballot::new(1, 100, 200);
        let b = random_blinding(1);
        let (s1, _) = make_sealed(1, 1, Vote::Yes, 40, &b);
        let s1_dup = s1.clone();

        ballot.commit(s1).unwrap();
        assert_eq!(ballot.commit(s1_dup).unwrap_err(), BallotError::DuplicateNullifier);
    }

    #[test]
    fn test_invalid_reveal_rejected() {
        let mut ballot = Ballot::new(1, 100, 200);
        let b = random_blinding(1);
        let (s1, mut r1) = make_sealed(1, 1, Vote::Yes, 40, &b);

        ballot.commit(s1).unwrap();
        ballot.start_reveal();

        // tamper with vote
        r1.vote = Vote::No;
        assert_eq!(ballot.reveal(r1).unwrap_err(), BallotError::InvalidReveal);
    }

    #[test]
    fn test_wrong_phase_errors() {
        let mut ballot = Ballot::new(1, 100, 200);
        let b = random_blinding(1);
        let (s1, r1) = make_sealed(1, 1, Vote::Yes, 40, &b);

        // can't reveal during commit phase
        assert_eq!(
            ballot.reveal(r1.clone()).unwrap_err(),
            BallotError::WrongPhase { expected: BallotPhase::Reveal, got: BallotPhase::Commit }
        );

        ballot.commit(s1).unwrap();
        ballot.start_reveal();

        // can't commit during reveal phase
        let (s2, _) = make_sealed(2, 1, Vote::No, 30, &b);
        assert_eq!(
            ballot.commit(s2).unwrap_err(),
            BallotError::WrongPhase { expected: BallotPhase::Commit, got: BallotPhase::Reveal }
        );

        ballot.reveal(r1).unwrap();
    }

    #[test]
    fn test_unrevealed_tracked() {
        let mut ballot = Ballot::new(1, 100, 200);
        let b1 = random_blinding(1);
        let b2 = random_blinding(2);

        let (s1, r1) = make_sealed(1, 1, Vote::Yes, 40, &b1);
        let (s2, _r2) = make_sealed(2, 1, Vote::No, 35, &b2);

        ballot.commit(s1).unwrap();
        ballot.commit(s2).unwrap();
        ballot.start_reveal();

        // only alice reveals
        ballot.reveal(r1).unwrap();

        let unrevealed = ballot.unrevealed();
        assert_eq!(unrevealed.len(), 1);
        assert_eq!(unrevealed[0].power, 35); // bob didn't reveal
    }

    #[test]
    fn test_tally_quorum() {
        let mut tally = Tally::default();
        tally.add(Vote::Yes, 30);

        // 30/100 = 30% < 50% quorum
        assert!(!tally.meets_quorum(100, 5000));

        tally.add(Vote::No, 25);
        // 55/100 = 55% >= 50% quorum
        assert!(tally.meets_quorum(100, 5000));
    }

    #[test]
    fn test_tally_slash() {
        let mut tally = Tally::default();
        tally.add(Vote::No, 85);
        tally.add(Vote::Yes, 15);

        // 85% no > 80% slash threshold
        let outcome = tally.outcome(100, 5000, 5100, 8000);
        assert_eq!(outcome, BallotOutcome::Slash);
    }

    #[test]
    fn test_tally_fail_no_quorum() {
        let mut tally = Tally::default();
        tally.add(Vote::Yes, 10);

        // 10% participation, quorum needs 50%
        let outcome = tally.outcome(100, 5000, 5100, 8000);
        assert_eq!(outcome, BallotOutcome::Fail);
    }

    #[test]
    fn test_staked_ballot_from_election() {
        use crate::election::{Election, EraConfig};

        fn pk(n: u8) -> [u8; 32] {
            let mut h = [0u8; 32];
            h[0] = n;
            h
        }

        let config = EraConfig {
            seats: 2,
            min_self_stake: 100,
            min_nominator_stake: 10,
            ..Default::default()
        };
        let mut election = Election::new(config);

        // validators
        election.register_validator(pk(1), 1000, 0).unwrap(); // alice
        election.register_validator(pk(2), 800, 0).unwrap();  // bob
        election.register_validator(pk(3), 100, 0).unwrap();  // carol (won't be elected)

        // nominator dave backs alice with 5000
        election.nominate(pk(10), 5000, vec![pk(1)]).unwrap();

        election.run_election().unwrap();

        // create staked ballot
        let mut sb = StakedBallot::from_election(1, 100, 200, &election).unwrap();

        // alice (validator) votes yes
        let b1 = random_blinding(1);
        sb.commit(pk(1), &voter_secret(1), Vote::Yes, b1, [0u8; 64]).unwrap();

        // bob (validator) votes no
        let b2 = random_blinding(2);
        sb.commit(pk(2), &voter_secret(2), Vote::No, b2, [0u8; 64]).unwrap();

        // dave (nominator of alice) votes no - overrides alice's yes for dave's portion
        let b3 = random_blinding(3);
        sb.commit(pk(10), &voter_secret(10), Vote::No, b3, [0u8; 64]).unwrap();

        // reveal all
        sb.ballot.start_reveal();
        sb.reveal(RevealedVote {
            vote: Vote::Yes,
            blinding: b1,
            nullifier: Nullifier::derive(&voter_secret(1), 1),
        }).unwrap();
        sb.reveal(RevealedVote {
            vote: Vote::No,
            blinding: b2,
            nullifier: Nullifier::derive(&voter_secret(2), 1),
        }).unwrap();
        sb.reveal(RevealedVote {
            vote: Vote::No,
            blinding: b3,
            nullifier: Nullifier::derive(&voter_secret(10), 1),
        }).unwrap();

        let tally = sb.finalize();

        // alice's total_stake included dave's 5000
        // dave voted no, so alice's yes is reduced by dave's 5000
        // alice yes: total_stake - dave's portion
        // dave no: 5000
        // bob no: 800
        assert!(tally.yes < tally.no, "nominator override should reduce validator's yes power");
        assert!(tally.no > 0);

        // dave's 5000 no + bob's 800 no should dominate
        // alice's yes = her total_stake (1000 self + 5000 dave) - dave's 5000 override = 1000
        assert_eq!(tally.yes, 1000);
    }

    #[test]
    fn test_tally_abstain_excluded_from_ratio() {
        let mut tally = Tally::default();
        tally.add(Vote::Yes, 30);
        tally.add(Vote::Abstain, 70);

        // yes ratio: 30/(30+0) = 100% (abstains don't count against)
        // quorum: 100/100 = 100% >= 50%
        let outcome = tally.outcome(100, 5000, 5100, 8000);
        assert_eq!(outcome, BallotOutcome::Pass);
    }
}
