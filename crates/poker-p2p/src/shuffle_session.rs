//! shuffle_session - mental poker shuffle protocol for P2P
//!
//! both peers collaboratively shuffle an encrypted deck so that:
//! - neither knows the card order
//! - both can verify the shuffle was fair (batch Chaum-Pedersen proofs)
//! - cards are revealed via partial ElGamal decryption during play
//!
//! uses zk-shuffle crate for the cryptographic operations.

use zk_shuffle::{
    ShuffleConfig, ShuffleProof, ShuffleTranscript,
    remasking::ElGamalCiphertext,
    proof::prove_shuffle,
    verify::verify_shuffle,
    dalek::{CompressedRistretto, RistrettoPoint, Scalar, RISTRETTO_BASEPOINT_POINT as G},
};
use rand_core::OsRng;
use sha2::{Sha256, Sha512, Digest};

/// Chaum-Pedersen proof of correct partial decryption
#[derive(Clone, Debug)]
pub struct RevealProof {
    /// commitment on generator G: k*G
    pub r_g: [u8; 32],
    /// commitment on ciphertext c0: k*c0
    pub r_c0: [u8; 32],
    /// response: k - challenge * sk
    pub response: [u8; 32],
}

/// state of the shuffle protocol
#[derive(Clone, Debug)]
pub enum ShuffleState {
    /// waiting for peer's public key
    WaitingForKey,
    /// we sent our shuffle, waiting for peer's shuffle
    WaitingForPeerShuffle,
    /// shuffle complete, deck ready
    Complete {
        deck: Vec<ElGamalCiphertext>,
        commitment: [u8; 32],
    },
    /// shuffle failed
    Failed(String),
}

/// shuffle session for one hand
pub struct ShuffleSession {
    /// our ElGamal secret key
    our_sk: Scalar,
    /// our ElGamal public key
    our_pk: RistrettoPoint,
    /// peer's ElGamal public key
    peer_pk: Option<RistrettoPoint>,
    /// joint public key (our_pk + peer_pk)
    joint_pk: Option<RistrettoPoint>,
    /// current deck state (encrypted)
    deck: Vec<ElGamalCiphertext>,
    /// deck after our shuffle (before peer shuffles)
    our_shuffle: Option<Vec<ElGamalCiphertext>>,
    /// our shuffle proof
    our_proof: Option<ShuffleProof>,
    /// hand number (for transcript binding)
    hand_number: u64,
    /// are we the first shuffler (host)?
    is_host: bool,
    /// current state
    pub state: ShuffleState,
    /// deck config
    config: ShuffleConfig,
    /// whether we have performed our shuffle
    we_shuffled: bool,
    /// whether peer has performed their shuffle
    peer_shuffled: bool,
}

impl ShuffleSession {
    pub fn new(hand_number: u64, is_host: bool) -> Self {
        let mut rng = OsRng;
        let sk = Scalar::random(&mut rng);
        let pk = sk * G;

        Self {
            our_sk: sk,
            our_pk: pk,
            peer_pk: None,
            joint_pk: None,
            deck: Vec::new(),
            our_shuffle: None,
            our_proof: None,
            hand_number,
            is_host,
            state: ShuffleState::WaitingForKey,
            config: ShuffleConfig::standard_deck(),
            we_shuffled: false,
            peer_shuffled: false,
        }
    }

    /// get our public key (to send to peer)
    pub fn our_public_key(&self) -> [u8; 32] {
        self.our_pk.compress().to_bytes()
    }

    /// receive peer's public key, compute joint key, create initial encrypted deck
    pub fn receive_peer_key(&mut self, peer_pk_bytes: &[u8; 32]) -> Result<(), String> {
        let peer_pk = CompressedRistretto::from_slice(peer_pk_bytes)
            .map_err(|_| "invalid peer pk".to_string())?
            .decompress()
            .ok_or_else(|| "failed to decompress peer pk".to_string())?;

        self.peer_pk = Some(peer_pk);
        let joint = self.our_pk + peer_pk;
        self.joint_pk = Some(joint);

        // create initial deck: 52 cards as trivial encryptions (r=0)
        // both parties must derive the same initial deck deterministically;
        // the shuffle+remask steps will randomize the ciphertexts
        let identity = Scalar::ZERO * G;
        self.deck = (0..52).map(|i| {
            let card_point = card_to_point(i);
            // trivial encryption: (0*G, 0*PK + M) = (identity, M)
            ElGamalCiphertext::new(identity, card_point)
        }).collect();

        Ok(())
    }

