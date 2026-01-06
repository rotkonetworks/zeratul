//! minimal on-chain verifier for mental poker shuffle proofs
//!
//! uses batch chaum-pedersen over ristretto255 with grand product permutation check.
//! designed for polkavm/revive smart contracts - no arkworks dependencies.

#[cfg(not(feature = "std"))]
use alloc::vec::Vec;

#[cfg(feature = "std")]
use std::vec::Vec;

use blake2::{Blake2s256, Digest};
use curve25519_dalek::{
    constants::RISTRETTO_BASEPOINT_POINT as G,
    ristretto::{CompressedRistretto, RistrettoPoint},
    scalar::Scalar,
};

// ============================================================================
// ERROR TYPES
// ============================================================================

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VerifyError {
    DeckSizeMismatch,
    InvalidPoint,
    InvalidScalar,
    ProofMismatch,
    PermutationInvalid,
}

pub type Result<T> = core::result::Result<T, VerifyError>;

// ============================================================================
// TRANSCRIPT (blake2-based fiat-shamir)
// ============================================================================

#[derive(Clone)]
pub struct Blake2Transcript {
    state: Blake2s256,
    challenge_counter: u64,
}

impl Blake2Transcript {
    pub fn new(domain_sep: &[u8]) -> Self {
        let mut state = Blake2s256::new();
        state.update(b"blake2-transcript-v1");
        state.update(&(domain_sep.len() as u32).to_le_bytes());
        state.update(domain_sep);
        Self {
            state,
            challenge_counter: 0,
        }
    }

    pub fn append_message(&mut self, label: &[u8], message: &[u8]) {
        self.state.update(&(label.len() as u32).to_le_bytes());
        self.state.update(label);
        self.state.update(&(message.len() as u32).to_le_bytes());
        self.state.update(message);
    }

    pub fn append_u64(&mut self, label: &[u8], value: u64) {
        self.append_message(label, &value.to_le_bytes());
    }

    pub fn challenge_bytes(&mut self, label: &[u8], dest: &mut [u8]) {
        let mut challenge_state = self.state.clone();
        challenge_state.update(b"challenge");
        challenge_state.update(&(label.len() as u32).to_le_bytes());
        challenge_state.update(label);
        challenge_state.update(&self.challenge_counter.to_le_bytes());
        self.challenge_counter += 1;

        let hash = challenge_state.finalize();

        if dest.len() <= 32 {
            dest.copy_from_slice(&hash[..dest.len()]);
        } else {
            let mut offset = 0;
            let mut seed = [0u8; 32];
            seed.copy_from_slice(&hash);
            while offset < dest.len() {
                let take = (dest.len() - offset).min(32);
                dest[offset..offset + take].copy_from_slice(&seed[..take]);
                offset += take;
                if offset < dest.len() {
                    let mut h = Blake2s256::new();
                    h.update(&seed);
                    h.update(b"extend");
                    let new_hash = h.finalize();
                    seed.copy_from_slice(&new_hash);
                }
            }
        }

        self.state.update(b"challenge_out");
        self.state.update(dest);
    }

    fn challenge_scalar(&mut self, label: &[u8]) -> Scalar {
        let mut bytes = [0u8; 64];
        self.challenge_bytes(label, &mut bytes);
        Scalar::from_bytes_mod_order_wide(&bytes)
    }
}

// ============================================================================
// ELGAMAL CIPHERTEXT
// ============================================================================

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ElGamalCiphertext {
    pub c0: RistrettoPoint,
    pub c1: RistrettoPoint,
}

impl ElGamalCiphertext {
    pub fn from_bytes(bytes: &[u8; 64]) -> Result<Self> {
        let c0 = CompressedRistretto::from_slice(&bytes[..32])
            .map_err(|_| VerifyError::InvalidPoint)?
            .decompress()
            .ok_or(VerifyError::InvalidPoint)?;
        let c1 = CompressedRistretto::from_slice(&bytes[32..])
            .map_err(|_| VerifyError::InvalidPoint)?
            .decompress()
            .ok_or(VerifyError::InvalidPoint)?;
        Ok(Self { c0, c1 })
    }

