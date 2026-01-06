//! reveal audit for mental poker state channel settlement
//!
//! tracks card reveals, detects cheating, handles disconnections
//!
//! security properties:
//! - detects duplicate/invalid cards (shuffle corruption)
//! - supports forced reveal via threshold reconstruction (VSS)
//! - provides cryptographic proof for on-chain disputes

#[cfg(not(feature = "std"))]
use alloc::{format, string::String, vec, vec::Vec, collections::BTreeSet};

#[cfg(feature = "std")]
use std::collections::BTreeSet;

use blake2::{Blake2s256, Digest};

use crate::{Result, ShuffleError};
// for forced reveals, use ligerito-escrow::shares::{SecretSharer, ShareSet} externally

/// card value (0-51 for standard deck)
pub type CardValue = u8;

/// player identifier
pub type PlayerId = u8;

/// block/slot number for timeouts
pub type BlockNumber = u64;

/// cheat detection result
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CheatType {
    /// same card revealed twice
    DuplicateCard { card: CardValue, positions: (u8, u8) },
    /// card not in original deck (value out of range or deck corrupted)
    InvalidCard { position: u8, claimed_value: CardValue },
    /// decryption proof invalid
    InvalidDecryptionProof { position: u8, player: PlayerId },
    /// player timed out (didn't reveal in time)
    RevealTimeout { player: PlayerId },
}

/// proof that a card was correctly revealed
#[derive(Clone, Debug)]
pub enum RevealProof {
    /// normal reveal - player provided their decryption share
    PlayerReveal {
        /// the revealing player
        player: PlayerId,
        /// their decryption share
        share: [u8; 32],
        /// signature over (game_id, position, share)
        signature: [u8; 64],
    },
    /// threshold reveal - k-of-n players reconstructed the key
    ThresholdReveal {
        /// which players contributed shares
        contributors: Vec<PlayerId>,
        /// their shares
        shares: Vec<[u8; 32]>,
        /// the reconstructed decryption key
        reconstructed_key: [u8; 32],
    },
    /// forced reveal - player timed out, key reconstructed from VSS
    ForcedReveal {
        /// player who timed out
        timed_out_player: PlayerId,
        /// VSS shares used for reconstruction
        vss_shares: Vec<VssShare>,
        /// the reconstructed key
        reconstructed_key: [u8; 32],
    },
}

/// a share from verifiable secret sharing
#[derive(Clone, Debug)]
pub struct VssShare {
    /// which player holds this share
    pub holder: PlayerId,
    /// the share value
    pub share: [u8; 32],
    /// commitment proof (for verification)
    pub commitment_proof: [u8; 32],
}

/// a revealed card with proof
#[derive(Clone, Debug)]
pub struct RevealedCard {
    /// position in the shuffled deck (0-51)
    pub position: u8,
    /// the revealed card value (0-51)
    pub card: CardValue,
    /// block when revealed
    pub revealed_at: BlockNumber,
    /// proof of correct decryption
    pub proof: RevealProof,
}

/// player's reveal state
#[derive(Clone, Debug, Default)]
pub struct PlayerRevealState {
    /// player id
    pub player_id: PlayerId,
    /// have they revealed their hole cards?
    pub hole_cards_revealed: bool,
    /// have they provided decryption shares for community cards?
    pub community_shares_provided: bool,
    /// did they time out?
    pub timed_out: bool,
    /// block when they must reveal by (None = no pending requirement)
    pub reveal_deadline: Option<BlockNumber>,
}

/// hand result for settlement
#[derive(Clone, Debug)]
pub struct HandResult {
    /// game identifier
    pub game_id: [u8; 32],
    /// hand number within game
    pub hand_number: u64,
    /// final pot amount
    pub pot: u64,
    /// winners and their shares
    pub winners: Vec<(PlayerId, u64)>,
    /// any detected cheating
    pub cheats_detected: Vec<(PlayerId, CheatType)>,
    /// merkle root of all revealed cards (for on-chain verification)
    pub reveal_root: [u8; 32],
}

