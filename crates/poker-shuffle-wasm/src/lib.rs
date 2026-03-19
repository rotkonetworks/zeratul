//! WASM bindings for mental poker zk-shuffle
//!
//! exposes the two-player shuffle ceremony to the browser:
//!   1. generate keypair
//!   2. create initial encrypted deck (player A)
//!   3. shuffle + prove (each player)
//!   4. verify opponent's shuffle
//!   5. reveal cards via decryption share exchange

use curve25519_dalek::{
    constants::RISTRETTO_BASEPOINT_POINT as G,
    ristretto::{CompressedRistretto, RistrettoPoint},
    scalar::Scalar,
};
use rand_core::{OsRng, RngCore};
use zk_shuffle::{
    proof::{compute_deck_commitment, prove_shuffle, ShuffleProof},
    remasking::ElGamalCiphertext,
    transcript::ShuffleTranscript,
    verify::verify_shuffle,
    Permutation, ShuffleConfig,
};

const DECK_SIZE: usize = 52;

fn card_to_point(index: u8) -> RistrettoPoint {
    Scalar::from((index as u64) + 1) * G
}

fn point_to_card(point: &RistrettoPoint) -> Option<u8> {
    for i in 0..DECK_SIZE as u8 {
        if card_to_point(i) == *point {
            return Some(i);
        }
    }
    None
}

fn hex_encode(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{:02x}", b)).collect()
}

fn hex_decode(s: &str) -> Option<Vec<u8>> {
    if s.len() % 2 != 0 { return None; }
    (0..s.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&s[i..i+2], 16).ok())
        .collect()
}

fn decode_point(hex: &str) -> Option<RistrettoPoint> {
    let bytes = hex_decode(hex)?;
    if bytes.len() != 32 { return None; }
    let mut arr = [0u8; 32];
    arr.copy_from_slice(&bytes);
    CompressedRistretto(arr).decompress()
}

fn encode_deck(deck: &[ElGamalCiphertext]) -> String {
    let mut out = String::with_capacity(deck.len() * 128);
    for ct in deck {
        out.push_str(&hex_encode(ct.c0.compress().as_bytes()));
        out.push_str(&hex_encode(ct.c1.compress().as_bytes()));
    }
    out
}

fn decode_deck(hex: &str) -> Option<Vec<ElGamalCiphertext>> {
    let bytes = hex_decode(hex)?;
    if bytes.len() % 64 != 0 { return None; }
    let n = bytes.len() / 64;
    let mut deck = Vec::with_capacity(n);
    for i in 0..n {
        let off = i * 64;
        let mut c0_bytes = [0u8; 32];
        let mut c1_bytes = [0u8; 32];
        c0_bytes.copy_from_slice(&bytes[off..off+32]);
        c1_bytes.copy_from_slice(&bytes[off+32..off+64]);
        let c0 = CompressedRistretto(c0_bytes).decompress()?;
        let c1 = CompressedRistretto(c1_bytes).decompress()?;
        deck.push(ElGamalCiphertext { c0, c1 });
    }
    Some(deck)
}

// ============================================================================
// WASM bindings
// ============================================================================

#[cfg(feature = "wasm")]
mod wasm {
    use super::*;
    use wasm_bindgen::prelude::*;

    /// a player's shuffle keys (ristretto255 ElGamal)
    #[wasm_bindgen]
    pub struct ShuffleKeys {
        sk: Scalar,
        pk: RistrettoPoint,
    }

    #[wasm_bindgen]
    impl ShuffleKeys {
        #[wasm_bindgen(constructor)]
        pub fn new() -> Self {
            let sk = Scalar::random(&mut OsRng);
            let pk = sk * G;
            Self { sk, pk }
        }

        /// public key as 32-byte hex
        pub fn public_key_hex(&self) -> String {
            hex_encode(self.pk.compress().as_bytes())
        }

        /// compute decryption share for a ciphertext at position `idx`
        pub fn decrypt_share(&self, state: &ShuffleState, idx: usize) -> Option<String> {
            if idx >= state.deck.len() { return None; }
            let share = self.sk * state.deck[idx].c0;
            Some(hex_encode(share.compress().as_bytes()))
        }
    }

    /// the shuffle state — holds the encrypted deck and transcript
    #[wasm_bindgen]
    pub struct ShuffleState {
        aggregate_pk: RistrettoPoint,
        deck: Vec<ElGamalCiphertext>,
        transcript: ShuffleTranscript,
        config: ShuffleConfig,
    }

    #[wasm_bindgen]
    impl ShuffleState {
        /// create initial encrypted deck from two public keys (hex).
        /// called by player A (host). binds the initial deck to the transcript
        /// so that the verifier's transcript matches the prover's.
        #[wasm_bindgen(constructor)]
        pub fn new(pk_a_hex: &str, pk_b_hex: &str) -> Result<ShuffleState, JsValue> {
            let pk_a = decode_point(pk_a_hex).ok_or("invalid pk_a")?;
            let pk_b = decode_point(pk_b_hex).ok_or("invalid pk_b")?;
            let aggregate_pk = pk_a + pk_b;
            let config = ShuffleConfig::custom(DECK_SIZE);

            let mut deck = Vec::with_capacity(DECK_SIZE);
            for i in 0..DECK_SIZE as u8 {
                let card_point = card_to_point(i);
                let (ct, _) = ElGamalCiphertext::encrypt(&card_point, &aggregate_pk, &mut OsRng);
                deck.push(ct);
            }

            let mut transcript = ShuffleTranscript::new(b"poker-mental", 2);
            // bind initial deck so transcript state matches on both sides
            let commitment = compute_deck_commitment(&deck);
            transcript.bind_initial_deck(&commitment);

            Ok(Self { aggregate_pk, deck, transcript, config })
        }

