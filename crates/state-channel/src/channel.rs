//! state channel management
//!
//! open, close, and manage payment channels

use alloc::vec::Vec;
use scale_codec::{Decode, Encode};
use scale_info::TypeInfo;

use crate::state::PokerState;
use crate::types::*;

/// channel status
#[derive(Clone, Copy, Debug, Encode, Decode, TypeInfo, PartialEq, Eq)]
pub enum ChannelStatus {
    /// channel created, waiting for all participants to join
    Pending,
    /// all participants joined, channel is active
    Open,
    /// dispute submitted, waiting for resolution
    Disputed,
    /// channel closed, funds distributed
    Closed,
}

/// on-chain channel data (stored in parachain storage)
#[derive(Clone, Debug, Encode, Decode, TypeInfo)]
pub struct Channel {
    /// unique channel identifier
    pub id: ChannelId,
    /// current status
    pub status: ChannelStatus,
    /// all participants with their stakes
    pub participants: Vec<Participant>,
    /// total value locked in channel
    pub total_stake: Balance,
    /// latest agreed state hash
    pub state_hash: H256,
    /// latest agreed nonce
    pub nonce: Nonce,
    /// block number when channel was opened
    pub opened_at: u64,
    /// block number when dispute started (if any)
    pub dispute_started_at: Option<u64>,
    /// timeout for disputes (in blocks)
    pub dispute_timeout: u64,
}

impl Channel {
    /// create new channel
    pub fn new(
        participants: Vec<Participant>,
        dispute_timeout: u64,
        opened_at: u64,
    ) -> Self {
        let total_stake = participants.iter().map(|p| p.stake).sum();

        // channel id is hash of participants + opened_at
        let id_preimage = (&participants, opened_at).encode();
        let id = H256::from(blake3::hash(&id_preimage).as_bytes());

        Self {
            id,
            status: ChannelStatus::Pending,
            participants,
            total_stake,
            state_hash: H256::zero(),
            nonce: 0,
            opened_at,
            dispute_started_at: None,
            dispute_timeout,
        }
    }

    /// check if all participants have joined
    pub fn is_ready(&self) -> bool {
        // for now, assume ready when created with all participants
        self.participants.len() >= 2
    }

    /// open the channel for gameplay
    pub fn open(&mut self) {
        if self.is_ready() {
            self.status = ChannelStatus::Open;
        }
    }

    /// update state hash after successful state update
    pub fn update_state(&mut self, state_hash: H256, nonce: Nonce) {
        self.state_hash = state_hash;
        self.nonce = nonce;
    }

    /// start a dispute
    pub fn start_dispute(&mut self, current_block: u64) {
        self.status = ChannelStatus::Disputed;
        self.dispute_started_at = Some(current_block);
    }

    /// check if dispute has timed out
    pub fn is_dispute_timeout(&self, current_block: u64) -> bool {
        if let Some(started) = self.dispute_started_at {
            current_block >= started + self.dispute_timeout
        } else {
            false
        }
    }

    /// close the channel
    pub fn close(&mut self) {
        self.status = ChannelStatus::Closed;
    }

    /// get participant by account
    pub fn get_participant(&self, account: &AccountId) -> Option<&Participant> {
        self.participants.iter().find(|p| &p.account == account)
    }

    /// get participant by seat
    pub fn get_participant_by_seat(&self, seat: Seat) -> Option<&Participant> {
        self.participants.iter().find(|p| p.seat == seat)
    }
}

/// request to open a new channel
#[derive(Clone, Debug, Encode, Decode, TypeInfo)]
pub struct OpenChannelRequest {
    /// participants and their stakes
    pub participants: Vec<Participant>,
    /// big blind for the game
    pub big_blind: Balance,
    /// dispute timeout in blocks
    pub dispute_timeout: u64,
}

/// request to close a channel cooperatively
#[derive(Clone, Debug, Encode, Decode, TypeInfo)]
pub struct CloseChannelRequest {
    pub channel_id: ChannelId,
    /// final state signed by all participants
    pub final_state: SignedState<PokerState>,
    /// payout distribution
    pub payouts: Vec<(AccountId, Balance)>,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mock_participants() -> Vec<Participant> {
        vec![
            Participant {
                account: PublicKey::from_raw([1u8; 32]),
                seat: 0,
                stake: 1000,
                encryption_key: vec![],
            },
            Participant {
                account: PublicKey::from_raw([2u8; 32]),
                seat: 1,
                stake: 1000,
                encryption_key: vec![],
            },
        ]
    }

    #[test]
    fn test_create_channel() {
        let channel = Channel::new(mock_participants(), 100, 0);

        assert_eq!(channel.status, ChannelStatus::Pending);
        assert_eq!(channel.total_stake, 2000);
        assert!(channel.is_ready());
    }

    #[test]
    fn test_open_channel() {
        let mut channel = Channel::new(mock_participants(), 100, 0);
        channel.open();

        assert_eq!(channel.status, ChannelStatus::Open);
    }

    #[test]
    fn test_dispute_timeout() {
        let mut channel = Channel::new(mock_participants(), 100, 0);
        channel.open();
        channel.start_dispute(50);

        assert!(!channel.is_dispute_timeout(100));
        assert!(channel.is_dispute_timeout(151));
    }
}
