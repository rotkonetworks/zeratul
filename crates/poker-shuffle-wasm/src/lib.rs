//! WASM bindings for mental poker zk-shuffle
//!
//! exposes the two-player shuffle ceremony to the browser:
//!   1. generate keypair + schnorr proof of possession
//!   2. both players construct the canonical initial deck locally
//!   3. shuffle + prove (each player)
//!   4. verify opponent's shuffle
//!   5. reveal cards via dleq-verified decryption share exchange
//!
//! the initial deck is trivially encrypted (c0 = identity, c1 = card point)
//! so both sides can verify it contains exactly one of each card; hiding
//! comes from the two subsequent shuffle+remask rounds.

use curve25519_dalek::{
    constants::RISTRETTO_BASEPOINT_POINT as G,
    ristretto::{CompressedRistretto, RistrettoPoint},
    scalar::Scalar,
    traits::Identity,
};
use rand_core::{OsRng, RngCore};
use zeroize::Zeroize;
use zk_shuffle::{
    proof::{compute_deck_commitment, prove_shuffle, ShuffleProof},
    remasking::ElGamalCiphertext,
    reveal::{PossessionProof, RevealProof},
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

/// canonical initial deck: c0 = identity, c1 = card point.
/// deterministic, so both players construct and verify it locally
fn canonical_deck() -> Vec<ElGamalCiphertext> {
    (0..DECK_SIZE as u8)
        .map(|i| ElGamalCiphertext {
            c0: RistrettoPoint::identity(),
            c1: card_to_point(i),
        })
        .collect()
}

/// unbiased sample in [0, n) via rejection sampling
fn sample_below<R: RngCore>(rng: &mut R, n: u64) -> u64 {
    debug_assert!(n > 0);
    // largest multiple of n that fits in u64; reject values above it
    let limit = u64::MAX - u64::MAX % n;
    loop {
        let v = rng.next_u64();
        if v < limit {
            return v % n;
        }
    }
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
// core ceremony logic (shared by wasm bindings and native tests)
// ============================================================================

/// a player's keypair, sk zeroized on drop
pub struct Keys {
    sk: Scalar,
    pk: RistrettoPoint,
}

impl Drop for Keys {
    fn drop(&mut self) {
        self.sk.zeroize();
    }
}

impl Keys {
    pub fn generate() -> Self {
        let sk = Scalar::random(&mut OsRng);
        let pk = sk * G;
        Self { sk, pk }
    }

    pub fn pk(&self) -> RistrettoPoint {
        self.pk
    }

    /// schnorr proof of possession of sk
    pub fn prove_possession(&self) -> PossessionProof {
        PossessionProof::prove(&self.sk, &mut OsRng)
    }

    /// decryption share for a ciphertext + dleq proof of correctness
    pub fn decrypt_share(&self, ct: &ElGamalCiphertext) -> (RistrettoPoint, RevealProof) {
        RevealProof::prove(&self.sk, &ct.c0, &mut OsRng)
    }
}

/// verify both players' dleq proofs and recover the card
fn reveal_verified(
    pk_a: &RistrettoPoint,
    pk_b: &RistrettoPoint,
    ct: &ElGamalCiphertext,
    share_a: &RistrettoPoint,
    proof_a: &RevealProof,
    share_b: &RistrettoPoint,
    proof_b: &RevealProof,
) -> Option<u8> {
    if !proof_a.verify(pk_a, &ct.c0, share_a) {
        return None;
    }
    if !proof_b.verify(pk_b, &ct.c0, share_b) {
        return None;
    }
    point_to_card(&(ct.c1 - share_a - share_b))
}

/// ceremony state: possession-verified keys, deck, fiat-shamir transcript
pub struct State {
    pk_a: RistrettoPoint,
    pk_b: RistrettoPoint,
    aggregate_pk: RistrettoPoint,
    deck: Vec<ElGamalCiphertext>,
    transcript: ShuffleTranscript,
    config: ShuffleConfig,
}

impl State {
    /// construct from both players' keys and proofs of possession.
    /// rejects rogue keys: each pop must verify against its pk.
    /// deck starts as the canonical initial deck
    pub fn new(
        pk_a: RistrettoPoint,
        pk_b: RistrettoPoint,
        pop_a: &PossessionProof,
        pop_b: &PossessionProof,
    ) -> Result<Self, &'static str> {
        if !pop_a.verify(&pk_a) {
            return Err("invalid proof of possession for pk_a");
        }
        if !pop_b.verify(&pk_b) {
            return Err("invalid proof of possession for pk_b");
        }
        let aggregate_pk = pk_a + pk_b;
        let deck = canonical_deck();

        let mut transcript = ShuffleTranscript::new(b"poker-mental", 2);
        transcript.bind_player_key(0, pk_a.compress().as_bytes(), &pop_a.to_bytes());
        transcript.bind_player_key(1, pk_b.compress().as_bytes(), &pop_b.to_bytes());
        transcript.bind_aggregate_key(aggregate_pk.compress().as_bytes());
        let commitment = compute_deck_commitment(&deck);
        transcript.bind_initial_deck(&commitment);

        Ok(Self {
            pk_a,
            pk_b,
            aggregate_pk,
            deck,
            transcript,
            config: ShuffleConfig::custom(DECK_SIZE),
        })
    }

    /// like new(), but checks a received initial deck equals the canonical
    /// deck (rejects duplicates/omissions/re-encryptions)
    pub fn with_initial_deck(
        pk_a: RistrettoPoint,
        pk_b: RistrettoPoint,
        pop_a: &PossessionProof,
        pop_b: &PossessionProof,
        deck: &[ElGamalCiphertext],
    ) -> Result<Self, &'static str> {
        if deck != canonical_deck().as_slice() {
            return Err("initial deck is not the canonical deck");
        }
        Self::new(pk_a, pk_b, pop_a, pop_b)
    }

    pub fn deck(&self) -> &[ElGamalCiphertext] {
        &self.deck
    }

    /// shuffle + remask + produce proof. updates own deck
    pub fn shuffle_and_prove(&mut self, player_id: u8) -> Result<ShuffleProof, String> {
        let n = self.deck.len();
        let mut rng = OsRng;

        // unbiased fisher-yates
        let mut mapping: Vec<usize> = (0..n).collect();
        for i in (1..n).rev() {
            let j = sample_below(&mut rng, (i + 1) as u64) as usize;
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

        self.deck = output_deck;
        Ok(proof)
    }

    /// verify opponent's shuffle and update deck. returns true if valid
    pub fn verify_and_apply(
        &mut self,
        new_deck: Vec<ElGamalCiphertext>,
        proof: &ShuffleProof,
    ) -> Result<bool, String> {
        let valid = verify_shuffle(
            &self.config,
            &self.aggregate_pk,
            proof,
            &self.deck,
            &new_deck,
            &mut self.transcript,
        ).map_err(|e| format!("{:?}", e))?;

        if valid {
            self.deck = new_deck;
        }
        Ok(valid)
    }

    /// reveal a card from both players' dleq-proven shares
    pub fn reveal_card(
        &self,
        idx: usize,
        share_a: &RistrettoPoint,
        proof_a: &RevealProof,
        share_b: &RistrettoPoint,
        proof_b: &RevealProof,
    ) -> Option<u8> {
        let ct = self.deck.get(idx)?;
        reveal_verified(&self.pk_a, &self.pk_b, ct, share_a, proof_a, share_b, proof_b)
    }
}

/// reveal-only state reconstructed after the ceremony: can produce and
/// verify decryption shares but cannot shuffle or verify shuffle proofs
pub struct RevealOnly {
    pk_a: RistrettoPoint,
    pk_b: RistrettoPoint,
    deck: Vec<ElGamalCiphertext>,
}

impl RevealOnly {
    pub fn new(pk_a: RistrettoPoint, pk_b: RistrettoPoint, deck: Vec<ElGamalCiphertext>) -> Self {
        Self { pk_a, pk_b, deck }
    }

    pub fn deck(&self) -> &[ElGamalCiphertext] {
        &self.deck
    }

    /// reveal a card from both players' dleq-proven shares
    pub fn reveal_card(
        &self,
        idx: usize,
        share_a: &RistrettoPoint,
        proof_a: &RevealProof,
        share_b: &RistrettoPoint,
        proof_b: &RevealProof,
    ) -> Option<u8> {
        let ct = self.deck.get(idx)?;
        reveal_verified(&self.pk_a, &self.pk_b, ct, share_a, proof_a, share_b, proof_b)
    }
}

// ============================================================================
// WASM bindings
// ============================================================================

#[cfg(feature = "wasm")]
mod wasm {
    use super::*;
    use wasm_bindgen::prelude::*;

    fn decode_pop(hex: &str) -> Option<PossessionProof> {
        PossessionProof::from_bytes(&hex_decode(hex)?)
    }

    fn decode_reveal_proof(hex: &str) -> Option<RevealProof> {
        RevealProof::from_bytes(&hex_decode(hex)?)
    }

    /// decryption share + proof as JSON {"share": hex, "proof": hex}
    fn share_json(keys: &Keys, deck: &[ElGamalCiphertext], idx: usize) -> Option<String> {
        let ct = deck.get(idx)?;
        let (share, proof) = keys.decrypt_share(ct);
        Some(serde_json::json!({
            "share": hex_encode(share.compress().as_bytes()),
            "proof": hex_encode(&proof.to_bytes()),
        }).to_string())
    }

    /// decode shares + proofs and reveal. -1 on any failure
    fn reveal_from_hex(
        reveal: impl FnOnce(&RistrettoPoint, &RevealProof, &RistrettoPoint, &RevealProof) -> Option<u8>,
        share_a_hex: &str,
        proof_a_hex: &str,
        share_b_hex: &str,
        proof_b_hex: &str,
    ) -> i32 {
        let share_a = match decode_point(share_a_hex) { Some(p) => p, None => return -1 };
        let share_b = match decode_point(share_b_hex) { Some(p) => p, None => return -1 };
        let proof_a = match decode_reveal_proof(proof_a_hex) { Some(p) => p, None => return -1 };
        let proof_b = match decode_reveal_proof(proof_b_hex) { Some(p) => p, None => return -1 };
        match reveal(&share_a, &proof_a, &share_b, &proof_b) {
            Some(i) => i as i32,
            None => -1,
        }
    }

    /// a player's shuffle keys (ristretto255 ElGamal)
    #[wasm_bindgen]
    pub struct ShuffleKeys {
        inner: Keys,
    }

    #[wasm_bindgen]
    impl ShuffleKeys {
        #[wasm_bindgen(constructor)]
        pub fn new() -> Self {
            Self { inner: Keys::generate() }
        }

        /// public key as 32-byte hex
        pub fn public_key_hex(&self) -> String {
            hex_encode(self.inner.pk().compress().as_bytes())
        }

        /// schnorr proof of possession of the secret key (64-byte hex).
        /// required by the ShuffleState constructors
        pub fn prove_possession(&self) -> String {
            hex_encode(&self.inner.prove_possession().to_bytes())
        }

        /// decryption share + dleq proof for the ciphertext at `idx`.
        /// returns JSON {"share": hex, "proof": hex}
        pub fn decrypt_share(&self, state: &ShuffleState, idx: usize) -> Option<String> {
            share_json(&self.inner, state.inner.deck(), idx)
        }

        /// same as decrypt_share, for a reveal-only state
        pub fn decrypt_share_reveal(&self, state: &RevealState, idx: usize) -> Option<String> {
            share_json(&self.inner, state.inner.deck(), idx)
        }
    }

    /// the shuffle ceremony state — holds the encrypted deck and transcript
    #[wasm_bindgen]
    pub struct ShuffleState {
        inner: State,
    }

    #[wasm_bindgen]
    impl ShuffleState {
        /// create ceremony state from both players' public keys and proofs
        /// of possession (hex). the deck starts as the canonical initial
        /// deck, constructed locally — identical on both sides
        #[wasm_bindgen(constructor)]
        pub fn new(
            pk_a_hex: &str,
            pk_b_hex: &str,
            pop_a_hex: &str,
            pop_b_hex: &str,
        ) -> Result<ShuffleState, JsValue> {
            let pk_a = decode_point(pk_a_hex).ok_or("invalid pk_a")?;
            let pk_b = decode_point(pk_b_hex).ok_or("invalid pk_b")?;
            let pop_a = decode_pop(pop_a_hex).ok_or("invalid pop_a")?;
            let pop_b = decode_pop(pop_b_hex).ok_or("invalid pop_b")?;
            let inner = State::new(pk_a, pk_b, &pop_a, &pop_b)?;
            Ok(Self { inner })
        }

        /// like the constructor, but verifies a received initial deck (hex)
        /// equals the canonical deck. rejects duplicated/omitted cards
        pub fn from_initial_deck(
            pk_a_hex: &str,
            pk_b_hex: &str,
            pop_a_hex: &str,
            pop_b_hex: &str,
            initial_deck_hex: &str,
        ) -> Result<ShuffleState, JsValue> {
            let pk_a = decode_point(pk_a_hex).ok_or("invalid pk_a")?;
            let pk_b = decode_point(pk_b_hex).ok_or("invalid pk_b")?;
            let pop_a = decode_pop(pop_a_hex).ok_or("invalid pop_a")?;
            let pop_b = decode_pop(pop_b_hex).ok_or("invalid pop_b")?;
            let deck = decode_deck(initial_deck_hex).ok_or("invalid deck")?;
            let inner = State::with_initial_deck(pk_a, pk_b, &pop_a, &pop_b, &deck)?;
            Ok(Self { inner })
        }

        /// shuffle + remask + produce proof. returns JSON: { deck: hex, proof: hex }
        pub fn shuffle_and_prove(&mut self, player_id: u8) -> Result<String, JsValue> {
            let proof = self.inner.shuffle_and_prove(player_id)?;
            Ok(serde_json::json!({
                "deck": encode_deck(self.inner.deck()),
                "proof": hex_encode(&proof.to_bytes()),
            }).to_string())
        }

        /// verify opponent's shuffle and update deck. returns true if valid.
        pub fn verify_and_apply(&mut self, deck_hex: &str, proof_hex: &str) -> Result<bool, JsValue> {
            let new_deck = decode_deck(deck_hex).ok_or("invalid deck")?;
            let proof_bytes = hex_decode(proof_hex).ok_or("invalid proof hex")?;
            let proof = ShuffleProof::from_bytes(&proof_bytes).ok_or("invalid proof")?;
            Ok(self.inner.verify_and_apply(new_deck, &proof)?)
        }

        /// get the current deck as hex (for sending to opponent)
        pub fn deck_hex(&self) -> String {
            encode_deck(self.inner.deck())
        }

        /// reveal a card from both players' decryption shares + dleq proofs
        /// (hex, as produced by decrypt_share). returns card index (0..51)
        /// or -1 if any share or proof is invalid
        pub fn reveal_card(
            &self,
            idx: usize,
            share_a_hex: &str,
            proof_a_hex: &str,
            share_b_hex: &str,
            proof_b_hex: &str,
        ) -> i32 {
            reveal_from_hex(
                |sa, pa, sb, pb| self.inner.reveal_card(idx, sa, pa, sb, pb),
                share_a_hex, proof_a_hex, share_b_hex, proof_b_hex,
            )
        }

        pub fn deck_size(&self) -> usize {
            self.inner.deck().len()
        }
    }

    /// reveal-only state for after the ceremony: reconstructs the final
    /// deck to produce/verify decryption shares. has no shuffle or proof
    /// methods, so it cannot be misused to skip shuffle verification
    #[wasm_bindgen]
    pub struct RevealState {
        inner: RevealOnly,
    }

    #[wasm_bindgen]
    impl RevealState {
        #[wasm_bindgen(constructor)]
        pub fn new(pk_a_hex: &str, pk_b_hex: &str, deck_hex: &str) -> Result<RevealState, JsValue> {
            let pk_a = decode_point(pk_a_hex).ok_or("invalid pk_a")?;
            let pk_b = decode_point(pk_b_hex).ok_or("invalid pk_b")?;
            let deck = decode_deck(deck_hex).ok_or("invalid deck")?;
            Ok(Self { inner: RevealOnly::new(pk_a, pk_b, deck) })
        }

        /// reveal a card from both players' decryption shares + dleq proofs.
        /// returns card index (0..51) or -1 if any share or proof is invalid
        pub fn reveal_card(
            &self,
            idx: usize,
            share_a_hex: &str,
            proof_a_hex: &str,
            share_b_hex: &str,
            proof_b_hex: &str,
        ) -> i32 {
            reveal_from_hex(
                |sa, pa, sb, pb| self.inner.reveal_card(idx, sa, pa, sb, pb),
                share_a_hex, proof_a_hex, share_b_hex, proof_b_hex,
            )
        }

        pub fn deck_size(&self) -> usize {
            self.inner.deck().len()
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

    #[test]
    fn test_sample_below_in_range() {
        let mut rng = OsRng;
        for n in 1..=8u64 {
            for _ in 0..200 {
                assert!(sample_below(&mut rng, n) < n);
            }
        }
    }

    #[test]
    fn test_canonical_deck_serialization_roundtrip() {
        let deck = canonical_deck();
        let hex = encode_deck(&deck);
        let decoded = decode_deck(&hex).unwrap();
        assert_eq!(deck, decoded);
    }

    fn setup_players() -> (Keys, Keys, State, State) {
        let keys_a = Keys::generate();
        let keys_b = Keys::generate();
        let pop_a = keys_a.prove_possession();
        let pop_b = keys_b.prove_possession();

        let host = State::new(keys_a.pk(), keys_b.pk(), &pop_a, &pop_b).unwrap();
        // guest verifies the host's initial deck against the canonical one
        let guest = State::with_initial_deck(
            keys_a.pk(), keys_b.pk(), &pop_a, &pop_b, host.deck(),
        ).unwrap();

        (keys_a, keys_b, host, guest)
    }

    #[test]
    fn test_full_ceremony_with_verification() {
        let (keys_a, keys_b, mut host, mut guest) = setup_players();

        // host shuffles from the canonical deck (identity c0)
        let host_proof = host.shuffle_and_prove(0).unwrap();

        // proof serialization roundtrip, then guest verifies
        let host_proof = ShuffleProof::from_bytes(&host_proof.to_bytes()).unwrap();
        assert!(guest.verify_and_apply(host.deck().to_vec(), &host_proof).unwrap());

        // guest shuffles on top, host verifies
        let guest_proof = guest.shuffle_and_prove(1).unwrap();
        assert!(host.verify_and_apply(guest.deck().to_vec(), &guest_proof).unwrap());
        assert_eq!(host.deck(), guest.deck());

        // reveal all cards with dleq-verified shares; must be a full deck
        let mut seen = [false; DECK_SIZE];
        for idx in 0..DECK_SIZE {
            let (share_a, proof_a) = keys_a.decrypt_share(&host.deck()[idx]);
            let (share_b, proof_b) = keys_b.decrypt_share(&host.deck()[idx]);
            let card = host
                .reveal_card(idx, &share_a, &proof_a, &share_b, &proof_b)
                .expect("reveal failed");
            assert!(!seen[card as usize], "duplicate card {}", card);
            seen[card as usize] = true;
        }
        assert!(seen.iter().all(|&s| s));
    }

    #[test]
    fn test_forged_decryption_share_rejected() {
        let (keys_a, keys_b, mut host, mut guest) = setup_players();
        let p = host.shuffle_and_prove(0).unwrap();
        assert!(guest.verify_and_apply(host.deck().to_vec(), &p).unwrap());
        let p = guest.shuffle_and_prove(1).unwrap();
        assert!(host.verify_and_apply(guest.deck().to_vec(), &p).unwrap());

        let ct = &host.deck()[0];
        let (share_a, proof_a) = keys_a.decrypt_share(ct);
        let (share_b, proof_b) = keys_b.decrypt_share(ct);

        // attacker crafts a share that would decrypt to card 7
        let fake_share_b = ct.c1 - share_a - card_to_point(7);
        assert_eq!(
            host.reveal_card(0, &share_a, &proof_a, &fake_share_b, &proof_b),
            None,
            "forged share with reused proof must be rejected"
        );

        // proof from an unrelated key must also fail
        let mallory = Keys::generate();
        let (fake_share, fake_proof) = mallory.decrypt_share(ct);
        assert_eq!(
            host.reveal_card(0, &share_a, &proof_a, &fake_share, &fake_proof),
            None,
            "share proven under the wrong key must be rejected"
        );
    }

    #[test]
    fn test_rogue_key_rejected() {
        let keys_a = Keys::generate();
        let pop_a = keys_a.prove_possession();

        // attacker picks pk_b = q*G - pk_a so the aggregate key becomes q*G
        let q = Scalar::random(&mut OsRng);
        let rogue_pk = q * G - keys_a.pk();

        // best the attacker can do without the dlog of rogue_pk
        let rogue_pop = PossessionProof::prove(&q, &mut OsRng);
        assert!(!rogue_pop.verify(&rogue_pk));
        assert!(State::new(keys_a.pk(), rogue_pk, &pop_a, &rogue_pop).is_err());

        // reusing the honest player's pop doesn't help either
        assert!(State::new(keys_a.pk(), rogue_pk, &pop_a, &pop_a).is_err());
    }

    #[test]
    fn test_tampered_initial_deck_rejected() {
        let keys_a = Keys::generate();
        let keys_b = Keys::generate();
        let pop_a = keys_a.prove_possession();
        let pop_b = keys_b.prove_possession();

        // duplicate card (deck contains two aces, no deuce)
        let mut dup_deck = canonical_deck();
        dup_deck[1] = dup_deck[0].clone();
        assert!(State::with_initial_deck(keys_a.pk(), keys_b.pk(), &pop_a, &pop_b, &dup_deck).is_err());

        // re-encrypted deck with hidden randomness
        let aggregate = keys_a.pk() + keys_b.pk();
        let enc_deck: Vec<_> = (0..DECK_SIZE as u8)
            .map(|i| ElGamalCiphertext::encrypt(&card_to_point(i), &aggregate, &mut OsRng).0)
            .collect();
        assert!(State::with_initial_deck(keys_a.pk(), keys_b.pk(), &pop_a, &pop_b, &enc_deck).is_err());
    }

    #[test]
    fn test_reveal_only_state() {
        let (keys_a, keys_b, mut host, mut guest) = setup_players();
        let p = host.shuffle_and_prove(0).unwrap();
        assert!(guest.verify_and_apply(host.deck().to_vec(), &p).unwrap());
        let p = guest.shuffle_and_prove(1).unwrap();
        assert!(host.verify_and_apply(guest.deck().to_vec(), &p).unwrap());

        // reconstruct a reveal-only state from the final deck
        let reveal = RevealOnly::new(keys_a.pk(), keys_b.pk(), host.deck().to_vec());
        let ct = &reveal.deck()[3];
        let (share_a, proof_a) = keys_a.decrypt_share(ct);
        let (share_b, proof_b) = keys_b.decrypt_share(ct);
        assert!(reveal.reveal_card(3, &share_a, &proof_a, &share_b, &proof_b).is_some());

        // forged share still rejected
        let fake = ct.c1 - share_a - card_to_point(0);
        assert_eq!(reveal.reveal_card(3, &share_a, &proof_a, &fake, &proof_b), None);
    }
}