/// the main audit structure
#[derive(Clone, Debug)]
pub struct RevealAudit {
    // === game identification ===
    /// unique game id
    pub game_id: [u8; 32],
    /// hand number
    pub hand_number: u64,
    /// number of players
    pub num_players: u8,
    /// deck size (usually 52)
    pub deck_size: u8,

    // === deck state ===
    /// commitment to the encrypted deck (after all shuffles)
    pub encrypted_deck_commitment: [u8; 32],
    /// expected card values (0..deck_size)
    expected_cards: BTreeSet<CardValue>,

    // === reveal tracking ===
    /// cards revealed so far
    revealed_cards: Vec<RevealedCard>,
    /// set of revealed card values (for duplicate detection)
    revealed_values: BTreeSet<CardValue>,
    /// player states
    player_states: Vec<PlayerRevealState>,

    // === timing ===
    /// current block (updated externally)
    pub current_block: BlockNumber,
    /// blocks allowed for reveal (timeout period)
    pub reveal_timeout_blocks: BlockNumber,

    // === dispute ===
    /// detected cheating
    cheats: Vec<(PlayerId, CheatType)>,
}

impl RevealAudit {
    /// create new audit for a hand
    pub fn new(
        game_id: [u8; 32],
        hand_number: u64,
        num_players: u8,
        deck_size: u8,
        encrypted_deck_commitment: [u8; 32],
        reveal_timeout_blocks: BlockNumber,
    ) -> Self {
        let expected_cards: BTreeSet<CardValue> = (0..deck_size).collect();
        let player_states = (0..num_players)
            .map(|i| PlayerRevealState {
                player_id: i,
                ..Default::default()
            })
            .collect();

        Self {
            game_id,
            hand_number,
            num_players,
            deck_size,
            encrypted_deck_commitment,
            expected_cards,
            revealed_cards: Vec::new(),
            revealed_values: BTreeSet::new(),
            player_states,
            current_block: 0,
            reveal_timeout_blocks,
            cheats: Vec::new(),
        }
    }

    /// standard 52-card poker game
    pub fn new_poker(
        game_id: [u8; 32],
        hand_number: u64,
        num_players: u8,
        encrypted_deck_commitment: [u8; 32],
    ) -> Self {
        Self::new(
            game_id,
            hand_number,
            num_players,
            52,
            encrypted_deck_commitment,
            100, // ~10 minutes at 6s blocks
        )
    }

    /// update current block (call before processing reveals)
    pub fn set_current_block(&mut self, block: BlockNumber) {
        self.current_block = block;
        self.check_timeouts();
    }

    /// require a player to reveal by deadline
    pub fn require_reveal(&mut self, player: PlayerId, deadline: BlockNumber) {
        if let Some(state) = self.player_states.get_mut(player as usize) {
            state.reveal_deadline = Some(deadline);
        }
    }

    /// check for timed out players
    fn check_timeouts(&mut self) {
        for state in &mut self.player_states {
            if let Some(deadline) = state.reveal_deadline {
                if self.current_block > deadline && !state.hole_cards_revealed && !state.timed_out {
                    state.timed_out = true;
                    self.cheats.push((
                        state.player_id,
                        CheatType::RevealTimeout { player: state.player_id },
                    ));
                }
            }
        }
    }

