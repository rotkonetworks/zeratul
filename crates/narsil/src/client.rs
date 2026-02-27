//! narsil client for syndicate participation
//!
//! provides a unified interface for:
//! - local storage of shares and state
//! - proposal creation and submission
//! - vote casting
//! - osst contribution
//!
//! # storage layout
//!
//! ```text
//! ~/.narsil/
//! ├── syndicates/
//! │   └── {syndicate_id}/
//! │       ├── state.borsh       # replicated syndicate state
//! │       ├── my_shares.enc     # encrypted osst shares
//! │       ├── backup_shares/    # vss backup shares for others
//! │       └── messages/         # pending messages
//! └── identity.enc              # member identity (encrypted)
//! ```

use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;

use crate::crypto::{SyndicateCrypto, MemberCrypto, EncryptedMessage, DirectMessage, generate_nonce};
use crate::mailbox::{MailboxId, BroadcastTopic, SyndicateRouter};
use crate::replay::ReplayValidator;
use crate::state::SyndicateStateManager;
use crate::vss::VerifiableSharePackage;
use crate::wire::{
    Hash32, ShareId, ProposalId, Envelope, MessagePayload,
    SignedProposal, Proposal, ProposalKind, SignedVote, Vote, VoteType,
    SignedContribution, Contribution as WireContribution,
};

/// member's local storage
#[derive(Clone, Debug)]
pub struct MemberStorage {
    /// syndicate id
    pub syndicate_id: Hash32,
    /// member's public key
    pub my_pubkey: Hash32,
    /// member's viewing key (for mailbox derivation)
    pub my_viewing_key: [u8; 32],
    /// shares we own
    pub my_shares: Vec<ShareId>,
    /// encrypted osst share data
    pub encrypted_shares: Vec<u8>,
    /// backup shares we hold for others
    pub backup_shares: Vec<VerifiableSharePackage>,
    /// current sequence number (for replay protection)
    pub sequence: u64,
}

impl MemberStorage {
    /// create new storage for a syndicate
    pub fn new(
        syndicate_id: Hash32,
        my_pubkey: Hash32,
        my_viewing_key: [u8; 32],
        my_shares: Vec<ShareId>,
    ) -> Self {
        Self {
            syndicate_id,
            my_pubkey,
            my_viewing_key,
            my_shares,
            encrypted_shares: Vec::new(),
            backup_shares: Vec::new(),
            sequence: 0,
        }
    }

    /// derive our mailbox id
    pub fn mailbox(&self) -> MailboxId {
        MailboxId::derive(&self.my_viewing_key, &self.syndicate_id)
    }

    /// get next sequence number and increment
    pub fn next_sequence(&mut self) -> u64 {
        self.sequence += 1;
        self.sequence
    }

    /// store encrypted osst shares
    pub fn store_shares(&mut self, encrypted: Vec<u8>) {
        self.encrypted_shares = encrypted;
    }

    /// add backup share for another member
    pub fn add_backup(&mut self, package: VerifiableSharePackage) {
        self.backup_shares.push(package);
    }
}

/// proposal builder
pub struct ProposalBuilder {
    kind: ProposalKind,
    title: String,
    description: String,
    threshold_required: u8,
    deadline: u64,
    action_data: Vec<u8>,
}

impl ProposalBuilder {
    /// create signaling proposal (no on-chain effect)
    pub fn signaling(title: impl Into<String>) -> Self {
        Self {
            kind: ProposalKind::Signaling,
            title: title.into(),
            description: String::new(),
            threshold_required: 51, // simple majority
            deadline: 0,
            action_data: Vec::new(),
        }
    }

    /// create action proposal (penumbra transaction)
    pub fn action(title: impl Into<String>, action_data: Vec<u8>) -> Self {
        Self {
            kind: ProposalKind::Action,
            title: title.into(),
            description: String::new(),
            threshold_required: 67, // supermajority
            deadline: 0,
            action_data,
        }
    }

    /// create amendment proposal (change syndicate parameters)
    pub fn amendment(title: impl Into<String>) -> Self {
        Self {
            kind: ProposalKind::Amendment,
            title: title.into(),
            description: String::new(),
            threshold_required: 75, // 3/4 majority
            deadline: 0,
            action_data: Vec::new(),
        }
    }

    /// create emergency proposal (time-sensitive)
    pub fn emergency(title: impl Into<String>, action_data: Vec<u8>) -> Self {
        Self {
            kind: ProposalKind::Emergency,
            title: title.into(),
            description: String::new(),
            threshold_required: 51, // simple majority
            deadline: 0,
            action_data,
        }
    }

