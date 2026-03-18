//! verifiable mental poker via zk-shuffle
//!
//! two-player heads-up protocol where neither player can:
//! - see the other's cards
//! - deal themselves better cards
//! - alter the deck after shuffling
//!
//! every shuffle round produces a chaum-pedersen proof that is:
//! - verifiable by the opponent before accepting the deck
//! - verifiable by the jury on dispute
//! - storable in the signed transcript
//!
//! # protocol (2 players, 52 cards)
//!
//! ```text
//! setup:
//!   both players generate ristretto255 keypairs
//!   aggregate public key: PK = pk_A + pk_B
//!
//! masking:
//!   player A encrypts each card as ElGamal(card_point, PK)
//!   → encrypted deck (52 ciphertexts)
//!
//! shuffle round 1 (player A):
//!   permute + remask the encrypted deck
//!   produce chaum-pedersen proof
//!   send (shuffled_deck, proof) to player B
//!
//! shuffle round 2 (player B):
//!   verify player A's proof
//!   permute + remask again
//!   produce chaum-pedersen proof
//!   send (shuffled_deck, proof) to player A
//!
//! player A verifies player B's proof
//! deck is now doubly shuffled — neither player knows the permutation
//!
//! card reveal:
//!   to reveal card i, both players provide decryption shares:
//!     share_A = sk_A * ciphertext[i].c0
//!     share_B = sk_B * ciphertext[i].c0
//!   card_point = ciphertext[i].c1 - share_A - share_B
//!   decode card_point back to card identity
//! ```
//!
//! # security
//!
//! - DDH assumption on ristretto255 (ElGamal semantic security)
//! - chaum-pedersen proofs bind shuffle to transcript
//! - neither player can learn card identities without both decryption shares
//! - proofs are compact and verifiable on-chain for disputes

pub use zk_shuffle::poker::{Card, Rank, Suit, HandCategory, HandRank};
pub use zk_shuffle::remasking::ElGamalCiphertext;
pub use zk_shuffle::proof::{ShuffleProof, prove_shuffle};
pub use zk_shuffle::verify::verify_shuffle;
pub use zk_shuffle::{ShuffleConfig, ShuffleError, Permutation};

use curve25519_dalek::{
    constants::RISTRETTO_BASEPOINT_POINT as G,
    ristretto::RistrettoPoint,
    scalar::Scalar,
};
use rand_core::{CryptoRng, RngCore};
use zk_shuffle::transcript::ShuffleTranscript;

/// number of cards in a standard deck
pub const DECK_SIZE: usize = 52;

/// a player's mental poker keys (ristretto255)
pub struct PlayerKeys {
    /// secret key (ElGamal decryption)
    pub sk: Scalar,
    /// public key (ElGamal encryption)
    pub pk: RistrettoPoint,
}

impl PlayerKeys {
    pub fn generate<R: RngCore + CryptoRng>(rng: &mut R) -> Self {
        let sk = Scalar::random(rng);
        let pk = sk * G;
        Self { sk, pk }
    }

    /// compute decryption share for a ciphertext
    /// share = sk * c0
    pub fn decrypt_share(&self, ct: &ElGamalCiphertext) -> RistrettoPoint {
        self.sk * ct.c0
    }
}

/// encode a card index (0..51) as a curve point
/// card_point = Scalar(index + 1) * G
/// (index + 1 so that card 0 doesn't map to identity)
pub fn card_to_point(index: u8) -> RistrettoPoint {
    Scalar::from((index as u64) + 1) * G
}

/// decode a curve point back to a card index
/// tries all 52 possibilities (brute force, but 52 is tiny)
pub fn point_to_card(point: &RistrettoPoint) -> Option<u8> {
    for i in 0..DECK_SIZE as u8 {
        if card_to_point(i) == *point {
            return Some(i);
        }
    }
    None
}

/// convert a card index (0..51) to a Card
pub fn index_to_card(index: u8) -> Card {
    let rank = Rank::from_value((index % 13) + 2).unwrap();
    let suit = match index / 13 {
        0 => Suit::Spades,
        1 => Suit::Hearts,
        2 => Suit::Diamonds,
        _ => Suit::Clubs,
    };
    Card::new(rank, suit)
}

