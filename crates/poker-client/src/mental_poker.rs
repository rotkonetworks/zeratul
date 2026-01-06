//! mental poker protocol integration
//!
//! implements the distributed shuffle protocol using zk-shuffle for
//! provably fair card dealing without a trusted dealer
//!
//! ## protocol overview
//!
//! 1. key setup: each player generates keypair, publishes pk
//! 2. deck masking: initial player masks all 52 cards with aggregate pubkey
//! 3. shuffle rounds: each player shuffles + remasks + proves
//! 4. card reveal: when card needed, all players provide reveal tokens

#![allow(dead_code)]

use zk_shuffle::{
    poker::{Card, Rank, Suit},
    remasking::ElGamalCiphertext,
    proof::prove_shuffle,
    verify::verify_shuffle,
    Permutation, ShuffleConfig, ShuffleProof,
    transcript::ShuffleTranscript,
};

use curve25519_dalek::{
    constants::RISTRETTO_BASEPOINT_POINT as G,
    ristretto::{CompressedRistretto, RistrettoPoint},
    scalar::Scalar,
};

use rand::rngs::OsRng;

// ============================================================================
// mental poker types
// ============================================================================

/// player's keys for mental poker
#[derive(Clone)]
pub struct MentalPokerKeys {
    pub secret_key: Scalar,
    pub public_key: RistrettoPoint,
}

impl MentalPokerKeys {
    /// generate new keys from a seed
    pub fn from_seed(seed: &[u8]) -> Self {
        let hash = blake3::hash(seed);
        let mut bytes = [0u8; 32];
        bytes.copy_from_slice(hash.as_bytes());
        let secret_key = Scalar::from_bytes_mod_order(bytes);
        let public_key = secret_key * G;
        Self { secret_key, public_key }
    }

    /// generate random keys
    pub fn random() -> Self {
        let mut rng = OsRng;
        let secret_key = Scalar::random(&mut rng);
        let public_key = secret_key * G;
        Self { secret_key, public_key }
    }

    /// get compressed public key bytes
    pub fn public_key_bytes(&self) -> [u8; 32] {
        self.public_key.compress().to_bytes()
    }
}

/// elgamal-style masked card
#[derive(Clone, Debug)]
pub struct MaskedCard {
    pub ciphertext: ElGamalCiphertext,
}

impl MaskedCard {
    pub fn new(ct: ElGamalCiphertext) -> Self {
        Self { ciphertext: ct }
    }

    /// serialize for network transmission
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut bytes = Vec::with_capacity(64);
        bytes.extend_from_slice(self.ciphertext.c0.compress().as_bytes());
        bytes.extend_from_slice(self.ciphertext.c1.compress().as_bytes());
        bytes
    }

    /// deserialize from bytes
    pub fn from_bytes(bytes: &[u8]) -> Option<Self> {
        if bytes.len() != 64 {
            return None;
        }
        let c0 = CompressedRistretto::from_slice(&bytes[..32]).ok()?
            .decompress()?;
        let c1 = CompressedRistretto::from_slice(&bytes[32..]).ok()?
            .decompress()?;
        Some(Self {
            ciphertext: ElGamalCiphertext { c0, c1 }
        })
    }
}

/// player's reveal token for a masked card
#[derive(Clone, Copy, Debug)]
pub struct RevealToken {
    /// token = sk * c0
    pub token: RistrettoPoint,
    /// player who provided this token
    pub player_id: u8,
}

impl RevealToken {
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut bytes = Vec::with_capacity(33);
        bytes.push(self.player_id);
        bytes.extend_from_slice(self.token.compress().as_bytes());
        bytes
    }

    pub fn from_bytes(bytes: &[u8]) -> Option<Self> {
        if bytes.len() != 33 {
            return None;
        }
        let player_id = bytes[0];
        let token = CompressedRistretto::from_slice(&bytes[1..]).ok()?
            .decompress()?;
        Some(Self { token, player_id })
    }
}

