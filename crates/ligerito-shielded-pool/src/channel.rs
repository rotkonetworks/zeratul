//! p2p state channels for shielded rollups
//!
//! channels live entirely between participants
//! each state transition is proven with ligerito
//! final state settles to L1

#[cfg(not(feature = "std"))]
use alloc::vec::Vec;

use crate::keys::{PublicKey, Signature, SpendKey};
use crate::note::Note;
use crate::value::{Amount, Value};
use crate::STATE_DOMAIN;

/// channel identifier
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct ChannelId(pub [u8; 32]);

impl ChannelId {
    /// derive channel id from participants
    pub fn derive(participants: &[PublicKey]) -> Self {
        let mut hasher = blake3::Hasher::new();
        hasher.update(b"ligerito.channel.id.v1");
        for pk in participants {
            hasher.update(&pk.0);
        }
        Self(*hasher.finalize().as_bytes())
    }
}

/// participant in a channel
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Participant {
    /// public key for signing
    pub public_key: PublicKey,
    /// current balance in channel
    pub balance: Amount,
}

/// channel state (signed by all participants)
#[derive(Clone, Debug)]
pub struct ChannelState {
    /// channel identifier
    pub channel_id: ChannelId,
    /// state sequence number (monotonic)
    pub nonce: u64,
    /// participant balances
    pub participants: Vec<Participant>,
    /// hash of previous state (for chain)
    pub prev_state_hash: [u8; 32],
    /// optional: application-specific data (e.g., poker game state)
    pub app_data: Vec<u8>,
}

impl ChannelState {
    /// create initial channel state
    pub fn new(participants: Vec<Participant>) -> Self {
        let pks: Vec<_> = participants.iter().map(|p| p.public_key).collect();
        Self {
            channel_id: ChannelId::derive(&pks),
            nonce: 0,
            participants,
            prev_state_hash: [0u8; 32],
            app_data: Vec::new(),
        }
    }

    /// hash of this state
    pub fn hash(&self) -> [u8; 32] {
        let mut hasher = blake3::Hasher::new();
        hasher.update(STATE_DOMAIN);
        hasher.update(&self.channel_id.0);
        hasher.update(&self.nonce.to_le_bytes());
        for p in &self.participants {
            hasher.update(&p.public_key.0);
            hasher.update(&p.balance.0.to_le_bytes());
        }
        hasher.update(&self.prev_state_hash);
        hasher.update(&self.app_data);
        *hasher.finalize().as_bytes()
    }

    /// total balance in channel (should be constant)
    pub fn total_balance(&self) -> Amount {
        self.participants.iter()
            .fold(Amount::ZERO, |acc, p| acc.saturating_add(p.balance))
    }

    /// get balance for a participant
    pub fn balance_of(&self, pk: &PublicKey) -> Option<Amount> {
        self.participants.iter()
            .find(|p| &p.public_key == pk)
            .map(|p| p.balance)
    }
}

/// signed channel state
#[derive(Clone, Debug)]
pub struct SignedState {
    pub state: ChannelState,
    /// signatures from all participants
    pub signatures: Vec<(PublicKey, Signature)>,
}

impl SignedState {
    /// check if all participants have signed
    pub fn is_fully_signed(&self) -> bool {
        let state_hash = self.state.hash();
        self.state.participants.iter().all(|p| {
            self.signatures.iter().any(|(pk, sig)| {
                pk == &p.public_key && pk.verify(&state_hash, sig)
            })
        })
    }

    /// add a signature
    pub fn add_signature(&mut self, pk: PublicKey, sig: Signature) {
        // remove old sig from this participant
        self.signatures.retain(|(k, _)| k != &pk);
        self.signatures.push((pk, sig));
    }
}

/// action in a channel (causes state transition)
#[derive(Clone, Debug)]
pub enum Action {
    /// transfer between participants
    Transfer {
        from: PublicKey,
        to: PublicKey,
        amount: Amount,
    },
    /// application-specific action
    AppAction {
        actor: PublicKey,
        data: Vec<u8>,
    },
    /// cooperative close
    Close,
}

