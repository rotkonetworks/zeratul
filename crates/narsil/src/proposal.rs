//! proposal types inspired by penumbra governance
//!
//! proposals can be:
//! - **signaling**: coordination/direction without on-chain action
//! - **action**: triggers penumbra transaction (requires OSST)
//! - **amendment**: changes syndicate rules/membership
//! - **emergency**: time-sensitive, lower threshold
//!
//! signaling proposals are key for syndicate governance - they let
//! members reach consensus on direction without spending anything.

use alloc::string::String;
use alloc::vec::Vec;
use sha2::{Digest, Sha256};

use crate::governance::ActionType;

/// vote on a proposal
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Vote {
    /// approve the proposal
    Yes,
    /// reject the proposal
    No,
    /// abstain (counts toward quorum but not threshold)
    Abstain,
}

impl Vote {
    pub fn as_str(&self) -> &'static str {
        match self {
            Vote::Yes => "yes",
            Vote::No => "no",
            Vote::Abstain => "abstain",
        }
    }
}

/// proposal kinds (inspired by penumbra governance)
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ProposalKind {
    /// signaling: coordination only, no on-chain effect
    Signaling,
    /// action: triggers penumbra transaction
    Action,
    /// amendment: changes syndicate rules
    Amendment,
    /// emergency: time-sensitive, expedited voting
    Emergency,
}

impl ProposalKind {
    /// what governance level does this kind require?
    pub fn action_type(&self) -> ActionType {
        match self {
            ProposalKind::Signaling => ActionType::Routine,
            ProposalKind::Action => ActionType::Major, // depends on action
            ProposalKind::Amendment => ActionType::Amendment,
            ProposalKind::Emergency => ActionType::Major, // but faster
        }
    }
}

/// proposal payload - what the proposal actually does
#[derive(Clone, Debug)]
pub enum ProposalPayload {
    /// signaling: just coordination, no automatic effect
    /// can reference a document, commit hash, or external resource
    Signaling {
        /// optional reference (URL, IPFS hash, commit, etc)
        reference: Option<String>,
    },

    /// action: execute a penumbra transaction
    Action {
        /// serialized action plan
        action_plan: Vec<u8>,
    },

    /// parameter change: modify syndicate settings
    ParameterChange {
        /// which parameter to change
        parameter: ParameterKind,
        /// new value (encoded)
        new_value: Vec<u8>,
    },

    /// membership change: add or remove member
    MembershipChange {
        /// the change to make
        change: MembershipChangeKind,
    },

    /// emergency: time-sensitive action
    Emergency {
        /// optional action to execute if passed
        action_plan: Option<Vec<u8>>,
        /// reason for emergency
        reason: String,
    },

    /// text: human-readable resolution (like signaling but more formal)
    Text {
        /// the resolution text
        text: String,
    },
}

/// parameters that can be changed
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ParameterKind {
    /// change voting threshold for routine actions
    RoutineThreshold,
    /// change voting threshold for major actions
    MajorThreshold,
    /// change voting threshold for amendments
    AmendmentThreshold,
    /// change quorum requirement
    QuorumPercentage,
    /// enable/disable transfers
    TransfersAllowed,
}

/// membership changes
#[derive(Clone, Debug)]
pub enum MembershipChangeKind {
    /// add new member with share allocation
    AddMember {
        /// new member's public key
        pubkey: [u8; 32],
        /// shares to allocate
        shares: u32,
        /// where shares come from (dilution or transfer)
        source: ShareSource,
    },
    /// remove member (buyout)
    RemoveMember {
        /// member to remove
        member_id: u32,
        /// buyout terms
        buyout: BuyoutTerms,
    },
    /// transfer shares between members
    TransferShares {
        from: u32,
        to: u32,
        shares: u32,
    },
}

/// where new shares come from
#[derive(Clone, Debug)]
pub enum ShareSource {
    /// dilute existing members proportionally
    Dilution,
    /// transfer from specific member
    Transfer { from: u32 },
    /// from treasury/reserve (if any unissued)
    Reserve,
}

/// buyout terms for member exit
#[derive(Clone, Debug)]
pub struct BuyoutTerms {
    /// amount to pay (in native asset)
    pub amount: u128,
    /// payment address
    pub payment_address: [u8; 80],
}

/// a complete proposal
#[derive(Clone, Debug)]
pub struct SyndicateProposal {
    /// unique proposal id
    pub id: u64,
    /// short title (max 80 chars like penumbra)
    pub title: String,
    /// detailed description
    pub description: String,
    /// the proposal payload
    pub payload: ProposalPayload,
    /// proposer's member id
    pub proposer: u32,
    /// block height when proposed
    pub proposed_at: u64,
    /// voting deadline (height or timestamp)
    pub deadline: u64,
}

impl SyndicateProposal {
    pub fn new(
        id: u64,
        title: String,
        description: String,
        payload: ProposalPayload,
        proposer: u32,
        proposed_at: u64,
        deadline: u64,
    ) -> Self {
        Self {
            id,
            title,
            description,
            payload,
            proposer,
            proposed_at,
            deadline,
        }
    }

    /// create signaling proposal (coordination only)
    pub fn signaling(
        id: u64,
        title: String,
        description: String,
        reference: Option<String>,
        proposer: u32,
        proposed_at: u64,
        deadline: u64,
    ) -> Self {
        Self::new(
            id,
            title,
            description,
            ProposalPayload::Signaling { reference },
            proposer,
            proposed_at,
            deadline,
        )
    }