/// shuffle state machine
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ShuffleState {
    /// waiting for all players to publish keys
    AwaitingKeys,
    /// initial masking phase
    InitialMasking,
    /// waiting for player N to shuffle
    AwaitingShuffle { next_player: u8 },
    /// all shuffles complete, ready to deal
    Ready,
    /// shuffle failed
    Failed(String),
}

impl Default for ShuffleState {
    fn default() -> Self {
        Self::AwaitingKeys
    }
}

/// mental poker hand state
pub struct MentalPokerHand {
    /// hand number (u32 for transcript compatibility)
    pub hand_id: u32,
    /// masked deck (52 cards)
    pub masked_deck: Vec<MaskedCard>,
    /// which cards have been revealed (index -> revealed Card)
    pub revealed_cards: Vec<Option<Card>>,
    /// collected reveal tokens per card per player
    pub reveal_tokens: Vec<Vec<Option<RevealToken>>>,
    /// shuffle proofs from each player
    pub shuffle_proofs: Vec<Option<ShuffleProof>>,
    /// current shuffle state
    pub state: ShuffleState,
    /// transcript for fiat-shamir
    pub transcript: ShuffleTranscript,
}

impl MentalPokerHand {
    pub fn new(hand_id: u32, num_players: u8, game_id: &[u8; 32]) -> Self {
        let mut reveal_tokens = Vec::with_capacity(52);
        for _ in 0..52 {
            reveal_tokens.push(vec![None; num_players as usize]);
        }

        Self {
            hand_id,
            masked_deck: Vec::new(),
            revealed_cards: vec![None; 52],
            reveal_tokens,
            shuffle_proofs: vec![None; num_players as usize],
            state: ShuffleState::AwaitingKeys,
            transcript: ShuffleTranscript::new(game_id, hand_id),
        }
    }
}

// ============================================================================
// shuffle protocol
// ============================================================================

/// mental poker shuffle context
pub struct ShuffleContext {
    /// our keys
    pub keys: MentalPokerKeys,
    /// our player index
    pub player_id: u8,
    /// number of players
    pub num_players: u8,
    /// all player public keys (ordered)
    pub player_keys: Vec<RistrettoPoint>,
    /// aggregate public key
    pub aggregate_key: Option<RistrettoPoint>,
    /// current hand
    pub current_hand: Option<MentalPokerHand>,
    /// shuffle config
    pub config: ShuffleConfig,
    /// game id for transcript binding
    pub game_id: [u8; 32],
}

impl ShuffleContext {
    pub fn new(seed: &[u8], player_id: u8, num_players: u8) -> Self {
        let mut game_id = [0u8; 32];
        let hash = blake3::hash(seed);
        game_id.copy_from_slice(hash.as_bytes());

        Self {
            keys: MentalPokerKeys::from_seed(seed),
            player_id,
            num_players,
            player_keys: vec![RistrettoPoint::default(); num_players as usize],
            aggregate_key: None,
            current_hand: None,
            config: ShuffleConfig::standard_deck(),
            game_id,
        }
    }

    /// register a player's public key
    pub fn register_player_key(&mut self, player_id: u8, pubkey_bytes: [u8; 32]) {
        if let Ok(compressed) = CompressedRistretto::from_slice(&pubkey_bytes) {
            if let Some(pk) = compressed.decompress() {
                if (player_id as usize) < self.player_keys.len() {
                    self.player_keys[player_id as usize] = pk;
                }
            }
        }
    }

    /// compute aggregate public key from all player keys
    pub fn compute_aggregate_key(&mut self) -> [u8; 32] {
        let aggregate: RistrettoPoint = self.player_keys.iter().sum();
        self.aggregate_key = Some(aggregate);
        aggregate.compress().to_bytes()
    }

    /// start a new hand
    pub fn start_hand(&mut self, hand_id: u32) {
        self.current_hand = Some(MentalPokerHand::new(hand_id, self.num_players, &self.game_id));
    }

