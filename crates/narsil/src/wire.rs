//! wire formats for narsil relay-based coordination
//!
//! all types are borsh-serializable for deterministic encoding.
//! messages are wrapped in envelopes for relay distribution.

use alloc::string::String;
use alloc::vec::Vec;

#[cfg(feature = "borsh")]
use borsh::{BorshDeserialize, BorshSerialize};

/// share identifier (1-100)
pub type ShareId = u8;

/// proposal identifier
pub type ProposalId = u64;

/// 32-byte hash
pub type Hash32 = [u8; 32];

/// 64-byte signature
pub type Signature64 = [u8; 64];

/// envelope wrapping all relay messages
///
/// contains metadata for replay protection and routing
#[derive(Clone, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "borsh", derive(BorshSerialize, BorshDeserialize))]
pub struct Envelope {
    /// protocol version
    pub version: u8,
    /// syndicate identifier
    pub syndicate_id: Hash32,
    /// state hash this message is valid for
    pub state_hash: Hash32,
    /// monotonic sequence number
    pub sequence: u64,
    /// message payload
    pub payload: MessagePayload,
    /// sender's signature over (version, syndicate_id, state_hash, sequence, payload_hash)
    pub signature: Signature64,
}

impl Envelope {
    /// compute the signing payload
    pub fn signing_payload(&self) -> Vec<u8> {
        let mut buf = Vec::new();
        buf.push(self.version);
        buf.extend_from_slice(&self.syndicate_id);
        buf.extend_from_slice(&self.state_hash);
        buf.extend_from_slice(&self.sequence.to_le_bytes());
        // hash the payload for signing (avoid double-serialization)
        let payload_hash = sha2_hash(&self.payload_bytes());
        buf.extend_from_slice(&payload_hash);
        buf
    }

    /// serialize payload to bytes
    #[cfg(feature = "borsh")]
    pub fn payload_bytes(&self) -> Vec<u8> {
        borsh::to_vec(&self.payload).unwrap_or_default()
    }

    #[cfg(not(feature = "borsh"))]
    pub fn payload_bytes(&self) -> Vec<u8> {
        Vec::new()
    }
}

/// message payload types
#[derive(Clone, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "borsh", derive(BorshSerialize, BorshDeserialize))]
pub enum MessagePayload {
    /// proposal for syndicate action
    Proposal(SignedProposal),
    /// vote on a proposal
    Vote(SignedVote),
    /// OSST contribution for approved proposal
    Contribution(SignedContribution),
    /// state sync request
    SyncRequest(SyncRequest),
    /// state sync response
    SyncResponse(SyncResponse),
}

/// a signed proposal
#[derive(Clone, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "borsh", derive(BorshSerialize, BorshDeserialize))]
pub struct SignedProposal {
    /// proposal content
    pub proposal: Proposal,
    /// proposer's public key
    pub proposer_pubkey: Hash32,
    /// signature over proposal
    pub signature: Signature64,
}

/// proposal content
#[derive(Clone, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "borsh", derive(BorshSerialize, BorshDeserialize))]
pub struct Proposal {
    /// unique proposal id
    pub id: ProposalId,
    /// proposal kind
    pub kind: ProposalKind,
    /// human-readable title (max 80 chars)
    pub title: String,
    /// detailed description
    pub description: String,
    /// shares required to pass (e.g., 67 for 67%)
    pub threshold_required: u8,
    /// deadline (unix timestamp or block height)
    pub deadline: u64,
    /// action data (serialized action plan if applicable)
    pub action_data: Vec<u8>,
}

/// proposal kinds
#[derive(Clone, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "borsh", derive(BorshSerialize, BorshDeserialize))]
pub enum ProposalKind {
    /// signaling only, no on-chain effect
    Signaling,
    /// penumbra transaction (spend, swap, etc.)
    Action,
    /// change syndicate parameters
    Amendment,
    /// time-sensitive action
    Emergency,
    /// formal text resolution
    Text,
}

/// vote types
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "borsh", derive(BorshSerialize, BorshDeserialize))]
pub enum VoteType {
    Yes,
    No,
    Abstain,
}

/// a signed vote
#[derive(Clone, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "borsh", derive(BorshSerialize, BorshDeserialize))]
pub struct SignedVote {
    /// vote content
    pub vote: Vote,
    /// voter's public key
    pub voter_pubkey: Hash32,
    /// signature over vote
    pub signature: Signature64,
}

/// vote content
#[derive(Clone, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "borsh", derive(BorshSerialize, BorshDeserialize))]
pub struct Vote {
    /// proposal being voted on
    pub proposal_id: ProposalId,
    /// vote decision
    pub vote_type: VoteType,
    /// share ids voting (voter may hold multiple shares)
    pub share_ids: Vec<ShareId>,
}

/// a signed OSST contribution
#[derive(Clone, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "borsh", derive(BorshSerialize, BorshDeserialize))]
pub struct SignedContribution {
    /// contribution content
    pub contribution: Contribution,
    /// contributor's public key
    pub contributor_pubkey: Hash32,
    /// signature over contribution
    pub signature: Signature64,
}

/// OSST contribution content
#[derive(Clone, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "borsh", derive(BorshSerialize, BorshDeserialize))]
pub struct Contribution {
    /// proposal this contribution is for
    pub proposal_id: ProposalId,
    /// share ids contributing (may be batched)
    pub share_ids: Vec<ShareId>,
    /// serialized OSST contribution data (R, z values)
    /// for batched: already combined locally
    pub osst_data: Vec<u8>,
}

/// state sync request
#[derive(Clone, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "borsh", derive(BorshSerialize, BorshDeserialize))]
pub struct SyncRequest {
    /// requester's current state hash
    pub current_state_hash: Hash32,
    /// requester's current sequence
    pub current_sequence: u64,
}

