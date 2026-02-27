//! proactive resharing for membership and threshold changes
//!
//! resharing allows the syndicate to:
//! - add new members (with share allocation)
//! - remove members (transfer their shares)
//! - change the signing threshold
//! - rotate keys for forward secrecy
//!
//! the group public key remains the same, but individual shares change.
//!
//! # protocol
//!
//! resharing uses the same DKG structure as formation, but starts from
//! the existing shares rather than generating new random polynomials.
//!
//! ```text
//! old shares ──▶ reshare polynomial ──▶ new shares
//!     s_i              f_i(x)              s'_j
//! ```

use alloc::string::String;
use alloc::vec::Vec;
use sha2::{Digest, Sha256};

use crate::wire::{Hash32, ShareId};

/// reshare request types
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ReshareReason {
    /// add new member(s)
    AddMembers { new_members: Vec<Hash32> },
    /// remove member(s)
    RemoveMembers { removed: Vec<Hash32> },
    /// change threshold
    ThresholdChange { old: u8, new: u8 },
    /// key rotation for forward secrecy
    KeyRotation,
    /// recover from lost member
    Recovery { lost_member: Hash32 },
}

/// reshare phase
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ResharePhase {
    /// collecting approvals
    Approving,
    /// collecting commitments from old share holders
    Committing,
    /// distributing shares to new holders
    Distributing,
    /// verifying new shares
    Verifying,
    /// complete
    Complete,
    /// aborted
    Aborted,
}

/// reshare proposal
#[derive(Clone, Debug)]
pub struct ReshareProposal {
    /// reason for reshare
    pub reason: ReshareReason,
    /// new threshold (if changing)
    pub new_threshold: Option<u8>,
    /// new member allocation
    pub new_allocation: Vec<(Hash32, Vec<ShareId>)>,
    /// deadline for reshare
    pub deadline: u64,
}

impl ReshareProposal {
    /// create proposal to add members
    pub fn add_members(
        new_members: Vec<(Hash32, String, Vec<ShareId>)>,
        deadline: u64,
    ) -> Self {
        let new_allocation: Vec<_> = new_members
            .iter()
            .map(|(pk, _, shares)| (*pk, shares.clone()))
            .collect();
        let pubkeys: Vec<_> = new_members.iter().map(|(pk, _, _)| *pk).collect();

        Self {
            reason: ReshareReason::AddMembers { new_members: pubkeys },
            new_threshold: None,
            new_allocation,
            deadline,
        }
    }

    /// create proposal to remove members
    pub fn remove_members(
        removed: Vec<Hash32>,
        share_reassignment: Vec<(Hash32, Vec<ShareId>)>,
        deadline: u64,
    ) -> Self {
        Self {
            reason: ReshareReason::RemoveMembers { removed },
            new_threshold: None,
            new_allocation: share_reassignment,
            deadline,
        }
    }

    /// create proposal to change threshold
    pub fn change_threshold(old: u8, new: u8, deadline: u64) -> Self {
        Self {
            reason: ReshareReason::ThresholdChange { old, new },
            new_threshold: Some(new),
            new_allocation: Vec::new(),
            deadline,
        }
    }

    /// create proposal for key rotation
    pub fn key_rotation(deadline: u64) -> Self {
        Self {
            reason: ReshareReason::KeyRotation,
            new_threshold: None,
            new_allocation: Vec::new(),
            deadline,
        }
    }
}

/// reshare session
#[derive(Clone, Debug)]
pub struct ReshareSession {
    /// syndicate id
    pub syndicate_id: Hash32,
    /// current epoch (will increment)
    pub current_epoch: u64,
    /// proposal
    pub proposal: ReshareProposal,
    /// current phase
    pub phase: ResharePhase,
    /// approvals received (share ids)
    pub approvals: Vec<ShareId>,
    /// required approval threshold
    pub approval_threshold: u8,
    /// old members (holding current shares)
    pub old_members: Vec<OldMember>,
    /// new members (receiving new shares)
    pub new_members: Vec<NewMember>,
    /// commitments received
    pub commitments: Vec<ReshareCommitment>,
    /// shares distributed
    pub distributed_shares: Vec<DistributedShare>,
}

/// old member in reshare
#[derive(Clone, Debug)]
pub struct OldMember {
    pub pubkey: Hash32,
    pub shares: Vec<ShareId>,
}

/// new member in reshare
#[derive(Clone, Debug)]
pub struct NewMember {
    pub pubkey: Hash32,
    pub viewing_key: [u8; 32],
    pub allocated_shares: Vec<ShareId>,
}

