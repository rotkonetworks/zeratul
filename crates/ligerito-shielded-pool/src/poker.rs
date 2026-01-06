//! poker game state for shielded pool channels
//!
//! integrates mental poker (zk-shuffle) with state channels.
//! all game state is private - only final balances settle to L1.
//!
//! # flow
//!
//! 1. players join channel with deposits (shielded notes)
//! 2. aggregate encryption key established
//! 3. each player shuffles + proves (zk-shuffle)
//! 4. deal cards (partial reveals with proofs)
//! 5. betting rounds (signed state updates)
//! 6. showdown (full reveals)
//! 7. pot distribution in channel
//! 8. settlement to L1 (shielded notes)

#[cfg(not(feature = "std"))]
use alloc::vec::Vec;

use crate::channel::ChannelError;
use crate::keys::PublicKey;
use crate::value::Amount;

/// poker game phase
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum GamePhase {
    /// waiting for players to join
    WaitingForPlayers = 0,
    /// key exchange phase
    KeyExchange = 1,
    /// shuffle phase (each player shuffles)
    Shuffle = 2,
    /// dealing hole cards
    DealHole = 3,
    /// pre-flop betting
    PreFlop = 4,
    /// dealing flop
    DealFlop = 5,
    /// flop betting
    FlopBet = 6,
    /// dealing turn
    DealTurn = 7,
    /// turn betting
    TurnBet = 8,
    /// dealing river
    DealRiver = 9,
    /// river betting
    RiverBet = 10,
    /// showdown
    Showdown = 11,
    /// hand complete
    Complete = 12,
}

impl GamePhase {
    pub fn from_u8(v: u8) -> Option<Self> {
        match v {
            0 => Some(GamePhase::WaitingForPlayers),
            1 => Some(GamePhase::KeyExchange),
            2 => Some(GamePhase::Shuffle),
            3 => Some(GamePhase::DealHole),
            4 => Some(GamePhase::PreFlop),
            5 => Some(GamePhase::DealFlop),
            6 => Some(GamePhase::FlopBet),
            7 => Some(GamePhase::DealTurn),
            8 => Some(GamePhase::TurnBet),
            9 => Some(GamePhase::DealRiver),
            10 => Some(GamePhase::RiverBet),
            11 => Some(GamePhase::Showdown),
            12 => Some(GamePhase::Complete),
            _ => None,
        }
    }
}

/// player's poker state within a hand
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PlayerPokerState {
    /// seat index (0-based)
    pub seat: u8,
    /// player's encryption key (for deck ops)
    pub encryption_key: [u8; 32],
    /// player's hole cards (encrypted indices)
    pub hole_cards: Vec<u8>,
    /// current bet amount this round
    pub current_bet: Amount,
    /// total contributed to pot this hand
    pub pot_contribution: Amount,
    /// whether player has folded
    pub folded: bool,
    /// whether player is all-in
    pub all_in: bool,
    /// whether player has acted this round
    pub has_acted: bool,
}

/// poker game state (stored in channel.app_data)
#[derive(Clone, Debug)]
pub struct PokerGameState {
    /// current phase
    pub phase: GamePhase,
    /// big blind amount
    pub big_blind: Amount,
    /// small blind amount (half of big_blind)
    pub small_blind: Amount,
    /// dealer button position (seat index)
    pub dealer: u8,
    /// current actor (seat index)
    pub current_actor: u8,
    /// per-player state
    pub players: Vec<PlayerPokerState>,
    /// community cards (encrypted indices)
    pub community: Vec<u8>,
    /// main pot
    pub pot: Amount,
    /// side pots for all-ins
    pub side_pots: Vec<SidePot>,
    /// minimum raise amount
    pub min_raise: Amount,
    /// highest bet this round
    pub current_bet: Amount,
    /// hand number (for sequencing)
    pub hand_number: u64,
    /// hash of shuffled deck commitment
    pub deck_commitment: [u8; 32],
    /// shuffle proofs from each player (compressed)
    pub shuffle_proof_hashes: Vec<[u8; 32]>,
}

/// side pot for all-in situations
#[derive(Clone, Debug)]
pub struct SidePot {
    /// amount in this pot
    pub amount: Amount,
    /// eligible players (seat indices)
    pub eligible: Vec<u8>,
}

