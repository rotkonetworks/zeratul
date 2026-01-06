//! on-chain state for ligerito shielded pool

use std::collections::HashSet;
use nomt::KeyPath;

use ligerito_shielded_pool::{
    note::NoteCommitment,
    nullifier::Nullifier,
    commitment::StateRoot,
    channel::{ChannelId, ChannelState, SignedState},
    value::Amount,
};

/// the on-chain pool state
#[derive(Clone, Debug, Default)]
pub struct PoolState {
    /// merkle root of note commitment tree
    pub commitment_root: StateRoot,
    /// number of notes in tree
    pub note_count: u64,
    /// set of spent nullifiers
    pub nullifiers: HashSet<Nullifier>,
    /// active channels awaiting settlement
    pub active_channels: HashSet<ChannelId>,
}

impl PoolState {
    /// create empty pool state
    pub fn new() -> Self {
        Self::default()
    }

    /// check if a nullifier has been spent
    pub fn is_spent(&self, nullifier: &Nullifier) -> bool {
        self.nullifiers.contains(nullifier)
    }

    /// add a nullifier (mark note as spent)
    /// returns false if already spent (double-spend attempt)
    pub fn add_nullifier(&mut self, nullifier: Nullifier) -> bool {
        self.nullifiers.insert(nullifier)
    }

    /// update commitment root
    pub fn update_root(&mut self, new_root: StateRoot, new_count: u64) {
        self.commitment_root = new_root;
        self.note_count = new_count;
    }

    /// register an active channel
    pub fn open_channel(&mut self, channel_id: ChannelId) {
        self.active_channels.insert(channel_id);
    }

    /// close a channel (after settlement)
    pub fn close_channel(&mut self, channel_id: &ChannelId) -> bool {
        self.active_channels.remove(channel_id)
    }

    /// check if channel is active
    pub fn is_channel_active(&self, channel_id: &ChannelId) -> bool {
        self.active_channels.contains(channel_id)
    }
}

/// NOMT key paths for pool state
pub mod keys {
    use super::*;

    /// key for the pool state root
    pub fn pool_root() -> KeyPath {
        KeyPath::from_bytes(b"ligerito_pool/root")
    }

    /// key for the note count
    pub fn note_count() -> KeyPath {
        KeyPath::from_bytes(b"ligerito_pool/note_count")
    }

    /// key for a nullifier
    pub fn nullifier(nf: &Nullifier) -> KeyPath {
        let mut path = b"ligerito_pool/nullifiers/".to_vec();
        path.extend_from_slice(&nf.0);
        KeyPath::from_bytes(&path)
    }

    /// key for a channel
    pub fn channel(id: &ChannelId) -> KeyPath {
        let mut path = b"ligerito_pool/channels/".to_vec();
        path.extend_from_slice(&id.0);
        KeyPath::from_bytes(&path)
    }

    /// key for channel state
    pub fn channel_state(id: &ChannelId) -> KeyPath {
        let mut path = b"ligerito_pool/channel_state/".to_vec();
        path.extend_from_slice(&id.0);
        KeyPath::from_bytes(&path)
    }
}

/// extension trait for reading pool state
pub trait StateReadExt {
    /// get the current commitment root
    fn get_commitment_root(&self) -> Option<StateRoot>;

    /// get note count
    fn get_note_count(&self) -> u64;

    /// check if nullifier is spent
    fn is_nullifier_spent(&self, nullifier: &Nullifier) -> bool;

    /// get channel state
    fn get_channel_state(&self, id: &ChannelId) -> Option<SignedState>;
}

/// extension trait for writing pool state
pub trait StateWriteExt: StateReadExt {
    /// set the commitment root
    fn set_commitment_root(&mut self, root: StateRoot);

    /// increment note count
    fn increment_note_count(&mut self) -> u64;

    /// add a nullifier
    fn add_nullifier(&mut self, nullifier: Nullifier) -> bool;

    /// store channel state
    fn set_channel_state(&mut self, id: &ChannelId, state: SignedState);

    /// remove channel state (after settlement)
    fn remove_channel_state(&mut self, id: &ChannelId);
}

/// on-chain channel registration
#[derive(Clone, Debug)]
pub struct OnChainChannel {
    /// channel id
    pub id: ChannelId,
    /// initial deposit amount per participant
    pub deposits: Vec<(ligerito_shielded_pool::keys::PublicKey, Amount)>,
    /// block when channel was opened
    pub opened_at: u64,
    /// latest known nonce (for dispute resolution)
    pub latest_nonce: u64,
}

impl OnChainChannel {
    /// total value locked in channel
    pub fn total_locked(&self) -> Amount {
        self.deposits.iter()
            .fold(Amount::ZERO, |acc, (_, amt)| acc.saturating_add(*amt))
    }
}
