//! protocol - poker p2p message types
//!
//! defines all messages exchanged between peers during table discovery,
//! game setup, and gameplay.

use parity_scale_codec::{Decode, Encode};

/// participant role at the table
#[derive(Clone, Copy, Debug, PartialEq, Eq, Encode, Decode)]
pub enum Role {
    /// active player with channel and buy-in
    Player,
    /// observer with view-only access
    Spectator,
}

/// security tier (mirrors ghettobox-primitives)
#[derive(Clone, Copy, Debug, PartialEq, Eq, Encode, Decode)]
pub enum SecurityTier {
    Training,
    Casual,
    Standard,
    Secure,
    Paranoid,
}

impl Default for SecurityTier {
    fn default() -> Self {
        Self::Training
    }
}

impl SecurityTier {
    pub fn allows_real_stakes(&self) -> bool {
        !matches!(self, Self::Training)
    }

    pub fn min_buy_in(&self) -> u128 {
        match self {
            Self::Training => 0,
            Self::Casual => 1_000_000_000_000,    // 1 KSM
            Self::Standard => 1_000_000_000_000,  // 1 KSM
            Self::Secure => 10_000_000_000_000,   // 10 KSM
            Self::Paranoid => 100_000_000_000_000, // 100 KSM
        }
    }
}

/// table rules - defined by host, must be accepted by all participants
#[derive(Clone, Debug, PartialEq, Eq, Encode, Decode)]
pub struct TableRules {
    /// small blind amount (planck)
    pub small_blind: u128,
    /// big blind amount (planck)
    pub big_blind: u128,
    /// ante per hand (0 = no ante)
    pub ante: u128,
    /// minimum buy-in
    pub min_buy_in: u128,
    /// maximum buy-in (0 = no max)
    pub max_buy_in: u128,
    /// number of seats (2-9)
    pub seats: u8,
    /// security tier required
    pub tier: SecurityTier,
    /// allow spectators
    pub allow_spectators: bool,
    /// max spectators (0 = unlimited)
    pub max_spectators: u8,
    /// time bank per player (seconds)
    pub time_bank: u32,
    /// action timeout (seconds)
    pub action_timeout: u32,
}

impl Default for TableRules {
    fn default() -> Self {
        Self {
            small_blind: 5_000_000_000,   // 0.005 KSM
            big_blind: 10_000_000_000,    // 0.01 KSM
            ante: 0,
            min_buy_in: 1_000_000_000_000, // 1 KSM
            max_buy_in: 0,
            seats: 6,
            tier: SecurityTier::Training,
            allow_spectators: true,
            max_spectators: 10,
            time_bank: 60,
            action_timeout: 30,
        }
    }
}

impl TableRules {
    /// training mode - free play with no real money
    pub fn training() -> Self {
        Self {
            small_blind: 0,
            big_blind: 0,
            ante: 0,
            min_buy_in: 0,
            max_buy_in: 0,
            seats: 6,
            tier: SecurityTier::Training,
            allow_spectators: true,
            max_spectators: 20,
            time_bank: 120,
            action_timeout: 60,
        }
    }

    /// heads-up (2 players)
    pub fn heads_up(tier: SecurityTier) -> Self {
        Self {
            seats: 2,
            tier,
            ..Default::default()
        }
    }

    /// compute rules hash for verification
    pub fn hash(&self) -> [u8; 32] {
        let encoded = self.encode();
        *blake3::hash(&encoded).as_bytes()
    }

    /// validate rules are consistent
    pub fn validate(&self) -> Result<(), RulesError> {
        if self.seats < 2 || self.seats > 9 {
            return Err(RulesError::InvalidSeats);
        }
        if self.big_blind < self.small_blind {
            return Err(RulesError::InvalidBlinds);
        }
        if self.tier.allows_real_stakes() && self.min_buy_in < self.tier.min_buy_in() {
            return Err(RulesError::BuyInTooLow);
        }
        if self.action_timeout == 0 {
            return Err(RulesError::InvalidTimeout);
        }
        Ok(())
    }
}

#[derive(Debug, Clone, thiserror::Error)]
pub enum RulesError {
    #[error("seats must be 2-9")]
    InvalidSeats,
    #[error("big blind must be >= small blind")]
    InvalidBlinds,
    #[error("buy-in too low for tier")]
    BuyInTooLow,
    #[error("action timeout must be > 0")]
    InvalidTimeout,
}

/// p2p message types
#[derive(Clone, Debug, Encode, Decode)]
pub enum Message {
    // === Table Discovery ===
    /// host announces table rules after PAKE auth
    TableAnnounce(TableAnnounce),