impl PokerGameState {
    /// create new game
    pub fn new(big_blind: Amount, num_players: u8) -> Self {
        let small_blind = Amount(big_blind.0 / 2);

        Self {
            phase: GamePhase::WaitingForPlayers,
            big_blind,
            small_blind,
            dealer: 0,
            current_actor: 0,
            players: Vec::with_capacity(num_players as usize),
            community: Vec::new(),
            pot: Amount::ZERO,
            side_pots: Vec::new(),
            min_raise: big_blind,
            current_bet: Amount::ZERO,
            hand_number: 0,
            deck_commitment: [0u8; 32],
            shuffle_proof_hashes: Vec::new(),
        }
    }

    /// serialize to bytes for app_data
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut bytes = Vec::new();

        // phase (1 byte)
        bytes.push(self.phase as u8);

        // blinds (16 bytes each for u128)
        bytes.extend_from_slice(&self.big_blind.0.to_le_bytes());
        bytes.extend_from_slice(&self.small_blind.0.to_le_bytes());

        // positions (2 bytes)
        bytes.push(self.dealer);
        bytes.push(self.current_actor);

        // player count (1 byte)
        bytes.push(self.players.len() as u8);

        // players
        for p in &self.players {
            bytes.push(p.seat);
            bytes.extend_from_slice(&p.encryption_key);
            bytes.push(p.hole_cards.len() as u8);
            bytes.extend_from_slice(&p.hole_cards);
            bytes.extend_from_slice(&p.current_bet.0.to_le_bytes());
            bytes.extend_from_slice(&p.pot_contribution.0.to_le_bytes());
            bytes.push(if p.folded { 1 } else { 0 });
            bytes.push(if p.all_in { 1 } else { 0 });
            bytes.push(if p.has_acted { 1 } else { 0 });
        }

        // community cards
        bytes.push(self.community.len() as u8);
        bytes.extend_from_slice(&self.community);

        // pot info (u128 = 16 bytes each)
        bytes.extend_from_slice(&self.pot.0.to_le_bytes());
        bytes.extend_from_slice(&self.min_raise.0.to_le_bytes());
        bytes.extend_from_slice(&self.current_bet.0.to_le_bytes());
        bytes.extend_from_slice(&self.hand_number.to_le_bytes());

        // deck commitment
        bytes.extend_from_slice(&self.deck_commitment);

        // shuffle proof hashes
        bytes.push(self.shuffle_proof_hashes.len() as u8);
        for hash in &self.shuffle_proof_hashes {
            bytes.extend_from_slice(hash);
        }

