//! replicated state machine for syndicate operations
//!
//! the syndicate maintains internal state (balances, pending actions, etc.)
//! that is replicated across all members via bft consensus.
//!
//! only state roots and proofs are posted to L1 - the actual state
//! stays within the syndicate.
//!
//! # state manager
//!
//! `SyndicateStateManager` wraps `SyndicateState` (from wire.rs) and provides:
//! - proposal submission and tracking
//! - vote recording and tally
//! - contribution collection
//! - state transitions with validation

use alloc::string::String;
use alloc::vec::Vec;
use sha2::{Digest, Sha256};

use crate::wire::{
    Hash32, ShareId, ProposalId, Proposal, ProposalKind, VoteType,
    SyndicateState, ShareOwnership, GovernanceRules, RecordedVote, MemberInfo,
};

/// state root (commitment to current state)
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct StateRoot(pub [u8; 32]);

impl StateRoot {
    pub fn from_bytes(bytes: [u8; 32]) -> Self {
        Self(bytes)
    }

    pub fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }

    /// compute state root from raw state bytes
    pub fn compute(state: &[u8]) -> Self {
        let hash: [u8; 32] = Sha256::digest(state).into();
        Self(hash)
    }
}

impl Default for StateRoot {
    fn default() -> Self {
        Self([0u8; 32])
    }
}

/// a state transition to be applied
#[derive(Clone, Debug)]
pub struct StateTransition {
    /// previous state root
    pub prev_root: StateRoot,
    /// new state root after transition
    pub new_root: StateRoot,
    /// transition data (application-specific)
    pub data: Vec<u8>,
    /// nullifiers consumed (prevents replay)
    pub nullifiers: Vec<[u8; 32]>,
}

impl StateTransition {
    pub fn new(prev_root: StateRoot, new_root: StateRoot, data: Vec<u8>) -> Self {
        Self {
            prev_root,
            new_root,
            data,
            nullifiers: Vec::new(),
        }
    }

    pub fn with_nullifiers(mut self, nullifiers: Vec<[u8; 32]>) -> Self {
        self.nullifiers = nullifiers;
        self
    }

    /// serialize for inclusion in bft round payload
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut buf = Vec::new();
        buf.extend_from_slice(&self.prev_root.0);
        buf.extend_from_slice(&self.new_root.0);
        buf.extend_from_slice(&(self.data.len() as u32).to_le_bytes());
        buf.extend_from_slice(&self.data);
        buf.extend_from_slice(&(self.nullifiers.len() as u32).to_le_bytes());
        for nullifier in &self.nullifiers {
            buf.extend_from_slice(nullifier);
        }
        buf
    }

    /// deserialize from bytes
    pub fn from_bytes(bytes: &[u8]) -> Option<Self> {
        if bytes.len() < 64 + 4 {
            return None;
        }

        let prev_root = StateRoot::from_bytes(bytes[0..32].try_into().ok()?);
        let new_root = StateRoot::from_bytes(bytes[32..64].try_into().ok()?);

        let data_len = u32::from_le_bytes(bytes[64..68].try_into().ok()?) as usize;
        if bytes.len() < 68 + data_len + 4 {
            return None;
        }

        let data = bytes[68..68 + data_len].to_vec();

        let nullifier_offset = 68 + data_len;
        let nullifier_count =
            u32::from_le_bytes(bytes[nullifier_offset..nullifier_offset + 4].try_into().ok()?)
                as usize;

        let mut nullifiers = Vec::with_capacity(nullifier_count);
        let mut offset = nullifier_offset + 4;
        for _ in 0..nullifier_count {
            if offset + 32 > bytes.len() {
                return None;
            }
            nullifiers.push(bytes[offset..offset + 32].try_into().ok()?);
            offset += 32;
        }

        Some(Self {
            prev_root,
            new_root,
            data,
            nullifiers,
        })
    }

    /// compute transition hash (for logging/audit)
    pub fn hash(&self) -> [u8; 32] {
        Sha256::digest(&self.to_bytes()).into()
    }
}

/// track consumed nullifiers to prevent replay
#[derive(Clone, Debug, Default)]
pub struct NullifierSet {
    nullifiers: Vec<[u8; 32]>,
}

impl NullifierSet {
    pub fn new() -> Self {
        Self::default()
    }

    /// check if nullifier has been consumed
    pub fn contains(&self, nullifier: &[u8; 32]) -> bool {
        self.nullifiers.contains(nullifier)
    }