    // === Join Flow ===
    /// request to join table
    JoinRequest(JoinRequest),
    /// host accepts join request
    JoinAccept(JoinAccept),
    /// host rejects join request
    JoinReject(JoinReject),
    /// participant confirms channel opened (player only)
    ChannelConfirm(ChannelConfirm),

    // === Game State ===
    /// game is starting
    GameStart(GameStart),
    /// player action (bet, fold, etc)
    Action(PlayerAction),
    /// card reveal (mental poker)
    CardReveal(CardReveal),
    /// hand result
    HandResult(HandResult),

    // === Channel ===
    /// signed state update
    StateUpdate(StateUpdate),
    /// request cooperative close
    CloseRequest(CloseRequest),

    // === Utility ===
    /// ping for keepalive
    Ping(u64),
    /// pong response
    Pong(u64),
    /// error message
    Error(ErrorMsg),

    // === Chat ===
    /// table chat message (broadcast to all at table)
    ChatMessage(ChatMessage),
    /// private message (direct to one player)
    PrivateMessage(PrivateMessage),
    /// typing indicator
    Typing(TypingIndicator),

    // === Voice ===
    /// voice audio packet (opus encoded)
    VoiceData(VoiceData),
    /// voice state change (mute, unmute, speaking)
    VoiceState(VoiceState),
}

/// table announcement from host
#[derive(Clone, Debug, Encode, Decode)]
pub struct TableAnnounce {
    pub rules: TableRules,
    pub host_pubkey: [u8; 32],
    pub signature: [u8; 64],
}

/// join request from participant
#[derive(Clone, Debug, Encode, Decode)]
pub struct JoinRequest {
    pub role: Role,
    pub pubkey: [u8; 32],
    /// for Player: proof of tier eligibility
    pub tier_proof: Option<Vec<u8>>,
    /// signature over rules hash
    pub rules_acceptance_sig: [u8; 64],
}

/// join acceptance from host
#[derive(Clone, Debug, Encode, Decode)]
pub struct JoinAccept {
    pub role: Role,
    /// seat number (1-indexed, 0 for spectator)
    pub seat: u8,
    /// for Player: channel parameters to open
    pub channel_params: Option<ChannelParams>,
    /// for Spectator: viewing key for delayed reveals
    pub view_key: Option<[u8; 32]>,
}

/// channel parameters for player
#[derive(Clone, Debug, Encode, Decode)]
pub struct ChannelParams {
    /// channel id (derived from participants)
    pub channel_id: [u8; 32],
    /// required deposit
    pub deposit: u128,
    /// other participants' pubkeys
    pub participants: Vec<[u8; 32]>,
}

/// join rejection from host
#[derive(Clone, Debug, Encode, Decode)]
pub struct JoinReject {
    pub reason: RejectReason,
}

#[derive(Clone, Debug, Encode, Decode)]
pub enum RejectReason {
    TableFull,
    SpectatorsFull,
    TierNotMet,
    Banned,
    GameInProgress,
    InvalidSignature,
}

/// channel confirmation from player
#[derive(Clone, Debug, Encode, Decode)]
pub struct ChannelConfirm {
    /// on-chain tx hash
    pub tx_hash: [u8; 32],
    /// channel id
    pub channel_id: [u8; 32],
    /// deposit amount
    pub deposit: u128,
}

/// game start notification
#[derive(Clone, Debug, Encode, Decode)]
pub struct GameStart {
    /// hand number
    pub hand_number: u64,
    /// button position (seat number)
    pub button: u8,
    /// player order for this hand
    pub player_order: Vec<u8>,
    /// deck commitment from shuffle protocol
    pub deck_commitment: [u8; 32],
}

/// player action
#[derive(Clone, Debug, Encode, Decode)]
pub struct PlayerAction {
    pub hand_number: u64,
    pub seat: u8,
    pub action: ActionType,
    pub signature: [u8; 64],
}

#[derive(Clone, Debug, Encode, Decode)]
pub enum ActionType {
    Fold,
    Check,
    Call,
    Bet(u128),
    Raise(u128),
    AllIn,
}

/// card reveal in mental poker
#[derive(Clone, Debug, Encode, Decode)]
pub struct CardReveal {
    pub hand_number: u64,
    /// card indices being revealed
    pub cards: Vec<u8>,
    /// decryption keys for each card
    pub keys: Vec<[u8; 32]>,
    /// proof of correct decryption
    pub proof: Vec<u8>,
}

/// hand result
#[derive(Clone, Debug, Encode, Decode)]
pub struct HandResult {
    pub hand_number: u64,
    /// pot awarded to each seat
    pub payouts: Vec<(u8, u128)>,
    /// winning hand description
    pub winning_hand: Option<Vec<u8>>,
}