        bytes
    }

    /// deserialize from bytes
    pub fn from_bytes(bytes: &[u8]) -> Option<Self> {
        if bytes.len() < 68 {
            // minimum: phase(1) + blinds(32) + positions(2) + player_count(1) + community_len(1)
            //          + pot_info(56) + deck_commitment(32) + proof_count(1) = 126 min
            return None;
        }

        let mut offset = 0;

        let phase = GamePhase::from_u8(bytes[offset])?;
        offset += 1;

        // blinds are u128 = 16 bytes each
        let big_blind = Amount(u128::from_le_bytes(bytes[offset..offset + 16].try_into().ok()?));
        offset += 16;
        let small_blind = Amount(u128::from_le_bytes(bytes[offset..offset + 16].try_into().ok()?));
        offset += 16;

        let dealer = bytes[offset];
        offset += 1;
        let current_actor = bytes[offset];
        offset += 1;

        let player_count = bytes[offset] as usize;
        offset += 1;

        let mut players = Vec::with_capacity(player_count);
        for _ in 0..player_count {
            if offset + 35 > bytes.len() {
                return None;
            }

            let seat = bytes[offset];
            offset += 1;

            let mut encryption_key = [0u8; 32];
            encryption_key.copy_from_slice(&bytes[offset..offset + 32]);
            offset += 32;

            let hole_count = bytes[offset] as usize;
            offset += 1;

            // current_bet(16) + pot_contribution(16) + flags(3) = 35 bytes after hole_cards
            if offset + hole_count + 35 > bytes.len() {
                return None;
            }

            let hole_cards = bytes[offset..offset + hole_count].to_vec();
            offset += hole_count;

            // u128 = 16 bytes
            let current_bet = Amount(u128::from_le_bytes(bytes[offset..offset + 16].try_into().ok()?));
            offset += 16;
            let pot_contribution = Amount(u128::from_le_bytes(bytes[offset..offset + 16].try_into().ok()?));
            offset += 16;

            let folded = bytes[offset] != 0;
            offset += 1;
            let all_in = bytes[offset] != 0;
            offset += 1;
            let has_acted = bytes[offset] != 0;
            offset += 1;

            players.push(PlayerPokerState {
                seat,
                encryption_key,
                hole_cards,
                current_bet,
                pot_contribution,
                folded,
                all_in,
                has_acted,
            });
        }

        if offset + 1 > bytes.len() {
            return None;
        }

        let community_count = bytes[offset] as usize;
        offset += 1;

        // pot(16) + min_raise(16) + current_bet(16) + hand_number(8) + deck_commitment(32) + proof_count(1) = 89
        if offset + community_count + 89 > bytes.len() {
            return None;
        }

        let community = bytes[offset..offset + community_count].to_vec();
        offset += community_count;

        // u128 = 16 bytes each
        let pot = Amount(u128::from_le_bytes(bytes[offset..offset + 16].try_into().ok()?));
        offset += 16;
        let min_raise = Amount(u128::from_le_bytes(bytes[offset..offset + 16].try_into().ok()?));
        offset += 16;
        let current_bet = Amount(u128::from_le_bytes(bytes[offset..offset + 16].try_into().ok()?));
        offset += 16;
        let hand_number = u64::from_le_bytes(bytes[offset..offset + 8].try_into().ok()?);
        offset += 8;

        let mut deck_commitment = [0u8; 32];
        deck_commitment.copy_from_slice(&bytes[offset..offset + 32]);
        offset += 32;

        if offset + 1 > bytes.len() {
            return None;
        }

        let proof_count = bytes[offset] as usize;
        offset += 1;

        if offset + proof_count * 32 > bytes.len() {
            return None;
        }

        let mut shuffle_proof_hashes = Vec::with_capacity(proof_count);
        for _ in 0..proof_count {
            let mut hash = [0u8; 32];
            hash.copy_from_slice(&bytes[offset..offset + 32]);
            shuffle_proof_hashes.push(hash);
            offset += 32;
        }

        Some(Self {
            phase,
            big_blind,
            small_blind,
            dealer,
            current_actor,
            players,
            community,
            pot,
            side_pots: Vec::new(), // side pots reconstructed from state
            min_raise,
            current_bet,
            hand_number,
            deck_commitment,
            shuffle_proof_hashes,
        })
    }

    /// count active players (not folded, not all-in)
    pub fn active_count(&self) -> usize {
        self.players.iter()
            .filter(|p| !p.folded && !p.all_in)
            .count()
    }

    /// count players still in hand (not folded)
    pub fn in_hand_count(&self) -> usize {
        self.players.iter()
            .filter(|p| !p.folded)
            .count()
    }

    /// get next actor seat (skipping folded/all-in)
    pub fn next_actor(&self, from: u8) -> Option<u8> {
        let n = self.players.len();
        for i in 1..=n {
            let seat = ((from as usize + i) % n) as u8;
            if let Some(p) = self.players.iter().find(|p| p.seat == seat) {
                if !p.folded && !p.all_in {
                    return Some(seat);
                }
            }
        }
        None
    }
}

/// poker-specific actions (encoded as AppAction.data)
#[derive(Clone, Debug)]
pub enum PokerAction {
    /// submit encryption key for shuffle
    SubmitKey { key: [u8; 32] },
    /// submit shuffle proof hash (actual proof sent P2P)
    SubmitShuffleProof { proof_hash: [u8; 32] },
    /// submit deck commitment after all shuffles
    CommitDeck { commitment: [u8; 32] },
    /// reveal cards (indices + decryption shares)
    RevealCards { card_indices: Vec<u8>, shares: Vec<[u8; 32]> },
    /// betting action
    Bet(BetAction),
    /// show hand at showdown
    ShowHand { hole_cards: Vec<u8>, hand_rank: u8 },
    /// claim pot (after showdown)
    ClaimPot { winner_seat: u8, amount: Amount },
}