    /// mask initial deck (done by first player)
    pub fn mask_initial_deck(&mut self) -> Vec<MaskedCard> {
        let agg_key = self.aggregate_key.expect("aggregate key not computed");
        let mut rng = OsRng;

        // create standard deck
        let deck = create_standard_deck();
        let mut masked_deck = Vec::with_capacity(52);

        // encrypt each card with aggregate pk
        for (i, card) in deck.iter().enumerate() {
            let card_value = card.to_index() as u64;
            let msg = Scalar::from(card_value) * G;
            let (ct, _) = ElGamalCiphertext::encrypt(&msg, &agg_key, &mut rng);
            masked_deck.push(MaskedCard::new(ct));
        }

        if let Some(ref mut hand) = self.current_hand {
            hand.masked_deck = masked_deck.clone();
            hand.state = ShuffleState::AwaitingShuffle { next_player: 0 };
            // bind aggregate key to transcript
            hand.transcript.bind_aggregate_key(&agg_key.compress().to_bytes());
        }

        masked_deck
    }

    /// shuffle and remask the deck
    pub fn shuffle_deck(&mut self) -> Result<(Vec<MaskedCard>, ShuffleProof), String> {
        let agg_key = self.aggregate_key.ok_or("no aggregate key")?;
        let hand = self.current_hand.as_mut().ok_or("no active hand")?;
        let mut rng = OsRng;

        // generate random permutation
        let perm = Permutation::random(&mut rng, 52);

        // shuffle and remask
        let mut new_deck = Vec::with_capacity(52);
        let mut randomness = Vec::with_capacity(52);

        let input_deck: Vec<ElGamalCiphertext> = hand.masked_deck.iter()
            .map(|m| m.ciphertext.clone())
            .collect();

        for i in 0..52 {
            let pi_i = perm.get(i);
            let (remasked, r) = input_deck[pi_i].remask(&agg_key, &mut rng);
            new_deck.push(MaskedCard::new(remasked));
            randomness.push(r);
        }

        let output_deck: Vec<ElGamalCiphertext> = new_deck.iter()
            .map(|m| m.ciphertext.clone())
            .collect();

        // generate shuffle proof
        let proof = prove_shuffle(
            &self.config,
            self.player_id,
            &agg_key,
            &input_deck,
            &output_deck,
            &perm,
            &randomness,
            &mut hand.transcript,
            &mut rng,
        ).map_err(|e| format!("shuffle proof failed: {}", e))?;

        // update state
        hand.masked_deck = new_deck.clone();
        hand.shuffle_proofs[self.player_id as usize] = Some(proof.clone());

        // advance to next player or ready
        let next = self.player_id + 1;
        if next >= self.num_players {
            hand.state = ShuffleState::Ready;
        } else {
            hand.state = ShuffleState::AwaitingShuffle { next_player: next };
        }

        Ok((new_deck, proof))
    }

    /// receive and verify another player's shuffle
    pub fn receive_shuffle(
        &mut self,
        player_id: u8,
        new_deck: Vec<MaskedCard>,
        proof: ShuffleProof,
    ) -> Result<(), String> {
        let agg_key = self.aggregate_key.ok_or("no aggregate key")?;
        let hand = self.current_hand.as_mut().ok_or("no active hand")?;

        // verify we're expecting this player
        match &hand.state {
            ShuffleState::AwaitingShuffle { next_player } if *next_player == player_id => {}
            _ => return Err("not expecting shuffle from this player".into()),
        }

        // verify proof
        let input_deck: Vec<ElGamalCiphertext> = hand.masked_deck.iter()
            .map(|m| m.ciphertext.clone())
            .collect();
        let output_deck: Vec<ElGamalCiphertext> = new_deck.iter()
            .map(|m| m.ciphertext.clone())
            .collect();

        verify_shuffle(
            &self.config,
            &agg_key,
            &proof,
            &input_deck,
            &output_deck,
            &mut hand.transcript,
        ).map_err(|e| format!("shuffle verification failed: {}", e))?;

        // update state
        hand.masked_deck = new_deck;
        hand.shuffle_proofs[player_id as usize] = Some(proof);

        let next = player_id + 1;
        if next >= self.num_players {
            hand.state = ShuffleState::Ready;
        } else {
            hand.state = ShuffleState::AwaitingShuffle { next_player: next };
        }

        Ok(())
    }