    /// add nullifier (returns false if already exists)
    pub fn insert(&mut self, nullifier: [u8; 32]) -> bool {
        if self.contains(&nullifier) {
            return false;
        }
        self.nullifiers.push(nullifier);
        true
    }

    /// add multiple nullifiers, returns false if any exist
    pub fn insert_all(&mut self, nullifiers: &[[u8; 32]]) -> bool {
        // check all first
        for n in nullifiers {
            if self.contains(n) {
                return false;
            }
        }
        // then insert
        for n in nullifiers {
            self.nullifiers.push(*n);
        }
        true
    }

    pub fn len(&self) -> usize {
        self.nullifiers.len()
    }

    pub fn is_empty(&self) -> bool {
        self.nullifiers.is_empty()
    }
}

/// errors from state operations
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum StateError {
    /// proposal id already exists
    ProposalExists { id: ProposalId },
    /// proposal not found
    ProposalNotFound { id: ProposalId },
    /// proposal deadline passed
    ProposalExpired { id: ProposalId },
    /// share already voted on this proposal
    AlreadyVoted { proposal_id: ProposalId, share_id: ShareId },
    /// share not found in registry
    ShareNotFound { share_id: ShareId },
    /// not authorized (share not owned by sender)
    NotAuthorized { share_id: ShareId },
    /// threshold not met
    ThresholdNotMet { required: u8, got: u8 },
    /// member not found
    MemberNotFound { pubkey: Hash32 },
    /// member already exists
    MemberExists { pubkey: Hash32 },
}

impl core::fmt::Display for StateError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::ProposalExists { id } => write!(f, "proposal {} already exists", id),
            Self::ProposalNotFound { id } => write!(f, "proposal {} not found", id),
            Self::ProposalExpired { id } => write!(f, "proposal {} expired", id),
            Self::AlreadyVoted { proposal_id, share_id } => {
                write!(f, "share {} already voted on proposal {}", share_id, proposal_id)
            }
            Self::ShareNotFound { share_id } => write!(f, "share {} not found", share_id),
            Self::NotAuthorized { share_id } => write!(f, "not authorized for share {}", share_id),
            Self::ThresholdNotMet { required, got } => {
                write!(f, "threshold not met: required {}, got {}", required, got)
            }
            Self::MemberNotFound { pubkey } => write!(f, "member {:?} not found", &pubkey[..4]),
            Self::MemberExists { pubkey } => write!(f, "member {:?} already exists", &pubkey[..4]),
        }
    }
}

/// vote tally for a proposal
#[derive(Clone, Debug, Default)]
pub struct VoteTally {
    pub yes: u8,
    pub no: u8,
    pub abstain: u8,
}

impl VoteTally {
    /// total shares that voted
    pub fn total(&self) -> u8 {
        self.yes.saturating_add(self.no).saturating_add(self.abstain)
    }

    /// check if yes votes meet threshold
    pub fn meets_threshold(&self, threshold: u8) -> bool {
        self.yes >= threshold
    }
}

/// syndicate state manager
///
/// wraps SyndicateState and provides operations for proposals, votes, etc.
#[derive(Clone, Debug)]
pub struct SyndicateStateManager {
    /// the underlying state
    state: SyndicateState,
    /// consumed nullifiers
    nullifiers: NullifierSet,
    /// next proposal id
    next_proposal_id: ProposalId,
}

impl SyndicateStateManager {
    /// create new manager with initial state
    pub fn new(syndicate_id: Hash32, rules: GovernanceRules) -> Self {
        Self {
            state: SyndicateState {
                syndicate_id,
                epoch: 0,
                shares: Vec::new(),
                rules,
                proposals: Vec::new(),
                votes: Vec::new(),
                members: Vec::new(),
                sequence: 0,
            },
            nullifiers: NullifierSet::new(),
            next_proposal_id: 1,
        }
    }

    /// create from existing state
    pub fn from_state(state: SyndicateState) -> Self {
        let next_id = state.proposals.iter()
            .map(|p| p.id)
            .max()
            .unwrap_or(0) + 1;
        Self {
            state,
            nullifiers: NullifierSet::new(),
            next_proposal_id: next_id,
        }
    }

    /// get the underlying state
    pub fn state(&self) -> &SyndicateState {
        &self.state
    }

    /// get syndicate id
    pub fn syndicate_id(&self) -> &Hash32 {
        &self.state.syndicate_id
    }