/// reshare commitment from old member
#[derive(Clone, Debug)]
pub struct ReshareCommitment {
    pub from: Hash32,
    pub share_id: ShareId,
    pub commitment: [u8; 32],
    pub verification_points: Vec<[u8; 32]>,
}

/// share distributed during reshare
#[derive(Clone, Debug)]
pub struct DistributedShare {
    pub from: Hash32,
    pub to: Hash32,
    pub for_share_id: ShareId,
    pub encrypted_data: Vec<u8>,
}

impl ReshareSession {
    /// create new reshare session
    pub fn new(
        syndicate_id: Hash32,
        current_epoch: u64,
        proposal: ReshareProposal,
        old_members: Vec<OldMember>,
        approval_threshold: u8,
    ) -> Self {
        Self {
            syndicate_id,
            current_epoch,
            proposal,
            phase: ResharePhase::Approving,
            approvals: Vec::new(),
            approval_threshold,
            old_members,
            new_members: Vec::new(),
            commitments: Vec::new(),
            distributed_shares: Vec::new(),
        }
    }

    /// add approval from share holder
    pub fn add_approval(&mut self, share_id: ShareId) -> Result<(), ReshareError> {
        if self.phase != ResharePhase::Approving {
            return Err(ReshareError::WrongPhase);
        }

        // verify share exists in old members
        let valid = self
            .old_members
            .iter()
            .any(|m| m.shares.contains(&share_id));
        if !valid {
            return Err(ReshareError::InvalidShare);
        }

        if !self.approvals.contains(&share_id) {
            self.approvals.push(share_id);
        }

        // check if threshold met
        if self.approvals.len() >= self.approval_threshold as usize {
            self.phase = ResharePhase::Committing;
        }

        Ok(())
    }

    /// add new member
    pub fn add_new_member(&mut self, member: NewMember) -> Result<(), ReshareError> {
        if self.phase != ResharePhase::Approving && self.phase != ResharePhase::Committing {
            return Err(ReshareError::WrongPhase);
        }

        if self.new_members.iter().any(|m| m.pubkey == member.pubkey) {
            return Err(ReshareError::DuplicateMember);
        }

        self.new_members.push(member);
        Ok(())
    }

    /// add commitment from old share holder
    pub fn add_commitment(&mut self, commitment: ReshareCommitment) -> Result<(), ReshareError> {
        if self.phase != ResharePhase::Committing {
            return Err(ReshareError::WrongPhase);
        }

        // verify from old member with this share
        let valid = self.old_members.iter().any(|m| {
            m.pubkey == commitment.from && m.shares.contains(&commitment.share_id)
        });
        if !valid {
            return Err(ReshareError::InvalidShare);
        }

        // check not duplicate
        if self.commitments.iter().any(|c| {
            c.from == commitment.from && c.share_id == commitment.share_id
        }) {
            return Err(ReshareError::DuplicateCommitment);
        }

        self.commitments.push(commitment);

        // check if all old shares committed
        let total_old: usize = self.old_members.iter().map(|m| m.shares.len()).sum();
        if self.commitments.len() >= total_old {
            self.phase = ResharePhase::Distributing;
        }

        Ok(())
    }

    /// add distributed share
    pub fn add_distributed_share(&mut self, share: DistributedShare) -> Result<(), ReshareError> {
        if self.phase != ResharePhase::Distributing {
            return Err(ReshareError::WrongPhase);
        }

        self.distributed_shares.push(share);

        // check if distribution complete
        // for each new share, need contributions from threshold old shares
        let new_shares: usize = self.new_members.iter().map(|m| m.allocated_shares.len()).sum();
        let expected = new_shares * self.approval_threshold as usize;
        if self.distributed_shares.len() >= expected {
            self.phase = ResharePhase::Verifying;
        }

        Ok(())
    }

    /// finalize reshare
    pub fn finalize(&mut self) -> Result<ReshareResult, ReshareError> {
        if self.phase != ResharePhase::Verifying {
            return Err(ReshareError::WrongPhase);
        }

        // in real impl:
        // 1. each new member combines received shares
        // 2. verify share consistency against commitments
        // 3. compute new group key (should equal old)

        let new_epoch = self.current_epoch + 1;

        // derive new group key hash (for verification)
        let mut hasher = Sha256::new();
        hasher.update(b"narsil-reshare-v1");
        hasher.update(&self.syndicate_id);
        hasher.update(&new_epoch.to_le_bytes());
        for c in &self.commitments {
            hasher.update(&c.commitment);
        }
        let verification_hash: [u8; 32] = hasher.finalize().into();

        self.phase = ResharePhase::Complete;

        Ok(ReshareResult {
            syndicate_id: self.syndicate_id,
            new_epoch,
            new_threshold: self.proposal.new_threshold,
            new_members: self.new_members.clone(),
            verification_hash,
        })
    }