    /// provide our reveal token for a card
    pub fn provide_reveal_token(&self, card_index: u8) -> Option<RevealToken> {
        let hand = self.current_hand.as_ref()?;
        if card_index as usize >= hand.masked_deck.len() {
            return None;
        }

        // token = sk * c0
        let c0 = &hand.masked_deck[card_index as usize].ciphertext.c0;
        let token = self.keys.secret_key * c0;

        Some(RevealToken {
            token,
            player_id: self.player_id,
        })
    }

    /// receive a reveal token from another player
    pub fn receive_reveal_token(&mut self, card_index: u8, token: RevealToken) -> Result<(), String> {
        let hand = self.current_hand.as_mut().ok_or("no active hand")?;
        if card_index as usize >= hand.reveal_tokens.len() {
            return Err("invalid card index".into());
        }
        if token.player_id as usize >= self.num_players as usize {
            return Err("invalid player id".into());
        }

        hand.reveal_tokens[card_index as usize][token.player_id as usize] = Some(token);
        Ok(())
    }

    /// try to reveal a card (requires all tokens)
    pub fn try_reveal_card(&mut self, card_index: u8) -> Option<Card> {
        let hand = self.current_hand.as_mut()?;
        if card_index as usize >= hand.masked_deck.len() {
            return None;
        }

        // check if already revealed
        if let Some(card) = hand.revealed_cards[card_index as usize] {
            return Some(card);
        }

        // collect all tokens
        let tokens: Vec<&RevealToken> = hand.reveal_tokens[card_index as usize]
            .iter()
            .filter_map(|t| t.as_ref())
            .collect();

        if tokens.len() != self.num_players as usize {
            return None; // not all tokens received
        }

        // compute sum of tokens
        let token_sum: RistrettoPoint = tokens.iter().map(|t| t.token).sum();

        // decrypt: msg = c1 - sum(tokens)
        let c1 = &hand.masked_deck[card_index as usize].ciphertext.c1;
        let msg_point = c1 - token_sum;

        // recover card value by trying each card
        for card_value in 0u64..52 {
            let expected = Scalar::from(card_value) * G;
            if expected == msg_point {
                let card = Card::from_index(card_value as u8)?;
                hand.revealed_cards[card_index as usize] = Some(card);
                return Some(card);
            }
        }

        None // decryption failed
    }
}

// ============================================================================
// utility functions
// ============================================================================

/// create standard 52-card deck
pub fn create_standard_deck() -> Vec<Card> {
    let mut deck = Vec::with_capacity(52);
    for suit in [Suit::Clubs, Suit::Diamonds, Suit::Hearts, Suit::Spades] {
        for rank in [
            Rank::Two, Rank::Three, Rank::Four, Rank::Five, Rank::Six,
            Rank::Seven, Rank::Eight, Rank::Nine, Rank::Ten,
            Rank::Jack, Rank::Queen, Rank::King, Rank::Ace,
        ] {
            deck.push(Card { rank, suit });
        }
    }
    deck
}

// ============================================================================
// message types for p2p
// ============================================================================