/// betting actions
#[derive(Clone, Copy, Debug)]
pub enum BetAction {
    /// fold hand
    Fold,
    /// check (if no bet to call)
    Check,
    /// call current bet
    Call,
    /// raise by amount
    Raise(Amount),
    /// bet amount (when no current bet)
    Bet(Amount),
    /// all-in
    AllIn,
}

impl PokerAction {
    /// encode to bytes for AppAction.data
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut bytes = Vec::new();
        match self {
            PokerAction::SubmitKey { key } => {
                bytes.push(0x01);
                bytes.extend_from_slice(key);
            }
            PokerAction::SubmitShuffleProof { proof_hash } => {
                bytes.push(0x02);
                bytes.extend_from_slice(proof_hash);
            }
            PokerAction::CommitDeck { commitment } => {
                bytes.push(0x03);
                bytes.extend_from_slice(commitment);
            }
            PokerAction::RevealCards { card_indices, shares } => {
                bytes.push(0x04);
                bytes.push(card_indices.len() as u8);
                bytes.extend_from_slice(card_indices);
                for share in shares {
                    bytes.extend_from_slice(share);
                }
            }
            PokerAction::Bet(bet) => {
                bytes.push(0x05);
                match bet {
                    BetAction::Fold => bytes.push(0x00),
                    BetAction::Check => bytes.push(0x01),
                    BetAction::Call => bytes.push(0x02),
                    BetAction::Raise(amt) => {
                        bytes.push(0x03);
                        bytes.extend_from_slice(&amt.0.to_le_bytes()); // u128 = 16 bytes
                    }
                    BetAction::Bet(amt) => {
                        bytes.push(0x04);
                        bytes.extend_from_slice(&amt.0.to_le_bytes()); // u128 = 16 bytes
                    }
                    BetAction::AllIn => bytes.push(0x05),
                }
            }
            PokerAction::ShowHand { hole_cards, hand_rank } => {
                bytes.push(0x06);
                bytes.push(hole_cards.len() as u8);
                bytes.extend_from_slice(hole_cards);
                bytes.push(*hand_rank);
            }
            PokerAction::ClaimPot { winner_seat, amount } => {
                bytes.push(0x07);
                bytes.push(*winner_seat);
                bytes.extend_from_slice(&amount.0.to_le_bytes()); // u128 = 16 bytes
            }
        }
        bytes
    }

    /// decode from bytes
    pub fn from_bytes(bytes: &[u8]) -> Option<Self> {
        if bytes.is_empty() {
            return None;
        }

        match bytes[0] {
            0x01 if bytes.len() >= 33 => {
                let mut key = [0u8; 32];
                key.copy_from_slice(&bytes[1..33]);
                Some(PokerAction::SubmitKey { key })
            }
            0x02 if bytes.len() >= 33 => {
                let mut proof_hash = [0u8; 32];
                proof_hash.copy_from_slice(&bytes[1..33]);
                Some(PokerAction::SubmitShuffleProof { proof_hash })
            }
            0x03 if bytes.len() >= 33 => {
                let mut commitment = [0u8; 32];
                commitment.copy_from_slice(&bytes[1..33]);
                Some(PokerAction::CommitDeck { commitment })
            }
            0x05 if bytes.len() >= 2 => {
                let bet = match bytes[1] {
                    0x00 => BetAction::Fold,
                    0x01 => BetAction::Check,
                    0x02 => BetAction::Call,
                    0x03 if bytes.len() >= 18 => {
                        // 1 (tag) + 1 (bet tag) + 16 (u128)
                        let amt = u128::from_le_bytes(bytes[2..18].try_into().ok()?);
                        BetAction::Raise(Amount(amt))
                    }
                    0x04 if bytes.len() >= 18 => {
                        let amt = u128::from_le_bytes(bytes[2..18].try_into().ok()?);
                        BetAction::Bet(Amount(amt))
                    }
                    0x05 => BetAction::AllIn,
                    _ => return None,
                };
                Some(PokerAction::Bet(bet))
            }
            0x07 if bytes.len() >= 18 => {
                // 1 (tag) + 1 (seat) + 16 (u128)
                let winner_seat = bytes[1];
                let amount = Amount(u128::from_le_bytes(bytes[2..18].try_into().ok()?));
                Some(PokerAction::ClaimPot { winner_seat, amount })
            }
            _ => None,
        }
    }
}