    /// set description
    pub fn with_description(mut self, description: impl Into<String>) -> Self {
        self.description = description.into();
        self
    }

    /// set custom threshold
    pub fn with_threshold(mut self, threshold: u8) -> Self {
        self.threshold_required = threshold;
        self
    }

    /// set deadline
    pub fn with_deadline(mut self, deadline: u64) -> Self {
        self.deadline = deadline;
        self
    }

    /// build into wire proposal
    pub fn build(self, id: ProposalId) -> Proposal {
        Proposal {
            id,
            kind: self.kind,
            title: self.title,
            description: self.description,
            threshold_required: self.threshold_required,
            deadline: self.deadline,
            action_data: self.action_data,
        }
    }
}

/// syndicate client
///
/// main interface for participating in a syndicate
pub struct SyndicateClient {
    /// local storage
    storage: MemberStorage,
    /// state manager
    state: SyndicateStateManager,
    /// replay validator
    replay: ReplayValidator,
    /// member crypto (signing, encryption)
    crypto: MemberCrypto,
    /// group crypto (syndicate-wide encryption)
    group_crypto: SyndicateCrypto,
    /// router for addressing
    router: SyndicateRouter,
}

impl SyndicateClient {
    /// create new client
    pub fn new(
        storage: MemberStorage,
        state: SyndicateStateManager,
        my_secret: [u8; 32],
        viewing_key: [u8; 32],
    ) -> Self {
        let syndicate_id = *state.syndicate_id();
        let state_hash = state.state_hash();

        Self {
            crypto: MemberCrypto::new(my_secret, syndicate_id),
            group_crypto: SyndicateCrypto::new(syndicate_id, &viewing_key, state.epoch()),
            replay: ReplayValidator::new(state_hash),
            router: SyndicateRouter::new(syndicate_id),
            storage,
            state,
        }
    }

    /// get our public key
    pub fn pubkey(&self) -> &Hash32 {
        &self.storage.my_pubkey
    }

    /// get our shares
    pub fn shares(&self) -> &[ShareId] {
        &self.storage.my_shares
    }

    /// get our mailbox
    pub fn mailbox(&self) -> MailboxId {
        self.storage.mailbox()
    }

    /// get broadcast topic
    pub fn broadcast_topic(&self) -> BroadcastTopic {
        self.router.broadcast
    }

    /// create and sign a proposal
    pub fn create_proposal<R: rand_core::RngCore>(
        &mut self,
        builder: ProposalBuilder,
        rng: &mut R,
    ) -> (Envelope, ProposalId) {
        // get next proposal id from state
        let id = self.state.submit_proposal(
            builder.kind.clone(),
            builder.title.clone(),
            builder.description.clone(),
            builder.threshold_required,
            builder.deadline,
            builder.action_data.clone(),
        );

        let proposal = builder.build(id);

        // sign proposal
        let proposal_bytes = serialize_proposal(&proposal);
        let signature = self.crypto.sign(&proposal_bytes);

        let signed = SignedProposal {
            proposal,
            proposer_pubkey: self.storage.my_pubkey,
            signature,
        };

        // build envelope
        let envelope = self.build_envelope(
            MessagePayload::Proposal(signed),
            rng,
        );

        (envelope, id)
    }

    /// create and sign a vote
    pub fn create_vote<R: rand_core::RngCore>(
        &mut self,
        proposal_id: ProposalId,
        vote_type: VoteType,
        share_ids: Vec<ShareId>,
        rng: &mut R,
    ) -> Result<Envelope, ClientError> {
        // verify we own these shares
        for &sid in &share_ids {
            if !self.storage.my_shares.contains(&sid) {
                return Err(ClientError::NotShareOwner { share_id: sid });
            }
        }

        // record votes in state
        for &sid in &share_ids {
            self.state.record_vote(proposal_id, sid, vote_type, &self.storage.my_pubkey)
                .map_err(|e| ClientError::StateError(format!("{:?}", e)))?;
        }

        let vote = Vote {
            proposal_id,
            vote_type,
            share_ids,
        };

        // sign vote
        let vote_bytes = serialize_vote(&vote);
        let signature = self.crypto.sign(&vote_bytes);

        let signed = SignedVote {
            vote,
            voter_pubkey: self.storage.my_pubkey,
            signature,
        };

        let envelope = self.build_envelope(
            MessagePayload::Vote(signed),
            rng,
        );

        Ok(envelope)
    }