    /// perform our shuffle (host goes first, then guest)
    pub fn shuffle(&mut self) -> Result<(Vec<u8>, Vec<u8>), String> {
        let joint = self.joint_pk.ok_or("no joint key")?;
        let mut rng = OsRng;

        let perm = zk_shuffle::Permutation::random(&mut rng, 52);
        let (shuffled, randomness) = zk_shuffle::shuffle_and_remask(
            &joint, &self.deck, &perm, &mut rng,
        );

        let mut transcript = ShuffleTranscript::new(b"zk.poker", self.hand_number as u32);
        transcript.bind_aggregate_key(joint.compress().as_bytes());

        let player_id = if self.is_host { 0 } else { 1 };
        let proof = prove_shuffle(
            &self.config,
            player_id,
            &joint,
            &self.deck,
            &shuffled,
            &perm,
            &randomness,
            &mut transcript,
            &mut rng,
        ).map_err(|e| format!("proof failed: {}", e))?;

        let proof_bytes = proof.to_bytes();
        let deck_bytes = serialize_deck(&shuffled);

        self.our_shuffle = Some(shuffled.clone());
        self.our_proof = Some(proof);
        self.deck = shuffled;
        self.we_shuffled = true;
        self.try_complete();

        Ok((deck_bytes, proof_bytes))
    }

    /// receive peer's shuffle + proof, verify it
    pub fn receive_shuffle(&mut self, deck_bytes: &[u8], proof_bytes: &[u8]) -> Result<(), String> {
        let joint = self.joint_pk.ok_or("no joint key")?;
        let input_deck = &self.deck; // deck before peer shuffled
        let output_deck = deserialize_deck(deck_bytes)?;

        let proof = ShuffleProof::from_bytes(proof_bytes)
            .ok_or_else(|| "bad proof: deserialization failed".to_string())?;

        let mut transcript = ShuffleTranscript::new(b"zk.poker", self.hand_number as u32);
        transcript.bind_aggregate_key(joint.compress().as_bytes());

        let valid = verify_shuffle(
            &self.config,
            &joint,
            &proof,
            input_deck,
            &output_deck,
            &mut transcript,
        ).map_err(|e| format!("verify error: {}", e))?;

        if !valid {
            self.state = ShuffleState::Failed("shuffle proof invalid".into());
            return Err("shuffle proof INVALID — peer cheated".into());
        }

        self.deck = output_deck;
        self.peer_shuffled = true;
        self.try_complete();

        Ok(())
    }

    /// check if both parties have shuffled; if so, mark complete
    fn try_complete(&mut self) {
        if self.we_shuffled && self.peer_shuffled {
            let commitment = deck_commitment(&self.deck);
            self.state = ShuffleState::Complete {
                deck: self.deck.clone(),
                commitment,
            };
        } else {
            self.state = ShuffleState::WaitingForPeerShuffle;
        }
    }