    /// create text proposal (formal resolution)
    pub fn text(
        id: u64,
        title: String,
        resolution_text: String,
        proposer: u32,
        proposed_at: u64,
        deadline: u64,
    ) -> Self {
        Self::new(
            id,
            title,
            resolution_text.clone(),
            ProposalPayload::Text { text: resolution_text },
            proposer,
            proposed_at,
            deadline,
        )
    }

    /// get proposal kind
    pub fn kind(&self) -> ProposalKind {
        match &self.payload {
            ProposalPayload::Signaling { .. } => ProposalKind::Signaling,
            ProposalPayload::Action { .. } => ProposalKind::Action,
            ProposalPayload::ParameterChange { .. } => ProposalKind::Amendment,
            ProposalPayload::MembershipChange { .. } => ProposalKind::Amendment,
            ProposalPayload::Emergency { .. } => ProposalKind::Emergency,
            ProposalPayload::Text { .. } => ProposalKind::Signaling,
        }
    }

    /// what approval level is required?
    pub fn required_approval(&self) -> ActionType {
        self.kind().action_type()
    }

    /// does this proposal trigger an on-chain action?
    pub fn has_on_chain_effect(&self) -> bool {
        match &self.payload {
            ProposalPayload::Signaling { .. } => false,
            ProposalPayload::Text { .. } => false,
            ProposalPayload::Action { .. } => true,
            ProposalPayload::ParameterChange { .. } => false, // internal only
            ProposalPayload::MembershipChange { .. } => false, // internal only
            ProposalPayload::Emergency { action_plan, .. } => action_plan.is_some(),
        }
    }

    /// compute proposal hash for signing
    pub fn hash(&self) -> [u8; 32] {
        let mut hasher = Sha256::new();
        hasher.update(b"narsil-proposal-v1");
        hasher.update(self.id.to_le_bytes());
        hasher.update((self.title.len() as u32).to_le_bytes());
        hasher.update(self.title.as_bytes());
        hasher.update((self.description.len() as u32).to_le_bytes());
        hasher.update(self.description.as_bytes());
        hasher.update(self.proposer.to_le_bytes());
        hasher.update(self.proposed_at.to_le_bytes());
        hasher.update(self.deadline.to_le_bytes());
        // include payload type
        let payload_type: u8 = match &self.payload {
            ProposalPayload::Signaling { .. } => 0,
            ProposalPayload::Action { .. } => 1,
            ProposalPayload::ParameterChange { .. } => 2,
            ProposalPayload::MembershipChange { .. } => 3,
            ProposalPayload::Emergency { .. } => 4,
            ProposalPayload::Text { .. } => 5,
        };
        hasher.update([payload_type]);
        hasher.finalize().into()
    }
}

/// proposal state machine
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ProposalState {
    /// accepting votes
    Voting,
    /// passed, awaiting execution (if applicable)
    Passed,
    /// failed to reach threshold
    Failed,
    /// passed and executed
    Executed,
    /// withdrawn by proposer
    Withdrawn,
    /// expired (deadline passed without decision)
    Expired,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_signaling_no_effect() {
        let proposal = SyndicateProposal::signaling(
            1,
            "Strategic direction".into(),
            "Should we focus on DeFi or NFTs?".into(),
            Some("https://forum.example.com/discussion/42".into()),
            1,
            100,
            200,
        );

        assert_eq!(proposal.kind(), ProposalKind::Signaling);
        assert!(!proposal.has_on_chain_effect());
        assert_eq!(proposal.required_approval(), ActionType::Routine);
    }

    #[test]
    fn test_text_proposal() {
        let proposal = SyndicateProposal::text(
            1,
            "Formal resolution".into(),
            "RESOLVED: The syndicate will prioritize privacy-preserving technologies.".into(),
            1,
            100,
            200,
        );

        assert_eq!(proposal.kind(), ProposalKind::Signaling);
        assert!(!proposal.has_on_chain_effect());
    }

    #[test]
    fn test_action_proposal() {
        let proposal = SyndicateProposal::new(
            1,
            "Spend funds".into(),
            "Transfer 100 UM to contractor".into(),
            ProposalPayload::Action {
                action_plan: vec![1, 2, 3],
            },
            1,
            100,
            200,
        );

        assert_eq!(proposal.kind(), ProposalKind::Action);
        assert!(proposal.has_on_chain_effect());
    }

    #[test]
    fn test_emergency_with_action() {
        let proposal = SyndicateProposal::new(
            1,
            "Emergency withdrawal".into(),
            "Market crash - withdraw from risky position".into(),
            ProposalPayload::Emergency {
                action_plan: Some(vec![1, 2, 3]),
                reason: "Market crash".into(),
            },
            1,
            100,
            110, // short deadline
        );

        assert_eq!(proposal.kind(), ProposalKind::Emergency);
        assert!(proposal.has_on_chain_effect());
    }

    #[test]
    fn test_proposal_hash_deterministic() {
        let proposal = SyndicateProposal::signaling(
            1,
            "Test".into(),
            "Description".into(),
            None,
            1,
            100,
            200,
        );

        let h1 = proposal.hash();
        let h2 = proposal.hash();
        assert_eq!(h1, h2);
    }
}