    /// create osst contribution for approved proposal
    pub fn create_contribution<R: rand_core::RngCore>(
        &mut self,
        proposal_id: ProposalId,
        osst_data: Vec<u8>,
        rng: &mut R,
    ) -> Envelope {
        let contribution = WireContribution {
            proposal_id,
            share_ids: self.storage.my_shares.clone(),
            osst_data,
        };

        // sign contribution
        let contrib_bytes = serialize_contribution(&contribution);
        let signature = self.crypto.sign(&contrib_bytes);

        let signed = SignedContribution {
            contribution,
            contributor_pubkey: self.storage.my_pubkey,
            signature,
        };

        self.build_envelope(
            MessagePayload::Contribution(signed),
            rng,
        )
    }

    /// encrypt message for broadcast to all members
    pub fn encrypt_broadcast<R: rand_core::RngCore>(
        &self,
        plaintext: &[u8],
        rng: &mut R,
    ) -> EncryptedMessage {
        let nonce = generate_nonce(rng);
        EncryptedMessage::seal(&self.group_crypto, plaintext, nonce)
    }

    /// encrypt message for specific member
    pub fn encrypt_direct<R: rand_core::RngCore>(
        &self,
        recipient: &Hash32,
        plaintext: &[u8],
        rng: &mut R,
    ) -> DirectMessage {
        let nonce = generate_nonce(rng);
        DirectMessage::seal(&self.crypto, recipient, plaintext, nonce)
    }

    /// build envelope with replay protection
    fn build_envelope<R: rand_core::RngCore>(
        &mut self,
        payload: MessagePayload,
        _rng: &mut R,
    ) -> Envelope {
        let sequence = self.storage.next_sequence();
        let state_hash = self.state.state_hash();

        // sign the envelope contents
        let mut signing_data = Vec::new();
        signing_data.push(1u8); // version
        signing_data.extend_from_slice(&self.storage.syndicate_id);
        signing_data.extend_from_slice(&state_hash);
        signing_data.extend_from_slice(&sequence.to_le_bytes());

        let sig = self.crypto.sign(&signing_data);

        Envelope {
            version: 1,
            syndicate_id: self.storage.syndicate_id,
            state_hash,
            sequence,
            payload,
            signature: sig,
        }
    }

    /// validate incoming envelope
    pub fn validate_envelope(&self, envelope: &Envelope, sender: &Hash32) -> Result<(), ClientError> {
        match self.replay.validate(envelope, sender) {
            crate::replay::ReplayCheck::Valid => Ok(()),
            crate::replay::ReplayCheck::StaleState { expected, got } => {
                Err(ClientError::StaleState { expected, got })
            }
            crate::replay::ReplayCheck::DuplicateSequence { sender, got, .. } => {
                Err(ClientError::DuplicateMessage { sender, sequence: got })
            }
            crate::replay::ReplayCheck::FutureSequence { .. } => {
                Err(ClientError::FutureMessage)
            }
            crate::replay::ReplayCheck::UnknownSender { pubkey } => {
                Err(ClientError::UnknownSender { pubkey })
            }
        }
    }
}

/// client errors
#[derive(Clone, Debug)]
pub enum ClientError {
    /// not owner of share
    NotShareOwner { share_id: ShareId },
    /// state operation failed
    StateError(String),
    /// stale state in message
    StaleState { expected: Hash32, got: Hash32 },
    /// duplicate message
    DuplicateMessage { sender: Hash32, sequence: u64 },
    /// message from future
    FutureMessage,
    /// unknown sender
    UnknownSender { pubkey: Hash32 },
}

impl core::fmt::Display for ClientError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::NotShareOwner { share_id } => write!(f, "not owner of share {}", share_id),
            Self::StateError(e) => write!(f, "state error: {}", e),
            Self::StaleState { .. } => write!(f, "stale state in message"),
            Self::DuplicateMessage { sequence, .. } => write!(f, "duplicate message seq {}", sequence),
            Self::FutureMessage => write!(f, "message from future"),
            Self::UnknownSender { .. } => write!(f, "unknown sender"),
        }
    }
}

// serialization helpers (simplified - real impl uses borsh)
fn serialize_proposal(p: &Proposal) -> Vec<u8> {
    let mut buf = Vec::new();
    buf.extend_from_slice(&p.id.to_le_bytes());
    buf.extend_from_slice(p.title.as_bytes());
    buf.extend_from_slice(&p.action_data);
    buf
}

