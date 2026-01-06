//! core types for state channels

use alloc::vec::Vec;
use scale_codec::{Decode, Encode};
use scale_info::TypeInfo;

/// 32-byte hash type
#[derive(Clone, Copy, Debug, Encode, Decode, TypeInfo, PartialEq, Eq, Default, Hash)]
pub struct H256(pub [u8; 32]);

impl H256 {
    pub fn zero() -> Self {
        Self([0u8; 32])
    }

    pub fn from_slice(data: &[u8]) -> Self {
        let mut bytes = [0u8; 32];
        bytes.copy_from_slice(&data[..32]);
        Self(bytes)
    }
}

impl From<&[u8; 32]> for H256 {
    fn from(bytes: &[u8; 32]) -> Self {
        Self(*bytes)
    }
}

/// 32-byte public key
#[derive(Clone, Copy, Debug, Encode, Decode, TypeInfo, PartialEq, Eq, Default, Hash)]
pub struct PublicKey(pub [u8; 32]);

impl PublicKey {
    pub fn from_raw(bytes: [u8; 32]) -> Self {
        Self(bytes)
    }
}

/// 64-byte signature
#[derive(Clone, Copy, Debug, Encode, Decode, TypeInfo, PartialEq, Eq)]
pub struct Signature(pub [u8; 64]);

impl Default for Signature {
    fn default() -> Self {
        Self([0u8; 64])
    }
}

impl Signature {
    pub fn from_raw(bytes: [u8; 64]) -> Self {
        Self(bytes)
    }
}

/// channel identifier (blake3 hash of initial params)
pub type ChannelId = H256;

/// player account (public key)
pub type AccountId = PublicKey;

/// balance in smallest unit
pub type Balance = u128;

/// state nonce (monotonically increasing)
pub type Nonce = u64;

/// card index (0-51)
pub type CardIndex = u8;

/// seat position at table (0-9, max 10 players)
pub type Seat = u8;

/// maximum players per table
pub const MAX_PLAYERS: usize = 10;

/// ligerito proof bytes (serialized)
#[derive(Clone, Debug, Encode, Decode, TypeInfo, PartialEq, Eq)]
pub struct LigeritoProof(pub Vec<u8>);

/// shuffle commitment (merkle root of encrypted deck)
#[derive(Clone, Copy, Debug, Encode, Decode, TypeInfo, PartialEq, Eq, Default)]
pub struct ShuffleCommitment(pub H256);

/// reveal token for decrypting a card
#[derive(Clone, Debug, Encode, Decode, TypeInfo, PartialEq, Eq)]
pub struct RevealToken {
    pub card_index: CardIndex,
    pub token: Vec<u8>,
    /// chaum-pedersen proof of correct decryption
    pub proof: Vec<u8>,
}

/// channel participant with stake and keys
#[derive(Clone, Debug, Encode, Decode, TypeInfo, PartialEq, Eq)]
pub struct Participant {
    pub account: AccountId,
    pub seat: Seat,
    pub stake: Balance,
    /// public key for card encryption
    pub encryption_key: Vec<u8>,
}

/// signed state update
#[derive(Clone, Debug, Encode, Decode, TypeInfo, PartialEq, Eq)]
pub struct SignedState<S: PartialEq + Eq> {
    pub state: S,
    pub nonce: Nonce,
    /// signatures from all participants (indexed by seat)
    pub signatures: Vec<Option<Signature>>,
}

impl<S: Encode + PartialEq + Eq> SignedState<S> {
    /// compute hash of state for signing
    pub fn state_hash(&self) -> H256 {
        let encoded = (&self.state, self.nonce).encode();
        H256::from(blake3::hash(&encoded).as_bytes())
    }

    /// check if state is fully signed by all participants
    pub fn is_fully_signed(&self, participant_count: usize) -> bool {
        self.signatures.len() >= participant_count
            && self.signatures.iter().take(participant_count).all(|s| s.is_some())
    }

    /// count valid signatures
    pub fn signature_count(&self) -> usize {
        self.signatures.iter().filter(|s| s.is_some()).count()
    }
}

/// result of state transition
#[derive(Clone, Debug, Encode, Decode, TypeInfo, PartialEq, Eq)]
pub enum TransitionResult {
    /// transition accepted, continue
    Continue,
    /// game ended, payouts determined
    Finished { payouts: Vec<(Seat, Balance)> },
    /// invalid transition
    Invalid(TransitionError),
}

/// transition errors
#[derive(Clone, Debug, Encode, Decode, TypeInfo, PartialEq, Eq)]
pub enum TransitionError {
    /// not this player's turn
    NotYourTurn,
    /// invalid action for current phase
    InvalidPhase,
    /// insufficient chips for bet
    InsufficientChips,
    /// invalid bet amount
    InvalidBetAmount,
    /// invalid shuffle proof
    InvalidShuffleProof,
    /// invalid reveal token
    InvalidRevealToken,
    /// card already revealed
    CardAlreadyRevealed,
    /// missing required signature
    MissingSignature,
    /// nonce mismatch
    NonceMismatch,
    /// channel not open
    ChannelNotOpen,
}