/// validate a poker action against current state
pub fn validate_poker_action(
    state: &PokerGameState,
    actor_seat: u8,
    actor_balance: Amount,
    action: &PokerAction,
) -> Result<(), ChannelError> {
    match action {
        PokerAction::Bet(bet) => {
            // must be current actor
            if state.current_actor != actor_seat {
                return Err(ChannelError::InvalidProof);
            }
            validate_bet(state, actor_balance, bet)
        }
        PokerAction::SubmitKey { .. } => {
            if state.phase != GamePhase::KeyExchange {
                return Err(ChannelError::InvalidProof);
            }
            Ok(())
        }
        PokerAction::SubmitShuffleProof { .. } => {
            if state.phase != GamePhase::Shuffle {
                return Err(ChannelError::InvalidProof);
            }
            Ok(())
        }
        PokerAction::RevealCards { .. } => {
            // valid in deal phases and showdown
            match state.phase {
                GamePhase::DealHole | GamePhase::DealFlop |
                GamePhase::DealTurn | GamePhase::DealRiver |
                GamePhase::Showdown => Ok(()),
                _ => Err(ChannelError::InvalidProof),
            }
        }
        _ => Ok(())
    }
}

impl PokerGameState {
    /// apply a poker action and return new state
    pub fn apply_action(&self, actor_seat: u8, action: &PokerAction) -> Result<Self, ChannelError> {
        let mut new_state = self.clone();

        match action {
            PokerAction::SubmitKey { key } => {
                if let Some(player) = new_state.players.iter_mut().find(|p| p.seat == actor_seat) {
                    player.encryption_key = *key;
                }
                // check if all players submitted keys
                let all_keys = new_state.players.iter().all(|p| p.encryption_key != [0u8; 32]);
                if all_keys {
                    new_state.phase = GamePhase::Shuffle;
                    new_state.current_actor = 0;
                }
            }

            PokerAction::SubmitShuffleProof { proof_hash } => {
                new_state.shuffle_proof_hashes.push(*proof_hash);
                // move to next shuffler or advance phase
                if new_state.shuffle_proof_hashes.len() == new_state.players.len() {
                    new_state.phase = GamePhase::DealHole;
                } else {
                    new_state.current_actor = (actor_seat + 1) % new_state.players.len() as u8;
                }
            }

            PokerAction::CommitDeck { commitment } => {
                new_state.deck_commitment = *commitment;
            }

            PokerAction::RevealCards { card_indices, shares: _ } => {
                match new_state.phase {
                    GamePhase::DealHole => {
                        // each player's hole cards (2 cards per player)
                        if let Some(player) = new_state.players.iter_mut().find(|p| p.seat == actor_seat) {
                            player.hole_cards = card_indices.clone();
                        }
                        // check if all dealt
                        let all_dealt = new_state.players.iter().all(|p| p.hole_cards.len() == 2);
                        if all_dealt {
                            new_state.phase = GamePhase::PreFlop;
                            new_state.post_blinds()?;
                        }
                    }
                    GamePhase::DealFlop => {
                        new_state.community.extend_from_slice(card_indices);
                        new_state.phase = GamePhase::FlopBet;
                        new_state.reset_betting_round();
                    }
                    GamePhase::DealTurn => {
                        new_state.community.extend_from_slice(card_indices);
                        new_state.phase = GamePhase::TurnBet;
                        new_state.reset_betting_round();
                    }
                    GamePhase::DealRiver => {
                        new_state.community.extend_from_slice(card_indices);
                        new_state.phase = GamePhase::RiverBet;
                        new_state.reset_betting_round();
                    }
                    _ => {}
                }
            }

            PokerAction::Bet(bet) => {
                new_state.apply_bet(actor_seat, bet)?;
            }

            PokerAction::ShowHand { .. } => {
                // showdown tracking handled by app layer
            }

            PokerAction::ClaimPot { winner_seat, amount } => {
                // transfer pot to winner (validated externally)
                if let Some(winner) = new_state.players.iter_mut().find(|p| p.seat == *winner_seat) {
                    winner.pot_contribution = Amount::ZERO; // winner gets pot
                }
                new_state.pot = new_state.pot.saturating_sub(*amount);
                if new_state.pot.is_zero() {
                    new_state.phase = GamePhase::Complete;
                }
            }
        }

        Ok(new_state)
    }