    pub fn to_bytes(&self) -> [u8; 64] {
        let mut bytes = [0u8; 64];
        bytes[..32].copy_from_slice(self.c0.compress().as_bytes());
        bytes[32..].copy_from_slice(self.c1.compress().as_bytes());
        bytes
    }
}

// ============================================================================
// REMASKING DELTA
// ============================================================================

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RemaskingDelta {
    pub delta_c0: RistrettoPoint,
    pub delta_c1: RistrettoPoint,
}

impl RemaskingDelta {
    pub fn from_bytes(bytes: &[u8; 64]) -> Result<Self> {
        let delta_c0 = CompressedRistretto::from_slice(&bytes[..32])
            .map_err(|_| VerifyError::InvalidPoint)?
            .decompress()
            .ok_or(VerifyError::InvalidPoint)?;
        let delta_c1 = CompressedRistretto::from_slice(&bytes[32..])
            .map_err(|_| VerifyError::InvalidPoint)?
            .decompress()
            .ok_or(VerifyError::InvalidPoint)?;
        Ok(Self { delta_c0, delta_c1 })
    }

    pub fn to_bytes(&self) -> [u8; 64] {
        let mut bytes = [0u8; 64];
        bytes[..32].copy_from_slice(self.delta_c0.compress().as_bytes());
        bytes[32..].copy_from_slice(self.delta_c1.compress().as_bytes());
        bytes
    }
}

// ============================================================================
// BATCH REMASKING PROOF
// ============================================================================

#[derive(Clone, Debug)]
pub struct BatchRemaskingProof {
    pub commitment_g: RistrettoPoint,
    pub commitment_pk: RistrettoPoint,
    pub response: Scalar,
}

impl BatchRemaskingProof {
    pub fn from_bytes(bytes: &[u8; 96]) -> Result<Self> {
        let commitment_g = CompressedRistretto::from_slice(&bytes[..32])
            .map_err(|_| VerifyError::InvalidPoint)?
            .decompress()
            .ok_or(VerifyError::InvalidPoint)?;
        let commitment_pk = CompressedRistretto::from_slice(&bytes[32..64])
            .map_err(|_| VerifyError::InvalidPoint)?
            .decompress()
            .ok_or(VerifyError::InvalidPoint)?;

        let mut scalar_bytes = [0u8; 32];
        scalar_bytes.copy_from_slice(&bytes[64..96]);
        let response = Scalar::from_canonical_bytes(scalar_bytes)
            .into_option()
            .ok_or(VerifyError::InvalidScalar)?;

        Ok(Self {
            commitment_g,
            commitment_pk,
            response,
        })
    }
}

// ============================================================================
// VERIFIER
// ============================================================================