impl Action {
    /// encode action for proof
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut bytes = Vec::new();
        match self {
            Action::Transfer { from, to, amount } => {
                bytes.push(0x01);
                bytes.extend_from_slice(&from.0);
                bytes.extend_from_slice(&to.0);
                bytes.extend_from_slice(&amount.0.to_le_bytes());
            }
            Action::AppAction { actor, data } => {
                bytes.push(0x02);
                bytes.extend_from_slice(&actor.0);
                bytes.extend_from_slice(data);
            }
            Action::Close => {
                bytes.push(0x03);
            }
        }
        bytes
    }
}

/// the p2p channel
#[derive(Clone, Debug)]
pub struct Channel {
    /// current signed state
    pub current_state: SignedState,
    /// our spend key
    our_key: SpendKey,
    /// our public key
    pub our_pk: PublicKey,
}

impl Channel {
    /// create new channel with initial deposits
    pub fn new(our_key: SpendKey, participants: Vec<Participant>) -> Self {
        let our_pk = our_key.public_key();
        let state = ChannelState::new(participants);
        Self {
            current_state: SignedState {
                state,
                signatures: Vec::new(),
            },
            our_key,
            our_pk,
        }
    }

    /// sign current state
    pub fn sign(&mut self) {
        let hash = self.current_state.state.hash();
        let sig = self.our_key.sign(&hash);
        self.current_state.add_signature(self.our_pk, sig);
    }

    /// apply an action and create new state
    pub fn apply_action(&mut self, action: &Action) -> Result<(), ChannelError> {
        let new_state = self.compute_next_state(action)?;

        self.current_state = SignedState {
            state: new_state,
            signatures: Vec::new(),
        };

        // sign the new state
        self.sign();

        Ok(())
    }

    /// compute next state from action (for verification)
    fn compute_next_state(&self, action: &Action) -> Result<ChannelState, ChannelError> {
        let current = &self.current_state.state;
        let mut new_participants = current.participants.clone();

        match action {
            Action::Transfer { from, to, amount } => {
                // find sender and receiver
                let sender_idx = new_participants.iter()
                    .position(|p| &p.public_key == from)
                    .ok_or(ChannelError::ParticipantNotFound)?;

                let receiver_idx = new_participants.iter()
                    .position(|p| &p.public_key == to)
                    .ok_or(ChannelError::ParticipantNotFound)?;

                // check sufficient balance
                if new_participants[sender_idx].balance.0 < amount.0 {
                    return Err(ChannelError::InsufficientBalance);
                }

                // update balances
                new_participants[sender_idx].balance = new_participants[sender_idx].balance
                    .checked_sub(*amount)
                    .ok_or(ChannelError::InsufficientBalance)?;
                new_participants[receiver_idx].balance = new_participants[receiver_idx].balance
                    .checked_add(*amount)
                    .ok_or(ChannelError::Overflow)?;
            }
            Action::AppAction { .. } => {
                // app-specific logic handled by application layer
            }
            Action::Close => {
                // no state change, just marks channel as closing
            }
        }

        Ok(ChannelState {
            channel_id: current.channel_id,
            nonce: current.nonce + 1,
            participants: new_participants,
            prev_state_hash: current.hash(),
            app_data: current.app_data.clone(),
        })
    }

    /// verify and apply received state from counterparty
    pub fn receive_state(&mut self, signed: SignedState) -> Result<(), ChannelError> {
        // check nonce is higher
        if signed.state.nonce <= self.current_state.state.nonce {
            return Err(ChannelError::StaleState);
        }

        // check channel id matches
        if signed.state.channel_id != self.current_state.state.channel_id {
            return Err(ChannelError::ChannelMismatch);
        }

        // check total balance is preserved
        if signed.state.total_balance() != self.current_state.state.total_balance() {
            return Err(ChannelError::BalanceMismatch);
        }

        // verify at least one valid signature
        let hash = signed.state.hash();
        let has_valid_sig = signed.signatures.iter().any(|(pk, sig)| {
            self.current_state.state.participants.iter()
                .any(|p| &p.public_key == pk) && pk.verify(&hash, sig)
        });

        if !has_valid_sig {
            return Err(ChannelError::InvalidSignature);
        }

        // accept and countersign
        self.current_state = signed;
        self.sign();

        Ok(())
    }