    /// post blinds at start of hand
    fn post_blinds(&mut self) -> Result<(), ChannelError> {
        let n = self.players.len();
        if n < 2 {
            return Err(ChannelError::ParticipantNotFound);
        }

        // small blind is left of dealer
        let sb_seat = ((self.dealer as usize + 1) % n) as u8;
        // big blind is left of small blind
        let bb_seat = ((self.dealer as usize + 2) % n) as u8;

        if let Some(sb) = self.players.iter_mut().find(|p| p.seat == sb_seat) {
            sb.current_bet = self.small_blind;
            sb.pot_contribution = self.small_blind;
        }

        if let Some(bb) = self.players.iter_mut().find(|p| p.seat == bb_seat) {
            bb.current_bet = self.big_blind;
            bb.pot_contribution = self.big_blind;
        }

        self.pot = self.small_blind.saturating_add(self.big_blind);
        self.current_bet = self.big_blind;
        self.min_raise = self.big_blind;

        // action starts left of big blind
        self.current_actor = ((self.dealer as usize + 3) % n) as u8;

        Ok(())
    }

    /// reset for new betting round
    fn reset_betting_round(&mut self) {
        for p in &mut self.players {
            p.current_bet = Amount::ZERO;
            p.has_acted = false;
        }
        self.current_bet = Amount::ZERO;
        self.min_raise = self.big_blind;

        // action starts left of dealer
        if let Some(seat) = self.next_actor(self.dealer) {
            self.current_actor = seat;
        }
    }

    /// apply a betting action
    fn apply_bet(&mut self, actor_seat: u8, bet: &BetAction) -> Result<(), ChannelError> {
        let player = self.players.iter_mut()
            .find(|p| p.seat == actor_seat)
            .ok_or(ChannelError::ParticipantNotFound)?;

        match bet {
            BetAction::Fold => {
                player.folded = true;
            }
            BetAction::Check => {
                // valid only if no bet to call
            }
            BetAction::Call => {
                let call_amount = self.current_bet.saturating_sub(player.current_bet);
                player.current_bet = self.current_bet;
                player.pot_contribution = player.pot_contribution.saturating_add(call_amount);
                self.pot = self.pot.saturating_add(call_amount);
            }
            BetAction::Raise(amount) => {
                let total = self.current_bet.saturating_add(*amount);
                let raise_cost = total.saturating_sub(player.current_bet);
                player.current_bet = total;
                player.pot_contribution = player.pot_contribution.saturating_add(raise_cost);
                self.pot = self.pot.saturating_add(raise_cost);
                self.current_bet = total;
                self.min_raise = *amount;
            }
            BetAction::Bet(amount) => {
                player.current_bet = *amount;
                player.pot_contribution = player.pot_contribution.saturating_add(*amount);
                self.pot = self.pot.saturating_add(*amount);
                self.current_bet = *amount;
                self.min_raise = *amount;
            }
            BetAction::AllIn => {
                player.all_in = true;
                // actual amount handled by channel balance
            }
        }

        player.has_acted = true;

        // advance to next actor or next phase
        if self.is_betting_complete() {
            self.advance_phase();
        } else if let Some(next) = self.next_actor(actor_seat) {
            self.current_actor = next;
        }

        Ok(())
    }

    /// check if betting round is complete
    fn is_betting_complete(&self) -> bool {
        // one player remaining
        if self.in_hand_count() == 1 {
            return true;
        }

        // all active players have acted and matched current bet
        self.players.iter()
            .filter(|p| !p.folded && !p.all_in)
            .all(|p| p.has_acted && p.current_bet == self.current_bet)
    }