    /// get current epoch
    pub fn epoch(&self) -> u64 {
        self.state.epoch
    }

    /// get current sequence
    pub fn sequence(&self) -> u64 {
        self.state.sequence
    }

    /// increment sequence
    pub fn increment_sequence(&mut self) {
        self.state.sequence += 1;
    }

    /// get governance rules
    pub fn rules(&self) -> &GovernanceRules {
        &self.state.rules
    }

    /// add member to syndicate
    pub fn add_member(
        &mut self,
        pubkey: Hash32,
        name: String,
        share_ids: Vec<ShareId>,
    ) -> Result<(), StateError> {
        // check not already member
        if self.state.members.iter().any(|m| m.pubkey == pubkey) {
            return Err(StateError::MemberExists { pubkey });
        }

        // add shares
        for &share_id in &share_ids {
            self.state.shares.push(ShareOwnership {
                share_id,
                owner_pubkey: pubkey,
            });
        }

        // add member
        self.state.members.push(MemberInfo {
            pubkey,
            name,
            shares: share_ids,
        });

        self.increment_sequence();
        Ok(())
    }

    /// get member by pubkey
    pub fn get_member(&self, pubkey: &Hash32) -> Option<&MemberInfo> {
        self.state.members.iter().find(|m| &m.pubkey == pubkey)
    }

    /// get owner of a share
    pub fn share_owner(&self, share_id: ShareId) -> Option<&Hash32> {
        self.state.shares.iter()
            .find(|s| s.share_id == share_id)
            .map(|s| &s.owner_pubkey)
    }

    /// get all shares owned by a pubkey
    pub fn shares_of(&self, pubkey: &Hash32) -> Vec<ShareId> {
        self.state.shares.iter()
            .filter(|s| &s.owner_pubkey == pubkey)
            .map(|s| s.share_id)
            .collect()
    }

    /// count shares owned by pubkey
    pub fn share_count(&self, pubkey: &Hash32) -> u8 {
        self.shares_of(pubkey).len() as u8
    }

    /// submit a new proposal
    pub fn submit_proposal(
        &mut self,
        kind: ProposalKind,
        title: String,
        description: String,
        threshold_required: u8,
        deadline: u64,
        action_data: Vec<u8>,
    ) -> ProposalId {
        let id = self.next_proposal_id;
        self.next_proposal_id += 1;

        self.state.proposals.push(Proposal {
            id,
            kind,
            title,
            description,
            threshold_required,
            deadline,
            action_data,
        });

        self.increment_sequence();
        id
    }

    /// get proposal by id
    pub fn get_proposal(&self, id: ProposalId) -> Option<&Proposal> {
        self.state.proposals.iter().find(|p| p.id == id)
    }

    /// record a vote
    pub fn record_vote(
        &mut self,
        proposal_id: ProposalId,
        share_id: ShareId,
        vote_type: VoteType,
        voter_pubkey: &Hash32,
    ) -> Result<(), StateError> {
        // check proposal exists
        if !self.state.proposals.iter().any(|p| p.id == proposal_id) {
            return Err(StateError::ProposalNotFound { id: proposal_id });
        }

        // check share exists and is owned by voter
        match self.share_owner(share_id) {
            None => return Err(StateError::ShareNotFound { share_id }),
            Some(owner) if owner != voter_pubkey => {
                return Err(StateError::NotAuthorized { share_id })
            }
            _ => {}
        }

        // check not already voted
        if self.state.votes.iter().any(|v| {
            v.proposal_id == proposal_id && v.share_id == share_id
        }) {
            return Err(StateError::AlreadyVoted { proposal_id, share_id });
        }

        // record vote
        self.state.votes.push(RecordedVote {
            proposal_id,
            share_id,
            vote_type,
        });

        self.increment_sequence();
        Ok(())
    }

    /// get vote tally for a proposal
    pub fn tally(&self, proposal_id: ProposalId) -> VoteTally {
        let mut tally = VoteTally::default();
        for vote in &self.state.votes {
            if vote.proposal_id == proposal_id {
                match vote.vote_type {
                    VoteType::Yes => tally.yes += 1,
                    VoteType::No => tally.no += 1,
                    VoteType::Abstain => tally.abstain += 1,
                }
            }
        }
        tally
    }