/// mental poker protocol messages
#[derive(Clone, Debug)]
pub enum MentalPokerMessage {
    /// publish public key
    PublishKey { pubkey: [u8; 32] },
    /// initial masked deck (from first player)
    InitialDeck { deck: Vec<Vec<u8>> },
    /// shuffle result with proof
    ShuffleResult {
        player_id: u8,
        deck: Vec<Vec<u8>>,
        proof_bytes: Vec<u8>,
    },
    /// reveal token for a card
    RevealToken {
        card_index: u8,
        token: Vec<u8>,
    },
    /// request reveal tokens for cards
    RequestReveal { card_indices: Vec<u8> },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_key_generation() {
        let keys = MentalPokerKeys::from_seed(b"test-seed");
        assert_ne!(keys.secret_key, Scalar::ZERO);

        // deterministic
        let keys2 = MentalPokerKeys::from_seed(b"test-seed");
        assert_eq!(keys.secret_key, keys2.secret_key);
    }

    #[test]
    fn test_shuffle_context_creation() {
        let ctx = ShuffleContext::new(b"test", 0, 2);
        assert_eq!(ctx.player_id, 0);
        assert_eq!(ctx.num_players, 2);
    }

    #[test]
    fn test_standard_deck_creation() {
        let deck = create_standard_deck();
        assert_eq!(deck.len(), 52);

        // check all unique
        let mut seen = std::collections::HashSet::new();
        for card in &deck {
            assert!(seen.insert(card.to_index()));
        }
    }

    #[test]
    fn test_two_player_protocol() {
        // simulate two-player mental poker hand
        let mut player0 = ShuffleContext::new(b"player0-seed", 0, 2);
        let mut player1 = ShuffleContext::new(b"player1-seed", 1, 2);

        // use same game_id for both
        player1.game_id = player0.game_id;

        // exchange keys
        player0.register_player_key(0, player0.keys.public_key_bytes());
        player0.register_player_key(1, player1.keys.public_key_bytes());
        player1.register_player_key(0, player0.keys.public_key_bytes());
        player1.register_player_key(1, player1.keys.public_key_bytes());

        // compute aggregate keys
        player0.compute_aggregate_key();
        player1.compute_aggregate_key();

        // start hands
        player0.start_hand(1);
        player1.start_hand(1);

        // player 0 creates initial masked deck
        let initial_deck = player0.mask_initial_deck();
        assert_eq!(initial_deck.len(), 52);

        // player 1 receives initial deck
        player1.current_hand.as_mut().unwrap().masked_deck = initial_deck;
        player1.current_hand.as_mut().unwrap().state =
            ShuffleState::AwaitingShuffle { next_player: 0 };
        // bind aggregate key for player1 as well
        let agg_bytes = player1.aggregate_key.unwrap().compress().to_bytes();
        player1.current_hand.as_mut().unwrap().transcript.bind_aggregate_key(&agg_bytes);

        // player 0 shuffles
        let (deck0, proof0) = player0.shuffle_deck().expect("shuffle should succeed");

        // player 1 verifies and shuffles
        player1.receive_shuffle(0, deck0, proof0).expect("verify should succeed");
        let (deck1, proof1) = player1.shuffle_deck().expect("shuffle should succeed");

        // player 0 receives final deck
        player0.receive_shuffle(1, deck1, proof1).expect("verify should succeed");

        // verify both are ready
        assert_eq!(
            player0.current_hand.as_ref().unwrap().state,
            ShuffleState::Ready
        );
        assert_eq!(
            player1.current_hand.as_ref().unwrap().state,
            ShuffleState::Ready
        );

        // test reveal protocol for first card
        let token0 = player0.provide_reveal_token(0).unwrap();
        let token1 = player1.provide_reveal_token(0).unwrap();

        // each player receives BOTH tokens
        player0.receive_reveal_token(0, token0.clone()).unwrap();
        player0.receive_reveal_token(0, token1.clone()).unwrap();
        player1.receive_reveal_token(0, token0).unwrap();
        player1.receive_reveal_token(0, token1).unwrap();

        // try to reveal
        let card0 = player0.try_reveal_card(0);
        let card1 = player1.try_reveal_card(0);

        // both should reveal the same card
        assert!(card0.is_some(), "player0 should reveal card");
        assert!(card1.is_some(), "player1 should reveal card");
        assert_eq!(card0, card1, "both players should reveal same card");
    }
}