    /// reveal a card with Chaum-Pedersen proof of correct decryption.
    ///
    /// proves DLOG equality: log_G(pk) == log_{c0}(partial)
    /// i.e. the same secret key sk was used for both pk = sk*G and partial = sk*c0.
    ///
    /// returns (partial_decryption, proof) where proof = (commitment_g, commitment_c0, response)
    pub fn reveal_card(&self, card_index: usize) -> Result<([u8; 32], RevealProof), String> {
        if card_index >= self.deck.len() {
            return Err("card index out of range".into());
        }
        let ct = &self.deck[card_index];
        let partial = self.our_sk * ct.c0;

        // Chaum-Pedersen proof: prove sk*G = pk AND sk*c0 = partial
        let mut rng = OsRng;
        let k = Scalar::random(&mut rng); // random nonce
        let r_g = k * G;            // commitment on G
        let r_c0 = k * ct.c0;       // commitment on c0

        // challenge = H(pk || c0 || partial || r_g || r_c0)
        let challenge = {
            let mut h = sha2::Sha512::new();
            h.update(b"zk.poker/reveal/v1");
            h.update(self.our_pk.compress().as_bytes());
            h.update(ct.c0.compress().as_bytes());
            h.update(partial.compress().as_bytes());
            h.update(r_g.compress().as_bytes());
            h.update(r_c0.compress().as_bytes());
            let hash: [u8; 64] = h.finalize().into();
            Scalar::from_bytes_mod_order_wide(&hash)
        };

        // response = k - challenge * sk
        let response = k - challenge * self.our_sk;

        Ok((partial.compress().to_bytes(), RevealProof {
            r_g: r_g.compress().to_bytes(),
            r_c0: r_c0.compress().to_bytes(),
            response: response.to_bytes(),
        }))
    }

    /// verify a card reveal proof from peer
    pub fn verify_reveal(
        peer_pk: &RistrettoPoint,
        c0: &RistrettoPoint,
        partial: &RistrettoPoint,
        proof: &RevealProof,
    ) -> bool {
        let r_g = match CompressedRistretto(proof.r_g).decompress() {
            Some(p) => p, None => return false,
        };
        let r_c0 = match CompressedRistretto(proof.r_c0).decompress() {
            Some(p) => p, None => return false,
        };
        let response = {
            let ct = Scalar::from_canonical_bytes(proof.response);
            if bool::from(ct.is_none()) { return false; }
            ct.unwrap()
        };

        // recompute challenge
        let challenge = {
            let mut h = sha2::Sha512::new();
            h.update(b"zk.poker/reveal/v1");
            h.update(peer_pk.compress().as_bytes());
            h.update(c0.compress().as_bytes());
            h.update(partial.compress().as_bytes());
            h.update(r_g.compress().as_bytes());
            h.update(r_c0.compress().as_bytes());
            let hash: [u8; 64] = h.finalize().into();
            Scalar::from_bytes_mod_order_wide(&hash)
        };

        // verify: response*G + challenge*pk == r_g
        let check_g = response * G + challenge * peer_pk;
        if check_g != r_g { return false; }

        // verify: response*c0 + challenge*partial == r_c0
        let check_c0 = response * c0 + challenge * partial;
        check_c0 == r_c0
    }

    /// decrypt a card given both partial decryptions
    pub fn decrypt_card(
        ct: &ElGamalCiphertext,
        our_partial: &RistrettoPoint,
        peer_partial: &RistrettoPoint,
    ) -> Result<u8, String> {
        // M = c1 - our_partial - peer_partial
        let message_point = ct.c1 - our_partial - peer_partial;
        point_to_card(&message_point).ok_or("failed to decode card".into())
    }

    /// get the deck commitment (only valid after complete)
    pub fn commitment(&self) -> Option<[u8; 32]> {
        match &self.state {
            ShuffleState::Complete { commitment, .. } => Some(*commitment),
            _ => None,
        }
    }
}

// ---------------------------------------------------------------------------
// Card ↔ Point encoding
// ---------------------------------------------------------------------------

/// map card index (0-51) to a curve point
/// uses hash-to-curve: H("zk.poker/card/" || index)
fn card_to_point(index: u8) -> RistrettoPoint {
    use sha2::Sha512;
    let mut hasher = Sha512::new();
    hasher.update(b"zk.poker/card/v1/");
    hasher.update([index]);
    RistrettoPoint::from_uniform_bytes(&hasher.finalize().into())
}

/// reverse: find which card index a point corresponds to
/// brute force over 52 cards (fine for poker, <1μs)
fn point_to_card(point: &RistrettoPoint) -> Option<u8> {
    let compressed = point.compress();
    for i in 0..52u8 {
        if card_to_point(i).compress() == compressed {
            return Some(i);
        }
    }
    None
}