/// verify batch remasking proof
///
/// checks:
/// 1. z * G = R + c * sum(rho_i * delta_c0[i])
/// 2. z * PK = S + c * sum(rho_i * delta_c1[i])
/// 3. stripped deck is permutation of input deck (grand product)
pub fn verify_remasking(
    pk: &RistrettoPoint,
    input_deck: &[ElGamalCiphertext],
    output_deck: &[ElGamalCiphertext],
    deltas: &[RemaskingDelta],
    proof: &BatchRemaskingProof,
    context: Option<&[u8]>,
) -> Result<bool> {
    let n = input_deck.len();
    if output_deck.len() != n || deltas.len() != n {
        return Err(VerifyError::DeckSizeMismatch);
    }

    // create transcript
    let mut transcript = Blake2Transcript::new(b"zk-shuffle.remasking.v1");

    // bind context
    if let Some(ctx) = context {
        transcript.append_message(b"context", ctx);
    }

    // bind statement
    transcript.append_message(b"pk", pk.compress().as_bytes());
    transcript.append_u64(b"n", n as u64);

    for card in input_deck {
        transcript.append_message(b"in", &card.to_bytes());
    }
    for card in output_deck {
        transcript.append_message(b"out", &card.to_bytes());
    }

    // bind deltas
    for delta in deltas {
        transcript.append_message(b"delta", &delta.to_bytes());
    }

    // derive batch weights
    let rho: Vec<Scalar> = (0..n)
        .map(|i| {
            let s = transcript.challenge_scalar(b"rho");
            transcript.append_u64(b"rho_idx", i as u64);
            s
        })
        .collect();

    // bind commitments and derive challenge
    transcript.append_message(b"R", proof.commitment_g.compress().as_bytes());
    transcript.append_message(b"S", proof.commitment_pk.compress().as_bytes());
    let c = transcript.challenge_scalar(b"c");

    // aggregate deltas with weights
    let agg_delta_c0: RistrettoPoint = deltas
        .iter()
        .zip(rho.iter())
        .map(|(d, r)| r * &d.delta_c0)
        .sum();
    let agg_delta_c1: RistrettoPoint = deltas
        .iter()
        .zip(rho.iter())
        .map(|(d, r)| r * &d.delta_c1)
        .sum();

    // verify equation 1: z * G = R + c * agg_delta_c0
    let lhs_g = proof.response * G;
    let rhs_g = proof.commitment_g + c * agg_delta_c0;

    if lhs_g != rhs_g {
        return Ok(false);
    }

    // verify equation 2: z * PK = S + c * agg_delta_c1
    let lhs_pk = proof.response * pk;
    let rhs_pk = proof.commitment_pk + c * agg_delta_c1;

    if lhs_pk != rhs_pk {
        return Ok(false);
    }

    // verify permutation property via grand product
    verify_permutation_property(input_deck, output_deck, deltas, &mut transcript)
}

fn verify_permutation_property(
    input_deck: &[ElGamalCiphertext],
    output_deck: &[ElGamalCiphertext],
    deltas: &[RemaskingDelta],
    transcript: &mut Blake2Transcript,
) -> Result<bool> {
    let n = input_deck.len();

    // derive beta
    let beta = transcript.challenge_scalar(b"beta");

    // hash input cards
    let input_hashes: Vec<Scalar> = input_deck.iter().map(hash_card).collect();

    // hash stripped cards (output - delta)
    let stripped_hashes: Vec<Scalar> = (0..n)
        .map(|i| {
            let stripped = ElGamalCiphertext {
                c0: output_deck[i].c0 - deltas[i].delta_c0,
                c1: output_deck[i].c1 - deltas[i].delta_c1,
            };
            hash_card(&stripped)
        })
        .collect();

    // grand product check
    let prod_input: Scalar = input_hashes.iter().map(|h| h + beta).product();
    let prod_stripped: Scalar = stripped_hashes.iter().map(|h| h + beta).product();

    Ok(prod_input == prod_stripped)
}

fn hash_card(card: &ElGamalCiphertext) -> Scalar {
    let mut transcript = Blake2Transcript::new(b"zk-shuffle.card.v1");
    transcript.append_message(b"c0", card.c0.compress().as_bytes());
    transcript.append_message(b"c1", card.c1.compress().as_bytes());
    transcript.challenge_scalar(b"h")
}

// ============================================================================
// COMPACT PROOF FORMAT (for on-chain submission)
// ============================================================================