    /// check if proposal has passed
    pub fn proposal_passed(&self, proposal_id: ProposalId) -> bool {
        if let Some(proposal) = self.get_proposal(proposal_id) {
            let tally = self.tally(proposal_id);
            tally.meets_threshold(proposal.threshold_required)
        } else {
            false
        }
    }

    /// get threshold for action kind
    pub fn threshold_for(&self, kind: &ProposalKind) -> u8 {
        match kind {
            ProposalKind::Signaling => self.state.rules.routine_threshold,
            ProposalKind::Action => self.state.rules.major_threshold,
            ProposalKind::Amendment => self.state.rules.amendment_threshold,
            ProposalKind::Emergency => self.state.rules.routine_threshold,
            ProposalKind::Text => self.state.rules.routine_threshold,
        }
    }

    /// remove passed/expired proposals
    pub fn cleanup_proposals(&mut self, current_time: u64) {
        // remove expired proposals
        self.state.proposals.retain(|p| p.deadline > current_time);

        // cleanup votes for removed proposals
        let active_ids: Vec<ProposalId> = self.state.proposals.iter()
            .map(|p| p.id)
            .collect();
        self.state.votes.retain(|v| active_ids.contains(&v.proposal_id));

        self.increment_sequence();
    }

    /// increment epoch (after reshare)
    pub fn increment_epoch(&mut self) {
        self.state.epoch += 1;
        self.increment_sequence();
    }

    /// compute state hash
    #[cfg(feature = "borsh")]
    pub fn state_hash(&self) -> Hash32 {
        let bytes = borsh::to_vec(&self.state).unwrap_or_default();
        Sha256::digest(&bytes).into()
    }

    #[cfg(not(feature = "borsh"))]
    pub fn state_hash(&self) -> Hash32 {
        let mut hasher = Sha256::new();
        hasher.update(&self.state.syndicate_id);
        hasher.update(&self.state.epoch.to_le_bytes());
        hasher.update(&self.state.sequence.to_le_bytes());
        hasher.finalize().into()
    }