fn deck_commitment(deck: &[ElGamalCiphertext]) -> [u8; 32] {
    let mut hasher = Sha256::new();
    hasher.update(b"zk.poker/deck/v1");
    for ct in deck {
        hasher.update(ct.c0.compress().as_bytes());
        hasher.update(ct.c1.compress().as_bytes());
    }
    hasher.finalize().into()
}

fn serialize_deck(deck: &[ElGamalCiphertext]) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(deck.len() * 64);
    for ct in deck {
        bytes.extend_from_slice(ct.c0.compress().as_bytes());
        bytes.extend_from_slice(ct.c1.compress().as_bytes());
    }
    bytes
}

fn deserialize_deck(bytes: &[u8]) -> Result<Vec<ElGamalCiphertext>, String> {
    if bytes.len() % 64 != 0 {
        return Err("deck bytes not multiple of 64".into());
    }
    let mut deck = Vec::new();
    for chunk in bytes.chunks(64) {
        let c0 = CompressedRistretto::from_slice(&chunk[..32])
            .map_err(|_| "bad c0".to_string())?
            .decompress().ok_or_else(|| "bad c0 decompress".to_string())?;
        let c1 = CompressedRistretto::from_slice(&chunk[32..])
            .map_err(|_| "bad c1".to_string())?
            .decompress().ok_or_else(|| "bad c1 decompress".to_string())?;
        deck.push(ElGamalCiphertext::new(c0, c1));
    }
    Ok(deck)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_card_point_roundtrip() {
        for i in 0..52u8 {
            let point = card_to_point(i);
            let decoded = point_to_card(&point).unwrap();
            assert_eq!(i, decoded);
        }
    }

    #[test]
    fn test_full_shuffle_protocol() {
        // simulate two players
        let mut host = ShuffleSession::new(1, true);
        let mut guest = ShuffleSession::new(1, false);

        // exchange keys
        let host_pk = host.our_public_key();
        let guest_pk = guest.our_public_key();
        host.receive_peer_key(&guest_pk).unwrap();
        guest.receive_peer_key(&host_pk).unwrap();

        // host shuffles first
        let (deck_bytes, proof_bytes) = host.shuffle().unwrap();
        // guest receives + verifies host's shuffle
        guest.receive_shuffle(&deck_bytes, &proof_bytes).unwrap_or_else(|e| {
            // guest needs to see the same input deck — sync deck state
            panic!("guest verify failed: {}", e);
        });

        // guest shuffles on top
        let (deck_bytes2, proof_bytes2) = guest.shuffle().unwrap();
        // host receives + verifies guest's shuffle
        host.receive_shuffle(&deck_bytes2, &proof_bytes2).unwrap();

        // both should have same commitment
        assert_eq!(host.commitment(), guest.commitment());
        assert!(host.commitment().is_some());

        // test card reveal with Chaum-Pedersen proof
        let (host_partial, host_proof) = host.reveal_card(0).unwrap();
        let (guest_partial, guest_proof) = guest.reveal_card(0).unwrap();

        // different partial decryptions (different keys)
        assert_ne!(host_partial, guest_partial);

        // verify proofs
        let ct = &host.deck[0];
        let host_pk = CompressedRistretto(host.our_public_key()).decompress().unwrap();
        let guest_pk = CompressedRistretto(guest.our_public_key()).decompress().unwrap();
        let host_p = CompressedRistretto(host_partial).decompress().unwrap();
        let guest_p = CompressedRistretto(guest_partial).decompress().unwrap();

        assert!(
            ShuffleSession::verify_reveal(&host_pk, &ct.c0, &host_p, &host_proof),
            "host reveal proof must verify"
        );
        assert!(
            ShuffleSession::verify_reveal(&guest_pk, &ct.c0, &guest_p, &guest_proof),
            "guest reveal proof must verify"
        );

        // tampered partial should fail
        let mut bad_partial = host_partial;
        bad_partial[0] ^= 0xFF;
        if let Some(bad_p) = CompressedRistretto(bad_partial).decompress() {
            assert!(
                !ShuffleSession::verify_reveal(&host_pk, &ct.c0, &bad_p, &host_proof),
                "tampered reveal must fail verification"
            );
        }
    }
}
