//! poker game state for state channels
//!
//! represents complete game state that all players agree on

use alloc::vec::Vec;
use scale_codec::{Decode, Encode};
use scale_info::TypeInfo;

use crate::types::*;

/// game phase
#[derive(Clone, Copy, Debug, Encode, Decode, TypeInfo, PartialEq, Eq, Default)]
pub enum Phase {
    #[default]
    Lobby,
    /// shuffle in progress, waiting for proofs
    Shuffling,
    /// cards dealt, pre-flop betting
    PreFlop,
    /// 3 community cards revealed
    Flop,
    /// 4th community card
    Turn,
    /// 5th community card
    River,
    /// cards revealed, determining winner
    Showdown,
    /// hand complete, ready for next
    Complete,
}

/// player state within a hand
#[derive(Clone, Debug, Encode, Decode, TypeInfo, PartialEq, Eq)]
pub struct PlayerState {
    pub seat: Seat,
    pub chips: Balance,
    pub current_bet: Balance,
    pub total_bet_this_hand: Balance,
    pub is_folded: bool,
    pub is_all_in: bool,
    /// encrypted hole cards (indices into shuffled deck)
    pub hole_card_indices: Option<[CardIndex; 2]>,
    /// revealed hole cards (after showdown or fold)
    pub revealed_cards: Option<[CardIndex; 2]>,
}

impl PlayerState {
    pub fn new(seat: Seat, chips: Balance) -> Self {
        Self {
            seat,
            chips,
            current_bet: 0,
            total_bet_this_hand: 0,
            is_folded: false,
            is_all_in: false,
            hole_card_indices: None,
            revealed_cards: None,
        }
    }

    pub fn can_act(&self) -> bool {
        !self.is_folded && !self.is_all_in && self.chips > 0
    }

    pub fn reset_for_new_hand(&mut self, chips: Balance) {
        self.chips = chips;
        self.current_bet = 0;
        self.total_bet_this_hand = 0;
        self.is_folded = false;
        self.is_all_in = false;
        self.hole_card_indices = None;
        self.revealed_cards = None;
    }
}

/// main pot and side pots
#[derive(Clone, Debug, Encode, Decode, TypeInfo, PartialEq, Eq, Default)]
pub struct Pot {
    /// total chips in pot
    pub total: Balance,
    /// eligible players for this pot (by seat)
    pub eligible: Vec<Seat>,
}

/// complete poker game state
#[derive(Clone, Debug, Encode, Decode, TypeInfo, PartialEq, Eq)]
pub struct PokerState {
    /// channel this game belongs to
    pub channel_id: ChannelId,
    /// current hand number
    pub hand_number: u64,
    /// game phase
    pub phase: Phase,
    /// all players
    pub players: Vec<PlayerState>,
    /// dealer button position
    pub dealer_seat: Seat,
    /// whose turn to act
    pub action_seat: Seat,
    /// current bet to call
    pub current_bet: Balance,
    /// minimum raise amount
    pub min_raise: Balance,
    /// big blind amount
    pub big_blind: Balance,
    /// main pot
    pub pot: Pot,
    /// side pots (for all-in situations)
    pub side_pots: Vec<Pot>,
    /// community card indices (revealed cards)
    pub community_cards: Vec<CardIndex>,
    /// shuffle commitment for current deck
    pub shuffle_commitment: ShuffleCommitment,
    /// ligerito proof for shuffle (stored for dispute)
    pub shuffle_proof: Option<LigeritoProof>,
    /// reveal tokens collected this hand
    pub reveal_tokens: Vec<RevealToken>,
    /// last action timestamp (for timeouts)
    pub last_action_time: u64,
}