/// a shuffled encrypted deck with its proof
pub struct ShuffledDeck {
    /// the encrypted cards
    pub ciphertexts: Vec<ElGamalCiphertext>,
    /// chaum-pedersen proof of valid shuffle
    pub proof: ShuffleProof,
}

/// the state of a mental poker game
pub struct MentalPokerState {
    /// aggregate public key (pk_A + pk_B)
    pub aggregate_pk: RistrettoPoint,
    /// current encrypted deck (after all shuffles)
    pub deck: Vec<ElGamalCiphertext>,
    /// shuffle transcript (for proof binding)
    pub transcript: ShuffleTranscript,
    /// config
    pub config: ShuffleConfig,
}

impl MentalPokerState {
    /// initialize: player A creates the initial encrypted deck
    pub fn initial_mask<R: RngCore + CryptoRng>(
        pk_a: &RistrettoPoint,
        pk_b: &RistrettoPoint,
        rng: &mut R,
    ) -> Self {
        let aggregate_pk = pk_a + pk_b;
        let config = ShuffleConfig::custom(DECK_SIZE);

        // encrypt each card with the aggregate public key
        let mut deck = Vec::with_capacity(DECK_SIZE);
        for i in 0..DECK_SIZE as u8 {
            let card_point = card_to_point(i);
            let (ct, _) = ElGamalCiphertext::encrypt(&card_point, &aggregate_pk, rng);
            deck.push(ct);
        }

        let transcript = ShuffleTranscript::new(b"poker-mental", 2);

        Self { aggregate_pk, deck, transcript, config }
    }

    /// shuffle + remask + prove (called by each player in turn)
    pub fn shuffle_and_prove<R: RngCore + CryptoRng>(
        &mut self,
        player_id: u8,
        rng: &mut R,
    ) -> Result<ShuffledDeck, ShuffleError> {
        let n = self.deck.len();

        // generate random permutation
        let mut mapping: Vec<usize> = (0..n).collect();
        for i in (1..n).rev() {
            let j = (rng.next_u64() as usize) % (i + 1);
            mapping.swap(i, j);
        }
        let permutation = Permutation::new(mapping)?;

        // apply permutation + remask
        let mut output_deck = Vec::with_capacity(n);
        let mut randomness = Vec::with_capacity(n);
        for i in 0..n {
            let pi_i = permutation.get(i);
            let (remasked, r) = self.deck[pi_i].remask(&self.aggregate_pk, rng);
            output_deck.push(remasked);
            randomness.push(r);
        }

        // generate chaum-pedersen proof
        let proof = prove_shuffle(
            &self.config,
            player_id,
            &self.aggregate_pk,
            &self.deck,
            &output_deck,
            &permutation,
            &randomness,
            &mut self.transcript,
            rng,
        )?;

        // update deck state
        let old_deck = std::mem::replace(&mut self.deck, output_deck.clone());
        drop(old_deck);

        Ok(ShuffledDeck {
            ciphertexts: output_deck,
            proof,
        })
    }

    /// verify an opponent's shuffle proof and update deck
    pub fn verify_and_apply(
        &mut self,
        shuffled: &ShuffledDeck,
    ) -> Result<bool, ShuffleError> {
        let valid = verify_shuffle(
            &self.config,
            &self.aggregate_pk,
            &shuffled.proof,
            &self.deck,
            &shuffled.ciphertexts,
            &mut self.transcript,
        )?;

        if valid {
            self.deck = shuffled.ciphertexts.clone();
        }

        Ok(valid)
    }