    /// check and record nullifier
    pub fn record_nullifier(&mut self, nullifier: [u8; 32]) -> bool {
        self.nullifiers.insert(nullifier)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_state_root_compute() {
        let state = b"some syndicate state";
        let root = StateRoot::compute(state);
        let root2 = StateRoot::compute(state);
        assert_eq!(root, root2);

        let different = StateRoot::compute(b"different state");
        assert_ne!(root, different);
    }

    #[test]
    fn test_state_transition_roundtrip() {
        let prev = StateRoot::compute(b"prev");
        let new = StateRoot::compute(b"new");
        let transition = StateTransition::new(prev, new, b"transfer".to_vec())
            .with_nullifiers(vec![[1u8; 32], [2u8; 32]]);

        let bytes = transition.to_bytes();
        let recovered = StateTransition::from_bytes(&bytes).unwrap();

        assert_eq!(transition.prev_root, recovered.prev_root);
        assert_eq!(transition.new_root, recovered.new_root);
        assert_eq!(transition.data, recovered.data);
        assert_eq!(transition.nullifiers, recovered.nullifiers);
    }

    #[test]
    fn test_nullifier_set() {
        let mut set = NullifierSet::new();

        let n1 = [1u8; 32];
        let n2 = [2u8; 32];

        assert!(set.insert(n1));
        assert!(!set.insert(n1)); // duplicate
        assert!(set.insert(n2));

        assert!(set.contains(&n1));
        assert!(set.contains(&n2));
        assert!(!set.contains(&[3u8; 32]));
    }

    #[test]
    fn test_nullifier_batch_insert() {
        let mut set = NullifierSet::new();

        let batch = vec![[1u8; 32], [2u8; 32], [3u8; 32]];
        assert!(set.insert_all(&batch));

        // second batch with overlap should fail
        let batch2 = vec![[3u8; 32], [4u8; 32]];
        assert!(!set.insert_all(&batch2));

        // but individual new ones should work
        assert!(set.insert([5u8; 32]));
    }

    #[test]
    fn test_state_manager_basic() {
        let syndicate_id = [1u8; 32];
        let rules = GovernanceRules::default();
        let mut manager = SyndicateStateManager::new(syndicate_id, rules);

        assert_eq!(manager.syndicate_id(), &syndicate_id);
        assert_eq!(manager.epoch(), 0);
        assert_eq!(manager.sequence(), 0);
    }

    #[test]
    fn test_state_manager_add_member() {
        let syndicate_id = [1u8; 32];
        let mut manager = SyndicateStateManager::new(syndicate_id, GovernanceRules::default());

        let alice = [2u8; 32];
        manager.add_member(alice, "alice".into(), vec![1, 2, 3]).unwrap();

        assert_eq!(manager.share_count(&alice), 3);
        assert_eq!(manager.shares_of(&alice), vec![1, 2, 3]);
        assert_eq!(manager.share_owner(1), Some(&alice));

        // duplicate member fails
        let result = manager.add_member(alice, "alice2".into(), vec![4]);
        assert!(matches!(result, Err(StateError::MemberExists { .. })));
    }

    #[test]
    fn test_state_manager_proposal() {
        let syndicate_id = [1u8; 32];
        let mut manager = SyndicateStateManager::new(syndicate_id, GovernanceRules::default());

        let id = manager.submit_proposal(
            ProposalKind::Signaling,
            "test proposal".into(),
            "this is a test".into(),
            51,
            1234567890,
            vec![],
        );

        assert_eq!(id, 1);
        assert!(manager.get_proposal(id).is_some());
        assert_eq!(manager.get_proposal(id).unwrap().title, "test proposal");
    }

    #[test]
    fn test_state_manager_voting() {
        let syndicate_id = [1u8; 32];
        let mut manager = SyndicateStateManager::new(syndicate_id, GovernanceRules::default());

        let alice = [2u8; 32];
        let bob = [3u8; 32];
        manager.add_member(alice, "alice".into(), vec![1, 2, 3]).unwrap();
        manager.add_member(bob, "bob".into(), vec![4, 5]).unwrap();

        let proposal_id = manager.submit_proposal(
            ProposalKind::Signaling,
            "vote test".into(),
            "testing voting".into(),
            3, // need 3 shares to pass
            1234567890,
            vec![],
        );

        // alice votes with share 1
        manager.record_vote(proposal_id, 1, VoteType::Yes, &alice).unwrap();
        let tally = manager.tally(proposal_id);
        assert_eq!(tally.yes, 1);
        assert!(!tally.meets_threshold(3));

        // alice votes with shares 2 and 3
        manager.record_vote(proposal_id, 2, VoteType::Yes, &alice).unwrap();
        manager.record_vote(proposal_id, 3, VoteType::Yes, &alice).unwrap();
        let tally = manager.tally(proposal_id);
        assert_eq!(tally.yes, 3);
        assert!(tally.meets_threshold(3));
        assert!(manager.proposal_passed(proposal_id));

        // double vote fails
        let result = manager.record_vote(proposal_id, 1, VoteType::Yes, &alice);
        assert!(matches!(result, Err(StateError::AlreadyVoted { .. })));

        // bob can vote with own shares
        manager.record_vote(proposal_id, 4, VoteType::No, &bob).unwrap();
        let tally = manager.tally(proposal_id);
        assert_eq!(tally.yes, 3);
        assert_eq!(tally.no, 1);
    }

    #[test]
    fn test_vote_unauthorized() {
        let syndicate_id = [1u8; 32];
        let mut manager = SyndicateStateManager::new(syndicate_id, GovernanceRules::default());

        let alice = [2u8; 32];
        let bob = [3u8; 32];
        manager.add_member(alice, "alice".into(), vec![1, 2]).unwrap();
        manager.add_member(bob, "bob".into(), vec![3, 4]).unwrap();

        let proposal_id = manager.submit_proposal(
            ProposalKind::Signaling,
            "test".into(),
            "test".into(),
            2,
            9999999999,
            vec![],
        );

        // bob tries to vote with alice's share
        let result = manager.record_vote(proposal_id, 1, VoteType::Yes, &bob);
        assert!(matches!(result, Err(StateError::NotAuthorized { .. })));
    }

    #[test]
    fn test_state_hash_changes() {
        let syndicate_id = [1u8; 32];
        let mut manager = SyndicateStateManager::new(syndicate_id, GovernanceRules::default());

        let hash1 = manager.state_hash();

        let alice = [2u8; 32];
        manager.add_member(alice, "alice".into(), vec![1]).unwrap();

        let hash2 = manager.state_hash();
        assert_ne!(hash1, hash2);

        manager.submit_proposal(
            ProposalKind::Signaling,
            "test".into(),
            "test".into(),
            1,
            9999999999,
            vec![],
        );

        let hash3 = manager.state_hash();
        assert_ne!(hash2, hash3);
    }
}