fn serialize_vote(v: &Vote) -> Vec<u8> {
    let mut buf = Vec::new();
    buf.extend_from_slice(&v.proposal_id.to_le_bytes());
    buf.push(match v.vote_type {
        VoteType::Yes => 1,
        VoteType::No => 2,
        VoteType::Abstain => 3,
    });
    for sid in &v.share_ids {
        buf.push(*sid);
    }
    buf
}

fn serialize_contribution(c: &WireContribution) -> Vec<u8> {
    let mut buf = Vec::new();
    buf.extend_from_slice(&c.proposal_id.to_le_bytes());
    for sid in &c.share_ids {
        buf.push(*sid);
    }
    buf.extend_from_slice(&c.osst_data);
    buf
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::wire::GovernanceRules;

    fn setup_client() -> SyndicateClient {
        let syndicate_id = [1u8; 32];
        let my_pubkey = [2u8; 32];
        let my_viewing_key = [3u8; 32];
        let my_secret = [4u8; 32];

        let storage = MemberStorage::new(
            syndicate_id,
            my_pubkey,
            my_viewing_key,
            vec![1, 2, 3], // own shares 1, 2, 3
        );

        let mut state = SyndicateStateManager::new(syndicate_id, GovernanceRules::default());
        state.add_member(my_pubkey, "alice".into(), vec![1, 2, 3]).unwrap();

        SyndicateClient::new(storage, state, my_secret, my_viewing_key)
    }

    #[test]
    fn test_create_proposal() {
        let mut client = setup_client();
        let mut rng = rand::thread_rng();

        let builder = ProposalBuilder::signaling("test proposal")
            .with_description("this is a test");

        let (envelope, id) = client.create_proposal(builder, &mut rng);

        assert_eq!(id, 1);
        assert_eq!(envelope.version, 1);
        assert!(matches!(envelope.payload, MessagePayload::Proposal(_)));
    }

    #[test]
    fn test_create_vote() {
        let mut client = setup_client();
        let mut rng = rand::thread_rng();

        // create proposal first
        let builder = ProposalBuilder::signaling("vote test");
        let (_, proposal_id) = client.create_proposal(builder, &mut rng);

        // vote with our shares
        let envelope = client.create_vote(
            proposal_id,
            VoteType::Yes,
            vec![1, 2],
            &mut rng,
        ).unwrap();

        assert!(matches!(envelope.payload, MessagePayload::Vote(_)));
    }

    #[test]
    fn test_vote_not_owner() {
        let mut client = setup_client();
        let mut rng = rand::thread_rng();

        // create proposal
        let builder = ProposalBuilder::signaling("test");
        let (_, proposal_id) = client.create_proposal(builder, &mut rng);

        // try to vote with share we don't own
        let result = client.create_vote(
            proposal_id,
            VoteType::Yes,
            vec![99], // we don't own share 99
            &mut rng,
        );

        assert!(matches!(result, Err(ClientError::NotShareOwner { .. })));
    }

    #[test]
    fn test_sequence_increment() {
        let mut client = setup_client();
        let mut rng = rand::thread_rng();

        let builder1 = ProposalBuilder::signaling("first");
        let (env1, _) = client.create_proposal(builder1, &mut rng);

        let builder2 = ProposalBuilder::signaling("second");
        let (env2, _) = client.create_proposal(builder2, &mut rng);

        assert!(env2.sequence > env1.sequence);
    }

    #[test]
    fn test_encryption_roundtrip() {
        let client = setup_client();
        let mut rng = rand::thread_rng();

        let plaintext = b"secret syndicate message";
        let encrypted = client.encrypt_broadcast(plaintext, &mut rng);

        // decrypt with same group crypto
        let decrypted = encrypted.open(&client.group_crypto).unwrap();
        assert_eq!(decrypted.as_slice(), plaintext);
    }

    #[test]
    fn test_member_storage() {
        let syndicate_id = [1u8; 32];
        let my_pubkey = [2u8; 32];
        let my_viewing_key = [3u8; 32];

        let storage = MemberStorage::new(
            syndicate_id,
            my_pubkey,
            my_viewing_key,
            vec![1, 2, 3],
        );

        let mailbox = storage.mailbox();
        assert_eq!(mailbox, MailboxId::derive(&my_viewing_key, &syndicate_id));
    }
}