/// serialized proof for on-chain submission
/// layout:
///   - pk: 32 bytes (compressed ristretto)
///   - proof: 96 bytes (commitment_g, commitment_pk, response)
///   - n_cards: 2 bytes (u16, max 65535)
///   - deltas: n_cards * 64 bytes
///   - context_len: 2 bytes
///   - context: context_len bytes
///
/// note: input_deck and output_deck are provided separately (from state)
pub fn verify_serialized_proof(
    proof_bytes: &[u8],
    input_deck_bytes: &[u8],
    output_deck_bytes: &[u8],
) -> Result<bool> {
    if proof_bytes.len() < 32 + 96 + 2 {
        return Err(VerifyError::ProofMismatch);
    }

    // parse pk
    let pk = CompressedRistretto::from_slice(&proof_bytes[..32])
        .map_err(|_| VerifyError::InvalidPoint)?
        .decompress()
        .ok_or(VerifyError::InvalidPoint)?;

    // parse proof
    let mut proof_arr = [0u8; 96];
    proof_arr.copy_from_slice(&proof_bytes[32..128]);
    let proof = BatchRemaskingProof::from_bytes(&proof_arr)?;

    // parse n_cards
    let n_cards = u16::from_le_bytes([proof_bytes[128], proof_bytes[129]]) as usize;

    // validate sizes
    let deltas_start = 130;
    let deltas_end = deltas_start + n_cards * 64;

    if proof_bytes.len() < deltas_end + 2 {
        return Err(VerifyError::ProofMismatch);
    }

    // parse deltas
    let mut deltas = Vec::with_capacity(n_cards);
    for i in 0..n_cards {
        let start = deltas_start + i * 64;
        let mut delta_bytes = [0u8; 64];
        delta_bytes.copy_from_slice(&proof_bytes[start..start + 64]);
        deltas.push(RemaskingDelta::from_bytes(&delta_bytes)?);
    }

    // parse context
    let context_len = u16::from_le_bytes([proof_bytes[deltas_end], proof_bytes[deltas_end + 1]]) as usize;
    let context = if context_len > 0 {
        if proof_bytes.len() < deltas_end + 2 + context_len {
            return Err(VerifyError::ProofMismatch);
        }
        Some(&proof_bytes[deltas_end + 2..deltas_end + 2 + context_len])
    } else {
        None
    };

    // parse input deck
    if input_deck_bytes.len() != n_cards * 64 {
        return Err(VerifyError::DeckSizeMismatch);
    }
    let mut input_deck = Vec::with_capacity(n_cards);
    for i in 0..n_cards {
        let mut card_bytes = [0u8; 64];
        card_bytes.copy_from_slice(&input_deck_bytes[i * 64..(i + 1) * 64]);
        input_deck.push(ElGamalCiphertext::from_bytes(&card_bytes)?);
    }

    // parse output deck
    if output_deck_bytes.len() != n_cards * 64 {
        return Err(VerifyError::DeckSizeMismatch);
    }
    let mut output_deck = Vec::with_capacity(n_cards);
    for i in 0..n_cards {
        let mut card_bytes = [0u8; 64];
        card_bytes.copy_from_slice(&output_deck_bytes[i * 64..(i + 1) * 64]);
        output_deck.push(ElGamalCiphertext::from_bytes(&card_bytes)?);
    }

    verify_remasking(&pk, &input_deck, &output_deck, &deltas, &proof, context)
}

#[cfg(test)]
mod tests {
    use super::*;
    use rand::rngs::OsRng;

    fn random_point() -> RistrettoPoint {
        let sk = Scalar::random(&mut OsRng);
        sk * G
    }

    #[test]
    fn test_transcript_determinism() {
        let mut t1 = Blake2Transcript::new(b"test");
        let mut t2 = Blake2Transcript::new(b"test");

        t1.append_message(b"data", b"hello");
        t2.append_message(b"data", b"hello");

        let s1 = t1.challenge_scalar(b"chal");
        let s2 = t2.challenge_scalar(b"chal");

        assert_eq!(s1, s2);
    }

    #[test]
    fn test_elgamal_roundtrip() {
        let ct = ElGamalCiphertext {
            c0: random_point(),
            c1: random_point(),
        };
        let bytes = ct.to_bytes();
        let recovered = ElGamalCiphertext::from_bytes(&bytes).unwrap();
        assert_eq!(ct, recovered);
    }

    #[test]
    fn test_delta_roundtrip() {
        let delta = RemaskingDelta {
            delta_c0: random_point(),
            delta_c1: random_point(),
        };
        let bytes = delta.to_bytes();
        let recovered = RemaskingDelta::from_bytes(&bytes).unwrap();
        assert_eq!(delta, recovered);
    }
}