impl PokerState {
    /// create new game state
    pub fn new(channel_id: ChannelId, participants: &[Participant], big_blind: Balance) -> Self {
        let players: Vec<PlayerState> = participants
            .iter()
            .map(|p| PlayerState::new(p.seat, p.stake))
            .collect();

        Self {
            channel_id,
            hand_number: 0,
            phase: Phase::Lobby,
            players,
            dealer_seat: 0,
            action_seat: 0,
            current_bet: 0,
            min_raise: big_blind,
            big_blind,
            pot: Pot::default(),
            side_pots: Vec::new(),
            community_cards: Vec::new(),
            shuffle_commitment: ShuffleCommitment::default(),
            shuffle_proof: None,
            reveal_tokens: Vec::new(),
            last_action_time: 0,
        }
    }

    /// number of active players (not folded)
    pub fn active_player_count(&self) -> usize {
        self.players.iter().filter(|p| !p.is_folded).count()
    }

    /// number of players who can still act
    pub fn players_who_can_act(&self) -> usize {
        self.players.iter().filter(|p| p.can_act()).count()
    }

    /// get player by seat
    pub fn get_player(&self, seat: Seat) -> Option<&PlayerState> {
        self.players.iter().find(|p| p.seat == seat)
    }

    /// get mutable player by seat
    pub fn get_player_mut(&mut self, seat: Seat) -> Option<&mut PlayerState> {
        self.players.iter_mut().find(|p| p.seat == seat)
    }

    /// find next active seat after given seat
    pub fn next_active_seat(&self, from: Seat) -> Option<Seat> {
        let n = self.players.len();
        for i in 1..=n {
            let seat = ((from as usize + i) % n) as Seat;
            if let Some(player) = self.get_player(seat) {
                if player.can_act() {
                    return Some(seat);
                }
            }
        }
        None
    }

    /// check if betting round is complete
    pub fn is_betting_round_complete(&self) -> bool {
        // betting is complete when all active players have matched the current bet
        // or are all-in
        let players_to_act: Vec<_> = self.players
            .iter()
            .filter(|p| !p.is_folded && !p.is_all_in)
            .collect();

        if players_to_act.is_empty() {
            return true;
        }

        // all remaining players must have equal bets
        let first_bet = players_to_act.first().map(|p| p.current_bet).unwrap_or(0);
        players_to_act.iter().all(|p| p.current_bet == first_bet && p.current_bet >= self.current_bet)
    }

    /// compute state hash for signing
    pub fn hash(&self) -> H256 {
        let encoded = self.encode();
        H256::from(blake3::hash(&encoded).as_bytes())
    }

    /// small blind seat (left of dealer)
    pub fn small_blind_seat(&self) -> Seat {
        self.next_active_seat(self.dealer_seat).unwrap_or(self.dealer_seat)
    }

    /// big blind seat (left of small blind)
    pub fn big_blind_seat(&self) -> Seat {
        let sb = self.small_blind_seat();
        self.next_active_seat(sb).unwrap_or(sb)
    }
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
            Participant {
                account: PublicKey::from_raw([3u8; 32]),
                seat: 2,
                stake: 1000,
                encryption_key: vec![],
            },
        ]
    }

    #[test]
    fn test_new_game() {
        let channel_id = H256::zero();
        let state = PokerState::new(channel_id, &mock_participants(), 10);

        assert_eq!(state.players.len(), 3);
        assert_eq!(state.phase, Phase::Lobby);
        assert_eq!(state.big_blind, 10);
    }

    #[test]
    fn test_active_player_count() {
        let channel_id = H256::zero();
        let mut state = PokerState::new(channel_id, &mock_participants(), 10);

        assert_eq!(state.active_player_count(), 3);

        state.players[0].is_folded = true;
        assert_eq!(state.active_player_count(), 2);
    }

    #[test]
    fn test_next_active_seat() {
        let channel_id = H256::zero();
        let mut state = PokerState::new(channel_id, &mock_participants(), 10);

        assert_eq!(state.next_active_seat(0), Some(1));
        assert_eq!(state.next_active_seat(2), Some(0));

        state.players[1].is_folded = true;
        assert_eq!(state.next_active_seat(0), Some(2));
    }
}
