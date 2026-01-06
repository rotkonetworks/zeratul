//! channel manager for poker and other p2p applications

use std::collections::HashMap;

use ligerito_shielded_pool::{
    channel::{Channel, ChannelId, ChannelState, SignedState, Action, Participant},
    keys::{SpendKey, PublicKey},
    value::Amount,
    proof::StateTransitionProof,
};

/// manages local channel state for a participant
pub struct ChannelManager {
    /// our spend key
    our_key: SpendKey,
    /// active channels
    channels: HashMap<ChannelId, Channel>,
}

impl ChannelManager {
    /// create a new channel manager
    pub fn new(our_key: SpendKey) -> Self {
        Self {
            our_key,
            channels: HashMap::new(),
        }
    }

    /// our public key
    pub fn our_pk(&self) -> PublicKey {
        self.our_key.public_key()
    }

    /// create a new channel with another participant
    pub fn create_channel(
        &mut self,
        counterparty: PublicKey,
        our_deposit: Amount,
        their_deposit: Amount,
    ) -> ChannelId {
        let participants = vec![
            Participant { public_key: self.our_pk(), balance: our_deposit },
            Participant { public_key: counterparty, balance: their_deposit },
        ];

        let channel = Channel::new(self.our_key.clone(), participants);
        let channel_id = channel.current_state.state.channel_id;

        self.channels.insert(channel_id, channel);
        channel_id
    }

    /// get a channel by id
    pub fn get_channel(&self, id: &ChannelId) -> Option<&Channel> {
        self.channels.get(id)
    }

    /// get mutable channel
    pub fn get_channel_mut(&mut self, id: &ChannelId) -> Option<&mut Channel> {
        self.channels.get_mut(id)
    }

    /// apply an action to a channel (as initiator)
    pub fn apply_action(
        &mut self,
        channel_id: &ChannelId,
        action: &Action,
    ) -> Result<SignedState, ChannelError> {
        let channel = self.channels.get_mut(channel_id)
            .ok_or(ChannelError::ChannelNotFound)?;

        channel.apply_action(action)
            .map_err(|e| ChannelError::ActionFailed(format!("{:?}", e)))?;

        Ok(channel.current_state.clone())
    }

    /// receive and countersign a state from counterparty
    pub fn receive_state(
        &mut self,
        channel_id: &ChannelId,
        signed_state: SignedState,
    ) -> Result<SignedState, ChannelError> {
        let channel = self.channels.get_mut(channel_id)
            .ok_or(ChannelError::ChannelNotFound)?;

        channel.receive_state(signed_state)
            .map_err(|e| ChannelError::InvalidState(format!("{:?}", e)))?;

        Ok(channel.current_state.clone())
    }

    /// get current state for a channel
    pub fn current_state(&self, channel_id: &ChannelId) -> Option<&SignedState> {
        self.channels.get(channel_id).map(|c| &c.current_state)
    }

    /// close a channel (after settlement)
    pub fn close_channel(&mut self, channel_id: &ChannelId) -> Option<SignedState> {
        self.channels.remove(channel_id).map(|c| c.current_state)
    }

    /// list all active channel ids
    pub fn active_channels(&self) -> Vec<ChannelId> {
        self.channels.keys().copied().collect()
    }

    /// get our balance in a channel
    pub fn our_balance(&self, channel_id: &ChannelId) -> Option<Amount> {
        self.channels.get(channel_id)
            .and_then(|c| c.current_state.state.balance_of(&self.our_pk()))
    }
}

/// channel manager errors
#[derive(Clone, Debug)]
pub enum ChannelError {
    ChannelNotFound,
    ActionFailed(String),
    InvalidState(String),
    InsufficientBalance,
}

/// poker-specific channel extensions
pub mod poker {
    use super::*;

    /// poker game state stored in channel app_data
    #[derive(Clone, Debug)]
    pub struct PokerGameState {
        /// current pot
        pub pot: Amount,
        /// current round (preflop, flop, turn, river)
        pub round: u8,
        /// whose turn
        pub current_player: u8,
        /// community cards (encrypted)
        pub community_cards: Vec<u8>,
        /// player hole cards (encrypted per player)
        pub hole_cards: Vec<Vec<u8>>,
        /// current bets per player
        pub bets: Vec<Amount>,
        /// folded players
        pub folded: Vec<bool>,
    }

    impl PokerGameState {
        /// encode for channel app_data
        pub fn encode(&self) -> Vec<u8> {
            // simplified encoding
            let mut bytes = Vec::new();
            bytes.extend_from_slice(&self.pot.0.to_le_bytes());
            bytes.push(self.round);
            bytes.push(self.current_player);
            // ... full encoding would go here
            bytes
        }

        /// decode from channel app_data
        pub fn decode(data: &[u8]) -> Option<Self> {
            if data.len() < 18 {
                return None;
            }
            let mut pot_bytes = [0u8; 16];
            pot_bytes.copy_from_slice(&data[0..16]);
            Some(Self {
                pot: Amount(u128::from_le_bytes(pot_bytes)),
                round: data[16],
                current_player: data[17],
                community_cards: Vec::new(),
                hole_cards: Vec::new(),
                bets: Vec::new(),
                folded: Vec::new(),
            })
        }
    }

    /// poker-specific actions
    #[derive(Clone, Debug)]
    pub enum PokerAction {
        /// post blind
        PostBlind { amount: Amount },
        /// call current bet
        Call,
        /// raise by amount
        Raise { amount: Amount },
        /// fold
        Fold,
        /// check
        Check,
        /// reveal hole cards (showdown)
        Reveal { cards: Vec<u8> },
    }

    impl PokerAction {
        /// encode as channel Action::AppAction
        pub fn to_app_action(&self, actor: PublicKey) -> Action {
            Action::AppAction {
                actor,
                data: self.encode(),
            }
        }

        fn encode(&self) -> Vec<u8> {
            let mut bytes = Vec::new();
            match self {
                PokerAction::PostBlind { amount } => {
                    bytes.push(0x01);
                    bytes.extend_from_slice(&amount.0.to_le_bytes());
                }
                PokerAction::Call => bytes.push(0x02),
                PokerAction::Raise { amount } => {
                    bytes.push(0x03);
                    bytes.extend_from_slice(&amount.0.to_le_bytes());
                }
                PokerAction::Fold => bytes.push(0x04),
                PokerAction::Check => bytes.push(0x05),
                PokerAction::Reveal { cards } => {
                    bytes.push(0x06);
                    bytes.extend_from_slice(cards);
                }
            }
            bytes
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_channel_manager() {
        let sk_alice = SpendKey::from_phrase("alice", "");
        let sk_bob = SpendKey::from_phrase("bob", "");

        let mut alice_mgr = ChannelManager::new(sk_alice);
        let bob_pk = sk_bob.public_key();

        // create channel
        let channel_id = alice_mgr.create_channel(
            bob_pk,
            1000u64.into(),
            500u64.into(),
        );

        // check balances
        assert_eq!(alice_mgr.our_balance(&channel_id), Some(1000u64.into()));

        // apply transfer
        let action = Action::Transfer {
            from: alice_mgr.our_pk(),
            to: bob_pk,
            amount: 100u64.into(),
        };
        alice_mgr.apply_action(&channel_id, &action).unwrap();

        // balance updated
        assert_eq!(alice_mgr.our_balance(&channel_id), Some(900u64.into()));
    }
}