    /// advance to next phase after betting
    fn advance_phase(&mut self) {
        match self.phase {
            GamePhase::PreFlop => self.phase = GamePhase::DealFlop,
            GamePhase::FlopBet => self.phase = GamePhase::DealTurn,
            GamePhase::TurnBet => self.phase = GamePhase::DealRiver,
            GamePhase::RiverBet => self.phase = GamePhase::Showdown,
            _ => {}
        }
    }
}

fn validate_bet(
    state: &PokerGameState,
    balance: Amount,
    bet: &BetAction,
) -> Result<(), ChannelError> {
    match bet {
        BetAction::Fold => Ok(()),
        BetAction::Check => {
            if state.current_bet.0 > 0 {
                // can't check if there's a bet to call
                return Err(ChannelError::InvalidProof);
            }
            Ok(())
        }
        BetAction::Call => {
            // must have enough to call
            // actual call amount depends on player's current bet
            Ok(())
        }
        BetAction::Raise(amount) | BetAction::Bet(amount) => {
            if amount.0 < state.min_raise.0 {
                return Err(ChannelError::InsufficientBalance);
            }
            if amount.0 > balance.0 {
                return Err(ChannelError::InsufficientBalance);
            }
            Ok(())
        }
        BetAction::AllIn => Ok(()),
    }
}

/// poker channel: wraps a state channel with poker game state
pub struct PokerChannel {
    /// underlying state channel
    pub channel: crate::channel::Channel,
    /// current poker game state
    pub game: PokerGameState,
    /// our seat at the table
    pub our_seat: u8,
    /// mapping from seat -> public key
    pub seat_keys: Vec<PublicKey>,
}

impl PokerChannel {
    /// create new poker channel
    pub fn new(
        our_key: crate::keys::SpendKey,
        participants: Vec<crate::channel::Participant>,
        big_blind: Amount,
    ) -> Self {
        let our_pk = our_key.public_key();
        let our_seat = participants.iter()
            .position(|p| p.public_key == our_pk)
            .unwrap_or(0) as u8;

        let seat_keys: Vec<_> = participants.iter().map(|p| p.public_key).collect();
        let num_players = participants.len() as u8;

        let mut game = PokerGameState::new(big_blind, num_players);
        for (i, p) in participants.iter().enumerate() {
            game.players.push(PlayerPokerState {
                seat: i as u8,
                encryption_key: [0u8; 32],
                hole_cards: Vec::new(),
                current_bet: Amount::ZERO,
                pot_contribution: Amount::ZERO,
                folded: false,
                all_in: false,
                has_acted: false,
            });
            let _ = p;
        }
        game.phase = GamePhase::KeyExchange;

        let channel = crate::channel::Channel::new(our_key, participants);

        Self {
            channel,
            game,
            our_seat,
            seat_keys,
        }
    }

    /// apply a poker action
    pub fn apply_poker_action(&mut self, actor_seat: u8, action: &PokerAction) -> Result<(), ChannelError> {
        let actor_pk = self.seat_keys.get(actor_seat as usize)
            .ok_or(ChannelError::ParticipantNotFound)?;
        let balance = self.channel.current_state.state.balance_of(actor_pk)
            .unwrap_or(Amount::ZERO);

        // validate action
        validate_poker_action(&self.game, actor_seat, balance, action)?;

        // apply to poker state
        let new_game = self.game.apply_action(actor_seat, action)?;

        // handle balance transfers for bets
        if let PokerAction::Bet(bet) = action {
            self.apply_bet_to_channel(actor_seat, bet)?;
        }

        // handle pot claim
        if let PokerAction::ClaimPot { winner_seat, amount } = action {
            self.transfer_pot(*winner_seat, *amount)?;
        }

        // update game state
        self.game = new_game;

        // serialize poker state to channel app_data
        self.channel.current_state.state.app_data = self.game.to_bytes();

        // sign the new state
        self.channel.sign();

        Ok(())
    }