/// state sync response
#[derive(Clone, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "borsh", derive(BorshSerialize, BorshDeserialize))]
pub struct SyncResponse {
    /// current state (full serialized SyndicateState)
    pub state: Vec<u8>,
    /// state hash
    pub state_hash: Hash32,
    /// current sequence
    pub sequence: u64,
}

/// syndicate state (the full replicated state)
#[derive(Clone, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "borsh", derive(BorshSerialize, BorshDeserialize))]
pub struct SyndicateState {
    /// syndicate identifier
    pub syndicate_id: Hash32,
    /// current epoch (increments on reshare)
    pub epoch: u64,
    /// share ownership: share_id -> owner pubkey
    pub shares: Vec<ShareOwnership>,
    /// governance rules
    pub rules: GovernanceRules,
    /// active proposals
    pub proposals: Vec<Proposal>,
    /// votes on active proposals
    pub votes: Vec<RecordedVote>,
    /// member directory
    pub members: Vec<MemberInfo>,
    /// current sequence number
    pub sequence: u64,
}

/// share ownership record
#[derive(Clone, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "borsh", derive(BorshSerialize, BorshDeserialize))]
pub struct ShareOwnership {
    /// share id (1-100)
    pub share_id: ShareId,
    /// owner's public key
    pub owner_pubkey: Hash32,
}

/// governance rules
#[derive(Clone, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "borsh", derive(BorshSerialize, BorshDeserialize))]
pub struct GovernanceRules {
    /// shares needed for routine actions (e.g., 51)
    pub routine_threshold: u8,
    /// shares needed for major actions (e.g., 67)
    pub major_threshold: u8,
    /// shares needed for amendments (e.g., 75)
    pub amendment_threshold: u8,
    /// shares needed for existential actions (e.g., 90)
    pub existential_threshold: u8,
    /// shares needed for quorum (e.g., 51)
    pub quorum: u8,
}

impl Default for GovernanceRules {
    fn default() -> Self {
        Self {
            routine_threshold: 51,
            major_threshold: 67,
            amendment_threshold: 75,
            existential_threshold: 90,
            quorum: 51,
        }
    }
}

/// recorded vote in state
#[derive(Clone, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "borsh", derive(BorshSerialize, BorshDeserialize))]
pub struct RecordedVote {
    /// proposal id
    pub proposal_id: ProposalId,
    /// share id that voted
    pub share_id: ShareId,
    /// vote type
    pub vote_type: VoteType,
}

/// member information
#[derive(Clone, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "borsh", derive(BorshSerialize, BorshDeserialize))]
pub struct MemberInfo {
    /// member's public key
    pub pubkey: Hash32,
    /// human-readable name (optional)
    pub name: String,
    /// shares owned by this member
    pub shares: Vec<ShareId>,
}

/// helper: compute sha256 hash
fn sha2_hash(data: &[u8]) -> Hash32 {
    use sha2::{Digest, Sha256};
    Sha256::digest(data).into()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_governance_rules_default() {
        let rules = GovernanceRules::default();
        assert_eq!(rules.routine_threshold, 51);
        assert_eq!(rules.major_threshold, 67);
        assert_eq!(rules.amendment_threshold, 75);
        assert_eq!(rules.existential_threshold, 90);
        assert_eq!(rules.quorum, 51);
    }

    #[test]
    fn test_vote_type() {
        assert_ne!(VoteType::Yes, VoteType::No);
        assert_ne!(VoteType::Yes, VoteType::Abstain);
    }

    #[cfg(feature = "borsh")]
    #[test]
    fn test_proposal_roundtrip() {
        let proposal = Proposal {
            id: 42,
            kind: ProposalKind::Signaling,
            title: "test proposal".into(),
            description: "this is a test".into(),
            threshold_required: 51,
            deadline: 1234567890,
            action_data: vec![1, 2, 3],
        };

        let bytes = borsh::to_vec(&proposal).unwrap();
        let decoded: Proposal = borsh::from_slice(&bytes).unwrap();

        assert_eq!(proposal, decoded);
    }

    #[cfg(feature = "borsh")]
    #[test]
    fn test_syndicate_state_roundtrip() {
        let state = SyndicateState {
            syndicate_id: [1u8; 32],
            epoch: 0,
            shares: vec![
                ShareOwnership { share_id: 1, owner_pubkey: [2u8; 32] },
                ShareOwnership { share_id: 2, owner_pubkey: [2u8; 32] },
            ],
            rules: GovernanceRules::default(),
            proposals: vec![],
            votes: vec![],
            members: vec![
                MemberInfo {
                    pubkey: [2u8; 32],
                    name: "alice".into(),
                    shares: vec![1, 2],
                },
            ],
            sequence: 0,
        };

        let bytes = borsh::to_vec(&state).unwrap();
        let decoded: SyndicateState = borsh::from_slice(&bytes).unwrap();

        assert_eq!(state, decoded);
    }

    #[cfg(feature = "borsh")]
    #[test]
    fn test_envelope_signing_payload() {
        let envelope = Envelope {
            version: 1,
            syndicate_id: [1u8; 32],
            state_hash: [2u8; 32],
            sequence: 42,
            payload: MessagePayload::SyncRequest(SyncRequest {
                current_state_hash: [3u8; 32],
                current_sequence: 10,
            }),
            signature: [0u8; 64],
        };

        let payload = envelope.signing_payload();
        // version(1) + syndicate_id(32) + state_hash(32) + sequence(8) + payload_hash(32)
        assert_eq!(payload.len(), 1 + 32 + 32 + 8 + 32);
    }
}
