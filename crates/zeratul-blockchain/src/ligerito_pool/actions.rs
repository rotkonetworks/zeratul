//! pool actions - the transactions that modify shielded pool state

use ligerito_shielded_pool::{
    note::NoteCommitment,
    nullifier::Nullifier,
    commitment::StateRoot,
    channel::{ChannelId, ChannelState, SignedState, Participant},
    keys::{PublicKey, Address},
    value::{Amount, AssetId, Value},
    proof::{SpendProof, OutputProof, StateTransitionProof},
};

/// result of processing an action
#[derive(Clone, Debug)]
pub enum ActionResult {
    /// action succeeded
    Success {
        /// new nullifiers added
        nullifiers: Vec<Nullifier>,
        /// new note commitments added
        commitments: Vec<NoteCommitment>,
        /// events to emit
        events: Vec<PoolEvent>,
    },
    /// action failed
    Failed {
        reason: ActionError,
    },
}

/// errors during action processing
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ActionError {
    /// double-spend attempt
    NullifierAlreadySpent,
    /// invalid proof
    InvalidProof,
    /// merkle anchor doesn't match current state
    InvalidAnchor,
    /// channel not found
    ChannelNotFound,
    /// insufficient balance in channel
    InsufficientBalance,
    /// unauthorized (wrong signer)
    Unauthorized,
    /// channel already exists
    ChannelAlreadyExists,
    /// invalid channel state
    InvalidChannelState,
}

/// events emitted by the pool
#[derive(Clone, Debug)]
pub enum PoolEvent {
    /// a note was created
    NoteCreated {
        commitment: NoteCommitment,
    },
    /// a note was spent
    NoteSpent {
        nullifier: Nullifier,
    },
    /// deposit from transparent balance
    Deposit {
        amount: Amount,
        asset_id: AssetId,
        commitment: NoteCommitment,
    },
    /// withdrawal to transparent balance
    Withdraw {
        amount: Amount,
        asset_id: AssetId,
        nullifier: Nullifier,
    },
    /// channel opened
    ChannelOpened {
        channel_id: ChannelId,
        participants: Vec<PublicKey>,
        total_locked: Amount,
    },
    /// channel settled
    ChannelSettled {
        channel_id: ChannelId,
        final_balances: Vec<(PublicKey, Amount)>,
    },
    /// channel state updated (for watchtowers)
    ChannelStateUpdated {
        channel_id: ChannelId,
        nonce: u64,
    },
}

/// actions that can be submitted to the pool
#[derive(Clone, Debug)]
pub enum PoolAction {
    /// deposit tokens into shielded pool
    Deposit {
        /// amount to deposit
        value: Value,
        /// output proof for the new note
        output_proof: OutputProof,
        /// encrypted note for recipient
        encrypted_note: Vec<u8>,
    },

    /// withdraw tokens from shielded pool
    Withdraw {
        /// spend proof for the note being withdrawn
        spend_proof: SpendProof,
        /// destination address (transparent)
        destination: [u8; 32],
    },

    /// private transfer (spend + output)
    Transfer {
        /// spend proofs for input notes
        spend_proofs: Vec<SpendProof>,
        /// output proofs for new notes
        output_proofs: Vec<OutputProof>,
        /// encrypted notes for recipients
        encrypted_notes: Vec<Vec<u8>>,
    },

    /// open a new state channel
    OpenChannel {
        /// participants and their initial deposits
        participants: Vec<(PublicKey, Amount)>,
        /// spend proofs for the deposit notes
        deposit_proofs: Vec<SpendProof>,
        /// channel id (derived from participants)
        channel_id: ChannelId,
    },

    /// update channel state (for watchtowers/disputes)
    UpdateChannelState {
        /// channel id
        channel_id: ChannelId,
        /// signed state with higher nonce
        signed_state: SignedState,
        /// state transition proof
        transition_proof: StateTransitionProof,
    },

    /// settle a channel cooperatively
    SettleChannel {
        /// channel id
        channel_id: ChannelId,
        /// final signed state
        final_state: SignedState,
        /// output proofs for settlement notes
        settlement_proofs: Vec<OutputProof>,
        /// encrypted settlement notes
        encrypted_notes: Vec<Vec<u8>>,
    },

    /// force-close a channel (dispute)
    ForceCloseChannel {
        /// channel id
        channel_id: ChannelId,
        /// latest known signed state
        latest_state: SignedState,
        /// state transition proof chain (if disputing)
        proof_chain: Option<Vec<StateTransitionProof>>,
    },
}

impl PoolAction {
    /// estimate gas cost for this action
    pub fn estimated_gas(&self) -> u64 {
        match self {
            PoolAction::Deposit { .. } => 50_000,
            PoolAction::Withdraw { .. } => 100_000,
            PoolAction::Transfer { spend_proofs, output_proofs, .. } => {
                // ~50k per proof
                (spend_proofs.len() + output_proofs.len()) as u64 * 50_000
            }
            PoolAction::OpenChannel { deposit_proofs, .. } => {
                100_000 + deposit_proofs.len() as u64 * 50_000
            }
            PoolAction::UpdateChannelState { .. } => 75_000,
            PoolAction::SettleChannel { settlement_proofs, .. } => {
                100_000 + settlement_proofs.len() as u64 * 50_000
            }
            PoolAction::ForceCloseChannel { proof_chain, .. } => {
                let chain_len = proof_chain.as_ref().map(|c| c.len()).unwrap_or(0);
                150_000 + chain_len as u64 * 50_000
            }
        }
    }

    /// get the action type as a string
    pub fn action_type(&self) -> &'static str {
        match self {
            PoolAction::Deposit { .. } => "deposit",
            PoolAction::Withdraw { .. } => "withdraw",
            PoolAction::Transfer { .. } => "transfer",
            PoolAction::OpenChannel { .. } => "open_channel",
            PoolAction::UpdateChannelState { .. } => "update_channel",
            PoolAction::SettleChannel { .. } => "settle_channel",
            PoolAction::ForceCloseChannel { .. } => "force_close",
        }
    }
}

/// helper to encode action for signing
impl PoolAction {
    pub fn to_bytes(&self) -> Vec<u8> {
        // simplified encoding - would use scale codec in production
        let mut bytes = Vec::new();
        bytes.push(match self {
            PoolAction::Deposit { .. } => 0x01,
            PoolAction::Withdraw { .. } => 0x02,
            PoolAction::Transfer { .. } => 0x03,
            PoolAction::OpenChannel { .. } => 0x04,
            PoolAction::UpdateChannelState { .. } => 0x05,
            PoolAction::SettleChannel { .. } => 0x06,
            PoolAction::ForceCloseChannel { .. } => 0x07,
        });
        // action-specific encoding would go here
        bytes
    }
}
