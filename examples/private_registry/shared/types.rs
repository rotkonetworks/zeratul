//! Shared types between client and server

use serde::{Deserialize, Serialize};

/// Card rarity levels
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Rarity {
    Common,
    Rare,
    Epic,
    Legendary,
}

impl Rarity {
    pub fn power_range(&self) -> (u32, u32) {
        match self {
            Rarity::Common => (1, 3),
            Rarity::Rare => (4, 6),
            Rarity::Epic => (7, 8),
            Rarity::Legendary => (9, 10),
        }
    }

    pub fn from_random(r: f64) -> Self {
        if r < 0.70 {
            Rarity::Common
        } else if r < 0.90 {
            Rarity::Rare
        } else if r < 0.97 {
            Rarity::Epic
        } else {
            Rarity::Legendary
        }
    }
}

/// Card type
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum CardType {
    Dragon,
    Phoenix,
    Unicorn,
    Griffin,
}

/// A collectible card (kept secret by client)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Card {
    pub id: String,
    pub card_type: CardType,
    pub rarity: Rarity,
    pub power: u32,
    /// Secret key - must be kept private!
    pub secret: [u8; 32],
}

/// Public commitment to a card
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Commitment(pub [u8; 32]);

/// A nullifier (spent card marker)
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Nullifier(pub [u8; 32]);

/// Mint request (create new card)
#[derive(Debug, Serialize, Deserialize)]
pub struct MintRequest {
    pub commitment: String, // hex-encoded
    pub proof: String,      // hex-encoded
}

/// Mint response
#[derive(Debug, Serialize, Deserialize)]
pub struct MintResponse {
    pub success: bool,
    pub message: String,
}

/// Prove ownership request
#[derive(Debug, Serialize, Deserialize)]
pub struct ProveRequest {
    pub challenge: String,     // "prove_legendary", "prove_dragon", etc.
    pub commitment: String,    // hex-encoded
    pub proof: String,         // hex-encoded
}

/// Prove response
#[derive(Debug, Serialize, Deserialize)]
pub struct ProveResponse {
    pub success: bool,
    pub message: String,
    pub achievement_unlocked: Option<String>,
}

/// Trade request
#[derive(Debug, Serialize, Deserialize)]
pub struct TradeRequest {
    pub from_commitment: String,
    pub to_commitment: String,
    pub proof_a: String,
    pub proof_b: String,
}

/// Leaderboard entry
#[derive(Debug, Serialize, Deserialize)]
pub struct LeaderboardEntry {
    pub player: String,
    pub total_cards: usize,
    pub legendaries: usize,
}

/// Leaderboard response
#[derive(Debug, Serialize, Deserialize)]
pub struct LeaderboardResponse {
    pub top_collectors: Vec<LeaderboardEntry>,
}