        /// create state from the initial (pre-shuffle) deck sent by host.
        /// the guest calls this with the INITIAL deck (before host's shuffle)
        /// so both transcripts are in the same state before verification.
        pub fn from_initial_deck(pk_a_hex: &str, pk_b_hex: &str, initial_deck_hex: &str) -> Result<ShuffleState, JsValue> {
            let pk_a = decode_point(pk_a_hex).ok_or("invalid pk_a")?;
            let pk_b = decode_point(pk_b_hex).ok_or("invalid pk_b")?;
            let aggregate_pk = pk_a + pk_b;
            let config = ShuffleConfig::custom(DECK_SIZE);
            let deck = decode_deck(initial_deck_hex).ok_or("invalid deck")?;

            let mut transcript = ShuffleTranscript::new(b"poker-mental", 2);
            let commitment = compute_deck_commitment(&deck);
            transcript.bind_initial_deck(&commitment);

            Ok(Self { aggregate_pk, deck, transcript, config })
        }

        /// UNVERIFIED: create state from an already-shuffled deck.
        /// WARNING: transcript is NOT bound. verify_and_apply() will produce
        /// WRONG results on this state. Only use for card reveal after both
        /// shuffles are complete and verified via from_initial_deck path.
        pub fn from_deck_unverified(pk_a_hex: &str, pk_b_hex: &str, deck_hex: &str) -> Result<ShuffleState, JsValue> {
            let pk_a = decode_point(pk_a_hex).ok_or("invalid pk_a")?;
            let pk_b = decode_point(pk_b_hex).ok_or("invalid pk_b")?;
            let aggregate_pk = pk_a + pk_b;
            let config = ShuffleConfig::custom(DECK_SIZE);
            let deck = decode_deck(deck_hex).ok_or("invalid deck")?;
            let transcript = ShuffleTranscript::new(b"poker-mental", 2);
            Ok(Self { aggregate_pk, deck, transcript, config })
        }

        /// shuffle + remask + produce proof. returns JSON: { deck: hex, proof: hex }
        pub fn shuffle_and_prove(&mut self, player_id: u8) -> Result<String, JsValue> {
            let n = self.deck.len();
            let mut rng = OsRng;

            // random permutation
            let mut mapping: Vec<usize> = (0..n).collect();
            for i in (1..n).rev() {
                let j = (rng.next_u32() as usize) % (i + 1);
                mapping.swap(i, j);
            }
            let permutation = Permutation::new(mapping).map_err(|e| format!("{:?}", e))?;

            // apply permutation + remask
            let mut output_deck = Vec::with_capacity(n);
            let mut randomness = Vec::with_capacity(n);
            for i in 0..n {
                let pi_i = permutation.get(i);
                let (remasked, r) = self.deck[pi_i].remask(&self.aggregate_pk, &mut rng);
                output_deck.push(remasked);
                randomness.push(r);
            }

            // generate proof
            let proof = prove_shuffle(
                &self.config,
                player_id,
                &self.aggregate_pk,
                &self.deck,
                &output_deck,
                &permutation,
                &randomness,
                &mut self.transcript,
                &mut rng,
            ).map_err(|e| format!("{:?}", e))?;

            let deck_hex = encode_deck(&output_deck);
            let proof_hex = hex_encode(&proof.to_bytes());
            self.deck = output_deck;

            Ok(format!(r#"{{"deck":"{}","proof":"{}"}}"#, deck_hex, proof_hex))
        }

        /// verify opponent's shuffle and update deck. returns true if valid.
        pub fn verify_and_apply(&mut self, deck_hex: &str, proof_hex: &str) -> Result<bool, JsValue> {
            let new_deck = decode_deck(deck_hex).ok_or("invalid deck")?;
            let proof_bytes = hex_decode(proof_hex).ok_or("invalid proof hex")?;
            let proof = ShuffleProof::from_bytes(&proof_bytes).ok_or("invalid proof")?;

            let valid = verify_shuffle(
                &self.config,
                &self.aggregate_pk,
                &proof,
                &self.deck,
                &new_deck,
                &mut self.transcript,
            ).map_err(|e| format!("{:?}", e))?;

            if valid {
                self.deck = new_deck;
            }
            Ok(valid)
        }

        /// get the current deck as hex (for sending to opponent)
        pub fn deck_hex(&self) -> String {
            encode_deck(&self.deck)
        }

        /// reveal a card given two decryption shares (hex).
        /// returns card index (0..51) or -1 if failed.
        pub fn reveal_card(&self, idx: usize, share_a_hex: &str, share_b_hex: &str) -> i32 {
            let share_a = match decode_point(share_a_hex) { Some(p) => p, None => return -1 };
            let share_b = match decode_point(share_b_hex) { Some(p) => p, None => return -1 };
            if idx >= self.deck.len() { return -1; }
            let ct = &self.deck[idx];
            let card_point = ct.c1 - share_a - share_b;
            match point_to_card(&card_point) {
                Some(i) => i as i32,
                None => -1,
            }
        }

        pub fn deck_size(&self) -> usize {
            self.deck.len()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_card_roundtrip() {
        for i in 0..52u8 {
            let point = card_to_point(i);
            assert_eq!(point_to_card(&point), Some(i));
        }
    }
}