    /// abort reshare
    pub fn abort(&mut self) {
        self.phase = ResharePhase::Aborted;
    }

    /// check if complete
    pub fn is_complete(&self) -> bool {
        self.phase == ResharePhase::Complete
    }
}

/// result of successful reshare
#[derive(Clone, Debug)]
pub struct ReshareResult {
    /// syndicate id (unchanged)
    pub syndicate_id: Hash32,
    /// new epoch
    pub new_epoch: u64,
    /// new threshold if changed
    pub new_threshold: Option<u8>,
    /// new members
    pub new_members: Vec<NewMember>,
    /// verification hash
    pub verification_hash: [u8; 32],
}

/// reshare errors
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ReshareError {
    /// wrong phase
    WrongPhase,
    /// invalid share
    InvalidShare,
    /// duplicate member
    DuplicateMember,
    /// duplicate commitment
    DuplicateCommitment,
    /// verification failed
    VerificationFailed,
    /// insufficient approvals
    InsufficientApprovals,
    /// deadline passed
    DeadlinePassed,
}

impl core::fmt::Display for ReshareError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::WrongPhase => write!(f, "wrong phase"),
            Self::InvalidShare => write!(f, "invalid share"),
            Self::DuplicateMember => write!(f, "duplicate member"),
            Self::DuplicateCommitment => write!(f, "duplicate commitment"),
            Self::VerificationFailed => write!(f, "verification failed"),
            Self::InsufficientApprovals => write!(f, "insufficient approvals"),
            Self::DeadlinePassed => write!(f, "deadline passed"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn setup_session() -> ReshareSession {
        let syndicate_id = [1u8; 32];
        let proposal = ReshareProposal::key_rotation(9999999999);
        let old_members = vec![
            OldMember {
                pubkey: [1u8; 32],
                shares: vec![1, 2, 3],
            },
            OldMember {
                pubkey: [2u8; 32],
                shares: vec![4, 5],
            },
        ];

        ReshareSession::new(syndicate_id, 0, proposal, old_members, 3)
    }

    #[test]
    fn test_reshare_approval() {
        let mut session = setup_session();

        session.add_approval(1).unwrap();
        session.add_approval(2).unwrap();
        assert_eq!(session.phase, ResharePhase::Approving);

        session.add_approval(3).unwrap();
        assert_eq!(session.phase, ResharePhase::Committing);
    }

    #[test]
    fn test_reshare_invalid_share() {
        let mut session = setup_session();

        let result = session.add_approval(99); // doesn't exist
        assert!(matches!(result, Err(ReshareError::InvalidShare)));
    }

    #[test]
    fn test_reshare_add_commitment() {
        let mut session = setup_session();

        // get to committing phase
        session.add_approval(1).unwrap();
        session.add_approval(2).unwrap();
        session.add_approval(3).unwrap();

        // add commitments
        for share_id in [1, 2, 3] {
            session.add_commitment(ReshareCommitment {
                from: [1u8; 32],
                share_id,
                commitment: [10u8; 32],
                verification_points: vec![],
            }).unwrap();
        }

        for share_id in [4, 5] {
            session.add_commitment(ReshareCommitment {
                from: [2u8; 32],
                share_id,
                commitment: [20u8; 32],
                verification_points: vec![],
            }).unwrap();
        }

        assert_eq!(session.phase, ResharePhase::Distributing);
    }

    #[test]
    fn test_reshare_add_members() {
        let proposal = ReshareProposal::add_members(
            vec![
                ([3u8; 32], "carol".into(), vec![6, 7]),
            ],
            9999999999,
        );

        assert!(matches!(
            proposal.reason,
            ReshareReason::AddMembers { .. }
        ));
        assert_eq!(proposal.new_allocation.len(), 1);
    }

    #[test]
    fn test_reshare_remove_members() {
        let proposal = ReshareProposal::remove_members(
            vec![[2u8; 32]],
            vec![([1u8; 32], vec![4, 5])], // transfer to member 1
            9999999999,
        );

        assert!(matches!(
            proposal.reason,
            ReshareReason::RemoveMembers { .. }
        ));
    }

    #[test]
    fn test_reshare_threshold_change() {
        let proposal = ReshareProposal::change_threshold(67, 51, 9999999999);

        assert!(matches!(
            proposal.reason,
            ReshareReason::ThresholdChange { old: 67, new: 51 }
        ));
        assert_eq!(proposal.new_threshold, Some(51));
    }
}