/// signed state update for channel
#[derive(Clone, Debug, Encode, Decode)]
pub struct StateUpdate {
    pub channel_id: [u8; 32],
    pub nonce: u64,
    pub state_hash: [u8; 32],
    /// balances per seat
    pub balances: Vec<u128>,
    pub signatures: Vec<[u8; 64]>,
}

/// cooperative close request
#[derive(Clone, Debug, Encode, Decode)]
pub struct CloseRequest {
    pub channel_id: [u8; 32],
    pub final_state: StateUpdate,
}

/// error message
#[derive(Clone, Debug, Encode, Decode)]
pub struct ErrorMsg {
    pub code: u16,
    pub message: String,
}

// === Chat Types ===

/// table chat message (visible to all at table)
#[derive(Clone, Debug, Encode, Decode)]
pub struct ChatMessage {
    /// sender seat (0 = spectator)
    pub seat: u8,
    /// sender display name
    pub sender: String,
    /// message content (max 500 chars, sanitized)
    pub content: String,
    /// unix timestamp ms
    pub timestamp: u64,
    /// message id for dedup
    pub id: u64,
}

impl ChatMessage {
    pub fn new(seat: u8, sender: String, content: String) -> Self {
        use std::time::{SystemTime, UNIX_EPOCH};
        let timestamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as u64;
        Self {
            seat,
            sender,
            content: content.chars().take(500).collect(),
            timestamp,
            id: timestamp ^ (seat as u64 * 0x9e3779b97f4a7c15),
        }
    }
}

/// private message (direct between two players)
#[derive(Clone, Debug, Encode, Decode)]
pub struct PrivateMessage {
    /// sender pubkey
    pub from: [u8; 32],
    /// recipient pubkey
    pub to: [u8; 32],
    /// encrypted content (x25519 + chacha20poly1305)
    pub ciphertext: Vec<u8>,
    /// nonce for decryption
    pub nonce: [u8; 24],
    /// unix timestamp ms
    pub timestamp: u64,
}

/// typing indicator
#[derive(Clone, Debug, Encode, Decode)]
pub struct TypingIndicator {
    pub seat: u8,
    pub is_typing: bool,
}

// === Voice Types ===

/// opus-encoded voice packet
#[derive(Clone, Debug, Encode, Decode)]
pub struct VoiceData {
    /// sender seat
    pub seat: u8,
    /// sequence number (for ordering/jitter buffer)
    pub seq: u32,
    /// opus frame (typically 20ms at 48kHz mono = ~120 bytes)
    pub frame: Vec<u8>,
    /// voice activity detected (for UI indicator)
    pub vad: bool,
}

/// voice state change
#[derive(Clone, Debug, Encode, Decode)]
pub struct VoiceState {
    pub seat: u8,
    pub state: VoiceStateType,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Encode, Decode)]
pub enum VoiceStateType {
    /// player muted themselves
    Muted,
    /// player unmuted
    Unmuted,
    /// player started speaking (PTT pressed)
    Speaking,
    /// player stopped speaking (PTT released)
    Silent,
    /// player deafened (not receiving audio)
    Deafened,
    /// player left voice
    Disconnected,
}

impl Message {
    /// encode message to bytes
    pub fn encode_to_vec(&self) -> Vec<u8> {
        self.encode()
    }

    /// decode message from bytes
    pub fn decode_from_slice(data: &[u8]) -> Result<Self, parity_scale_codec::Error> {
        Self::decode(&mut &data[..])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_rules_validation() {
        let rules = TableRules::default();
        assert!(rules.validate().is_ok());

        let bad_seats = TableRules { seats: 1, ..Default::default() };
        assert!(bad_seats.validate().is_err());

        let bad_blinds = TableRules {
            small_blind: 100,
            big_blind: 50,
            ..Default::default()
        };
        assert!(bad_blinds.validate().is_err());
    }

    #[test]
    fn test_training_rules() {
        let rules = TableRules::training();
        assert_eq!(rules.tier, SecurityTier::Training);
        assert_eq!(rules.min_buy_in, 0);
        assert!(!rules.tier.allows_real_stakes());
    }

    #[test]
    fn test_message_encode_decode() {
        let msg = Message::Ping(12345);
        let encoded = msg.encode_to_vec();
        let decoded = Message::decode_from_slice(&encoded).unwrap();
        match decoded {
            Message::Ping(n) => assert_eq!(n, 12345),
            _ => panic!("wrong message type"),
        }
    }
}