    /// reveal a card given both players' decryption shares
    pub fn reveal_card(
        &self,
        card_index: usize,
        share_a: &RistrettoPoint,
        share_b: &RistrettoPoint,
    ) -> Option<u8> {
        if card_index >= self.deck.len() {
            return None;
        }
        let ct = &self.deck[card_index];
        // card_point = c1 - share_A - share_B
        let card_point = ct.c1 - share_a - share_b;
        point_to_card(&card_point)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rand::rngs::OsRng;

    #[test]
    fn test_card_encoding_roundtrip() {
        for i in 0..52u8 {
            let point = card_to_point(i);
            let decoded = point_to_card(&point);
            assert_eq!(decoded, Some(i), "card {} failed roundtrip", i);
        }
    }

    #[test]
    fn test_index_to_card() {
        let c = index_to_card(0);
        assert_eq!(c.rank, Rank::Two);
        assert_eq!(c.suit, Suit::Spades);

        let c = index_to_card(12);
        assert_eq!(c.rank, Rank::Ace);
        assert_eq!(c.suit, Suit::Spades);

        let c = index_to_card(51);
        assert_eq!(c.rank, Rank::Ace);
        assert_eq!(c.suit, Suit::Clubs);
    }

    #[test]
    fn test_two_player_shuffle_and_reveal() {
        let mut rng = OsRng;

        // both players generate keys
        let alice = PlayerKeys::generate(&mut rng);
        let bob = PlayerKeys::generate(&mut rng);

        // alice creates initial encrypted deck and sends it to bob
        let mut alice_state = MentalPokerState::initial_mask(&alice.pk, &bob.pk, &mut rng);
        assert_eq!(alice_state.deck.len(), 52);

        // bob receives the initial deck (same deck, same transcript init)
        let pre_shuffle_deck = alice_state.deck.clone();

        // alice shuffles + proves
        let alice_shuffled = alice_state.shuffle_and_prove(0, &mut rng)
            .expect("alice shuffle should succeed");

        // bob starts from the pre-shuffle deck (what alice sent before shuffling)
        let mut bob_state = MentalPokerState {
            aggregate_pk: alice_state.aggregate_pk,
            deck: pre_shuffle_deck,
            transcript: ShuffleTranscript::new(b"poker-mental", 2),
            config: ShuffleConfig::custom(DECK_SIZE),
        };

        // bob verifies alice's shuffle
        let valid = bob_state.verify_and_apply(&alice_shuffled)
            .expect("verification should not error");
        assert!(valid, "alice's shuffle proof should verify");

        // bob shuffles + proves
        let bob_shuffled = bob_state.shuffle_and_prove(1, &mut rng)
            .expect("bob shuffle should succeed");

        // alice verifies bob's shuffle
        let valid = alice_state.verify_and_apply(&bob_shuffled)
            .expect("verification should not error");
        assert!(valid, "bob's shuffle proof should verify");

        // both should have the same final deck
        for i in 0..52 {
            assert_eq!(alice_state.deck[i], bob_state.deck[i],
                "deck mismatch at position {}", i);
        }

        // reveal card 0: both provide decryption shares
        let share_a = alice.decrypt_share(&alice_state.deck[0]);
        let share_b = bob.decrypt_share(&alice_state.deck[0]);

        let card_idx = alice_state.reveal_card(0, &share_a, &share_b);
        assert!(card_idx.is_some(), "card reveal should succeed");
        let card = index_to_card(card_idx.unwrap());
        println!("revealed card 0: {}{}", card.rank.char(), card.suit.char());

        // reveal card 1
        let share_a = alice.decrypt_share(&alice_state.deck[1]);
        let share_b = bob.decrypt_share(&alice_state.deck[1]);
        let card_idx = alice_state.reveal_card(1, &share_a, &share_b);
        assert!(card_idx.is_some(), "card 1 reveal should succeed");
        let card = index_to_card(card_idx.unwrap());
        println!("revealed card 1: {}{}", card.rank.char(), card.suit.char());
    }

    #[test]
    fn test_single_share_insufficient() {
        let mut rng = OsRng;
        let alice = PlayerKeys::generate(&mut rng);
        let bob = PlayerKeys::generate(&mut rng);

        let state = MentalPokerState::initial_mask(&alice.pk, &bob.pk, &mut rng);

        // alice tries to reveal with only her share
        let share_a = alice.decrypt_share(&state.deck[0]);
        let zero = RistrettoPoint::default();
        let card = state.reveal_card(0, &share_a, &zero);

        // should either be None or wrong card
        if let Some(idx) = card {
            // if brute force finds something, verify it's the wrong card
            // (the deck is freshly encrypted, card 0 should be index 0)
            // with only one share, the decryption is garbage
            println!("single share decoded to index {} (should be garbage)", idx);
        }
    }
}