    /// record a card reveal
    pub fn record_reveal(&mut self, revealed: RevealedCard) -> Result<()> {
        let card = revealed.card;
        let position = revealed.position;

        // check card is in valid range
        if card >= self.deck_size {
            self.cheats.push((
                self.get_revealing_player(&revealed.proof),
                CheatType::InvalidCard { position, claimed_value: card },
            ));
            return Err(ShuffleError::VerificationError(
                format!("invalid card value {} at position {}", card, position)
            ));
        }

        // check for duplicate
        if self.revealed_values.contains(&card) {
            // find the other position
            let other_pos = self.revealed_cards
                .iter()
                .find(|r| r.card == card)
                .map(|r| r.position)
                .unwrap_or(0);

            self.cheats.push((
                self.get_revealing_player(&revealed.proof),
                CheatType::DuplicateCard {
                    card,
                    positions: (other_pos, position)
                },
            ));
            return Err(ShuffleError::VerificationError(
                format!("duplicate card {} at positions {} and {}", card, other_pos, position)
            ));
        }

        // verify proof (simplified - real impl would do crypto verification)
        if !self.verify_reveal_proof(&revealed) {
            self.cheats.push((
                self.get_revealing_player(&revealed.proof),
                CheatType::InvalidDecryptionProof {
                    position,
                    player: self.get_revealing_player(&revealed.proof)
                },
            ));
            return Err(ShuffleError::VerificationError(
                "invalid decryption proof".into()
            ));
        }

        // record the reveal
        self.revealed_values.insert(card);
        self.revealed_cards.push(revealed);

        Ok(())
    }

    /// get the player responsible for a reveal
    fn get_revealing_player(&self, proof: &RevealProof) -> PlayerId {
        match proof {
            RevealProof::PlayerReveal { player, .. } => *player,
            RevealProof::ThresholdReveal { contributors, .. } => {
                contributors.first().copied().unwrap_or(0)
            }
            RevealProof::ForcedReveal { timed_out_player, .. } => *timed_out_player,
        }
    }

    /// verify a reveal proof (simplified)
    fn verify_reveal_proof(&self, revealed: &RevealedCard) -> bool {
        match &revealed.proof {
            RevealProof::PlayerReveal { player, share, signature } => {
                // verify signature over (game_id, position, share)
                // simplified: just check non-zero
                *player < self.num_players &&
                *share != [0u8; 32] &&
                *signature != [0u8; 64]
            }
            RevealProof::ThresholdReveal { contributors, shares, reconstructed_key } => {
                // verify threshold (need k of n)
                // simplified: need at least 2 contributors
                contributors.len() >= 2 &&
                shares.len() == contributors.len() &&
                *reconstructed_key != [0u8; 32]
            }
            RevealProof::ForcedReveal { vss_shares, reconstructed_key, .. } => {
                // verify VSS shares combine to reconstructed key
                // simplified: need at least 2 shares
                vss_shares.len() >= 2 && *reconstructed_key != [0u8; 32]
            }
        }
    }

    /// mark player's hole cards as revealed
    pub fn mark_hole_cards_revealed(&mut self, player: PlayerId) {
        if let Some(state) = self.player_states.get_mut(player as usize) {
            state.hole_cards_revealed = true;
            state.reveal_deadline = None; // clear deadline
        }
    }

    /// check if all required reveals are complete
    pub fn all_reveals_complete(&self) -> bool {
        self.player_states.iter().all(|s| {
            s.hole_cards_revealed || s.timed_out
        })
    }

    /// get detected cheats
    pub fn get_cheats(&self) -> &[(PlayerId, CheatType)] {
        &self.cheats
    }

    /// has any cheating been detected?
    pub fn has_cheating(&self) -> bool {
        !self.cheats.is_empty()
    }

    /// compute merkle root of revealed cards (for on-chain proof)
    pub fn compute_reveal_root(&self) -> [u8; 32] {
        let mut hasher = Blake2s256::new();
        hasher.update(b"reveal_audit_root");
        hasher.update(&self.game_id);
        hasher.update(&self.hand_number.to_le_bytes());

        for revealed in &self.revealed_cards {
            hasher.update(&[revealed.position]);
            hasher.update(&[revealed.card]);
            hasher.update(&revealed.revealed_at.to_le_bytes());
        }

        let mut root = [0u8; 32];
        root.copy_from_slice(&hasher.finalize());
        root
    }