    /// get final note commitments for settlement
    pub fn settlement_notes(&self, addresses: &[(PublicKey, crate::keys::Address)]) -> Vec<Note> {
        use crate::note::Rseed;
        use crate::value::AssetId;

        self.current_state.state.participants.iter()
            .filter_map(|p| {
                let addr = addresses.iter()
                    .find(|(pk, _)| pk == &p.public_key)
                    .map(|(_, a)| *a)?;

                if p.balance.is_zero() {
                    return None;
                }

                // in production, rseed would be derived deterministically
                let mut rseed_bytes = [0u8; 32];
                let mut hasher = blake3::Hasher::new();
                hasher.update(b"ligerito.settlement.rseed.v1");
                hasher.update(&self.current_state.state.hash());
                hasher.update(&p.public_key.0);
                rseed_bytes.copy_from_slice(hasher.finalize().as_bytes());

                Some(Note::new(
                    Value::new(AssetId::NATIVE, p.balance),
                    addr,
                    Rseed(rseed_bytes),
                ))
            })
            .collect()
    }
}

/// channel errors
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ChannelError {
    ParticipantNotFound,
    InsufficientBalance,
    Overflow,
    StaleState,
    ChannelMismatch,
    BalanceMismatch,
    InvalidSignature,
    InvalidProof,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_channel_transfer() {
        let sk_alice = SpendKey::from_phrase("alice", "");
        let sk_bob = SpendKey::from_phrase("bob", "");

        let pk_alice = sk_alice.public_key();
        let pk_bob = sk_bob.public_key();

        let participants = vec![
            Participant { public_key: pk_alice, balance: 1000u64.into() },
            Participant { public_key: pk_bob, balance: 500u64.into() },
        ];

        let mut alice_channel = Channel::new(sk_alice, participants.clone());
        let mut bob_channel = Channel::new(sk_bob, participants);

        // alice transfers 100 to bob
        let action = Action::Transfer {
            from: pk_alice,
            to: pk_bob,
            amount: 100u64.into(),
        };

        alice_channel.apply_action(&action).unwrap();

        // bob receives and countersigns
        bob_channel.receive_state(alice_channel.current_state.clone()).unwrap();

        // check balances
        assert_eq!(bob_channel.current_state.state.balance_of(&pk_alice), Some(900u64.into()));
        assert_eq!(bob_channel.current_state.state.balance_of(&pk_bob), Some(600u64.into()));

        // both should have signed
        assert!(bob_channel.current_state.is_fully_signed());
    }

    #[test]
    fn test_insufficient_balance() {
        let sk = SpendKey::from_phrase("alice", "");
        let pk = sk.public_key();
        let pk_bob = SpendKey::from_phrase("bob", "").public_key();

        let participants = vec![
            Participant { public_key: pk, balance: 100u64.into() },
            Participant { public_key: pk_bob, balance: 100u64.into() },
        ];

        let mut channel = Channel::new(sk, participants);

        // try to transfer more than balance
        let action = Action::Transfer {
            from: pk,
            to: pk_bob,
            amount: 200u64.into(),
        };

        assert_eq!(channel.apply_action(&action), Err(ChannelError::InsufficientBalance));
    }

    #[test]
    fn test_balance_preservation() {
        let sk = SpendKey::from_phrase("alice", "");
        let pk = sk.public_key();
        let pk_bob = SpendKey::from_phrase("bob", "").public_key();

        let participants = vec![
            Participant { public_key: pk, balance: 500u64.into() },
            Participant { public_key: pk_bob, balance: 500u64.into() },
        ];

        let mut channel = Channel::new(sk, participants);
        let initial_total = channel.current_state.state.total_balance();

        // do some transfers
        channel.apply_action(&Action::Transfer {
            from: pk,
            to: pk_bob,
            amount: 100u64.into(),
        }).unwrap();

        channel.apply_action(&Action::Transfer {
            from: pk,
            to: pk_bob,
            amount: 50u64.into(),
        }).unwrap();

        // total should be preserved
        assert_eq!(channel.current_state.state.total_balance(), initial_total);
    }
}