    fn apply_bet_to_channel(&mut self, actor_seat: u8, bet: &BetAction) -> Result<(), ChannelError> {
        let actor_pk = self.seat_keys[actor_seat as usize];
        let player = self.game.players.iter()
            .find(|p| p.seat == actor_seat)
            .ok_or(ChannelError::ParticipantNotFound)?;

        // calculate how much goes to pot
        let amount = match bet {
            BetAction::Fold | BetAction::Check => Amount::ZERO,
            BetAction::Call => self.game.current_bet.saturating_sub(player.current_bet),
            BetAction::Raise(amt) => {
                let total = self.game.current_bet.saturating_add(*amt);
                total.saturating_sub(player.current_bet)
            }
            BetAction::Bet(amt) => *amt,
            BetAction::AllIn => {
                self.channel.current_state.state.balance_of(&actor_pk)
                    .unwrap_or(Amount::ZERO)
            }
        };

        if amount.0 > 0 {
            // deduct from player's channel balance
            if let Some(p) = self.channel.current_state.state.participants.iter_mut()
                .find(|p| p.public_key == actor_pk) {
                p.balance = p.balance.saturating_sub(amount);
            }
        }

        Ok(())
    }

    fn transfer_pot(&mut self, winner_seat: u8, amount: Amount) -> Result<(), ChannelError> {
        let winner_pk = self.seat_keys[winner_seat as usize];

        if let Some(p) = self.channel.current_state.state.participants.iter_mut()
            .find(|p| p.public_key == winner_pk) {
            p.balance = p.balance.saturating_add(amount);
        }

        Ok(())
    }

    /// get current game phase
    pub fn phase(&self) -> GamePhase {
        self.game.phase
    }

    /// check if it's our turn
    pub fn is_our_turn(&self) -> bool {
        self.game.current_actor == self.our_seat
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_game_state_serialization() {
        let mut state = PokerGameState::new(100u64.into(), 2);
        state.phase = GamePhase::PreFlop;
        state.players.push(PlayerPokerState {
            seat: 0,
            encryption_key: [1u8; 32],
            hole_cards: vec![0, 1],
            current_bet: 50u64.into(),
            pot_contribution: 50u64.into(),
            folded: false,
            all_in: false,
            has_acted: true,
        });
        state.players.push(PlayerPokerState {
            seat: 1,
            encryption_key: [2u8; 32],
            hole_cards: vec![2, 3],
            current_bet: 100u64.into(),
            pot_contribution: 100u64.into(),
            folded: false,
            all_in: false,
            has_acted: true,
        });
        state.pot = 150u64.into();
        state.current_bet = 100u64.into();

        let bytes = state.to_bytes();
        let recovered = PokerGameState::from_bytes(&bytes).unwrap();

        assert_eq!(recovered.phase, GamePhase::PreFlop);
        assert_eq!(recovered.players.len(), 2);
        assert_eq!(recovered.pot.0, 150);
        assert_eq!(recovered.current_bet.0, 100);
    }

    #[test]
    fn test_poker_action_serialization() {
        let actions = vec![
            PokerAction::SubmitKey { key: [42u8; 32] },
            PokerAction::Bet(BetAction::Fold),
            PokerAction::Bet(BetAction::Raise(200u64.into())),
            PokerAction::ClaimPot { winner_seat: 1, amount: 500u64.into() },
        ];

        for action in actions {
            let bytes = action.to_bytes();
            let recovered = PokerAction::from_bytes(&bytes);
            assert!(recovered.is_some());
        }
    }

    #[test]
    fn test_active_count() {
        let mut state = PokerGameState::new(100u64.into(), 3);
        state.players = vec![
            PlayerPokerState {
                seat: 0,
                encryption_key: [0u8; 32],
                hole_cards: vec![],
                current_bet: 0u64.into(),
                pot_contribution: 0u64.into(),
                folded: false,
                all_in: false,
                has_acted: false,
            },
            PlayerPokerState {
                seat: 1,
                encryption_key: [1u8; 32],
                hole_cards: vec![],
                current_bet: 0u64.into(),
                pot_contribution: 0u64.into(),
                folded: true,
                all_in: false,
                has_acted: false,
            },
            PlayerPokerState {
                seat: 2,
                encryption_key: [2u8; 32],
                hole_cards: vec![],
                current_bet: 0u64.into(),
                pot_contribution: 0u64.into(),
                folded: false,
                all_in: true,
                has_acted: false,
            },
        ];

        assert_eq!(state.active_count(), 1); // only seat 0
        assert_eq!(state.in_hand_count(), 2); // seats 0 and 2
    }
}