    /// finalize and produce settlement result
    pub fn finalize(
        &self,
        pot: u64,
        winners: Vec<(PlayerId, u64)>,
    ) -> HandResult {
        HandResult {
            game_id: self.game_id,
            hand_number: self.hand_number,
            pot,
            winners,
            cheats_detected: self.cheats.clone(),
            reveal_root: self.compute_reveal_root(),
        }
    }

    /// get players who should forfeit (cheated or timed out)
    pub fn get_forfeiting_players(&self) -> Vec<PlayerId> {
        self.cheats.iter().map(|(p, _)| *p).collect()
    }
}

/// settlement decision based on audit
#[derive(Clone, Debug)]
pub enum SettlementDecision {
    /// normal settlement - winners split pot
    Normal { winners: Vec<(PlayerId, u64)> },
    /// player(s) cheated - they forfeit, others split
    CheatPenalty {
        cheaters: Vec<PlayerId>,
        innocent_split: Vec<(PlayerId, u64)>,
    },
    /// timeout - timed out player forfeits
    TimeoutForfeit {
        timed_out: PlayerId,
        winner: PlayerId,
        amount: u64,
    },
    /// dispute required - needs on-chain resolution
    Dispute {
        reason: String,
        evidence_root: [u8; 32],
    },
}

impl RevealAudit {
    /// compute settlement decision
    pub fn compute_settlement(
        &self,
        pot: u64,
        hand_winners: Vec<PlayerId>, // winners by poker rules
    ) -> SettlementDecision {
        let forfeiting = self.get_forfeiting_players();

        if forfeiting.is_empty() {
            // normal case - split among winners
            let share = pot / hand_winners.len() as u64;
            let winners: Vec<_> = hand_winners.iter()
                .map(|&p| (p, share))
                .collect();
            SettlementDecision::Normal { winners }
        } else {
            // someone cheated or timed out
            let innocent: Vec<_> = (0..self.num_players)
                .filter(|p| !forfeiting.contains(p))
                .collect();

            if innocent.is_empty() {
                // everyone cheated? dispute needed
                SettlementDecision::Dispute {
                    reason: "all players flagged for cheating".into(),
                    evidence_root: self.compute_reveal_root(),
                }
            } else {
                // split among innocent players
                let share = pot / innocent.len() as u64;
                let innocent_split: Vec<_> = innocent.iter()
                    .map(|&p| (p, share))
                    .collect();
                SettlementDecision::CheatPenalty {
                    cheaters: forfeiting,
                    innocent_split,
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_reveal(position: u8, card: CardValue, player: PlayerId) -> RevealedCard {
        RevealedCard {
            position,
            card,
            revealed_at: 100,
            proof: RevealProof::PlayerReveal {
                player,
                share: [1u8; 32],
                signature: [2u8; 64],
            },
        }
    }

    #[test]
    fn test_normal_reveal() {
        let mut audit = RevealAudit::new_poker(
            [0u8; 32],
            1,
            4,
            [1u8; 32],
        );

        // reveal some cards normally
        audit.record_reveal(make_reveal(0, 0, 0)).unwrap();  // Ace of Spades
        audit.record_reveal(make_reveal(1, 13, 0)).unwrap(); // Ace of Hearts
        audit.record_reveal(make_reveal(2, 26, 1)).unwrap(); // Ace of Diamonds

        assert!(!audit.has_cheating());
        assert_eq!(audit.revealed_cards.len(), 3);
    }

    #[test]
    fn test_duplicate_detection() {
        let mut audit = RevealAudit::new_poker(
            [0u8; 32],
            1,
            4,
            [1u8; 32],
        );

        audit.record_reveal(make_reveal(0, 5, 0)).unwrap();

        // try to reveal same card at different position
        let result = audit.record_reveal(make_reveal(10, 5, 1));

        assert!(result.is_err());
        assert!(audit.has_cheating());

        match &audit.get_cheats()[0].1 {
            CheatType::DuplicateCard { card, positions } => {
                assert_eq!(*card, 5);
                assert_eq!(*positions, (0, 10));
            }
            _ => panic!("wrong cheat type"),
        }
    }

    #[test]
    fn test_invalid_card() {
        let mut audit = RevealAudit::new_poker(
            [0u8; 32],
            1,
            4,
            [1u8; 32],
        );

        // try to reveal card value 99 (invalid for 52-card deck)
        let result = audit.record_reveal(make_reveal(0, 99, 0));

        assert!(result.is_err());
        assert!(audit.has_cheating());

        match &audit.get_cheats()[0].1 {
            CheatType::InvalidCard { position, claimed_value } => {
                assert_eq!(*position, 0);
                assert_eq!(*claimed_value, 99);
            }
            _ => panic!("wrong cheat type"),
        }
    }

    #[test]
    fn test_timeout_detection() {
        let mut audit = RevealAudit::new_poker(
            [0u8; 32],
            1,
            4,
            [1u8; 32],
        );

        // require player 2 to reveal by block 50
        audit.require_reveal(2, 50);

        // advance to block 51
        audit.set_current_block(51);

        assert!(audit.has_cheating());

        match &audit.get_cheats()[0].1 {
            CheatType::RevealTimeout { player } => {
                assert_eq!(*player, 2);
            }
            _ => panic!("wrong cheat type"),
        }
    }

    #[test]
    fn test_settlement_normal() {
        let mut audit = RevealAudit::new_poker(
            [0u8; 32],
            1,
            4,
            [1u8; 32],
        );

        // all good, no cheating
        let settlement = audit.compute_settlement(1000, vec![0, 2]);

        match settlement {
            SettlementDecision::Normal { winners } => {
                assert_eq!(winners.len(), 2);
                assert_eq!(winners[0], (0, 500));
                assert_eq!(winners[1], (2, 500));
            }
            _ => panic!("expected normal settlement"),
        }
    }

    #[test]
    fn test_settlement_cheat_penalty() {
        let mut audit = RevealAudit::new_poker(
            [0u8; 32],
            1,
            4,
            [1u8; 32],
        );

        // player 1 reveals duplicate (cheating)
        audit.record_reveal(make_reveal(0, 5, 0)).unwrap();
        let _ = audit.record_reveal(make_reveal(10, 5, 1)); // fails, marks cheater

        let settlement = audit.compute_settlement(1000, vec![1]); // cheater "won"

        match settlement {
            SettlementDecision::CheatPenalty { cheaters, innocent_split } => {
                assert_eq!(cheaters, vec![1]);
                // pot split among 3 innocent players
                assert_eq!(innocent_split.len(), 3);
            }
            _ => panic!("expected cheat penalty"),
        }
    }

    #[test]
    fn test_reveal_root() {
        let mut audit = RevealAudit::new_poker(
            [0u8; 32],
            1,
            4,
            [1u8; 32],
        );

        audit.record_reveal(make_reveal(0, 0, 0)).unwrap();
        audit.record_reveal(make_reveal(1, 1, 0)).unwrap();

        let root1 = audit.compute_reveal_root();

        // same reveals should give same root
        let mut audit2 = RevealAudit::new_poker(
            [0u8; 32],
            1,
            4,
            [1u8; 32],
        );
        audit2.record_reveal(make_reveal(0, 0, 0)).unwrap();
        audit2.record_reveal(make_reveal(1, 1, 0)).unwrap();

        assert_eq!(root1, audit2.compute_reveal_root());

        // different reveals should give different root
        let mut audit3 = RevealAudit::new_poker(
            [0u8; 32],
            1,
            4,
            [1u8; 32],
        );
        audit3.record_reveal(make_reveal(0, 0, 0)).unwrap();
        audit3.record_reveal(make_reveal(1, 2, 0)).unwrap(); // different card

        assert_ne!(root1, audit3.compute_reveal_root());
    }
}
