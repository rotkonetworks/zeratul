//! shuffle proof generation
//!
//! uses batch chaum-pedersen for valid remasking
//! and scalar grand product for permutation correctness

#[cfg(not(feature = "std"))]
use alloc::{format, vec::Vec};

use curve25519_dalek::{ristretto::RistrettoPoint, scalar::Scalar};

use crate::{
    remasking::{
        BatchRemaskingProof, ElGamalCiphertext, RemaskingDelta,
        RemaskingProver, RemaskingStatement, RemaskingWitness,
    },
    transcript::{Blake2Transcript, ShuffleTranscript},
    Permutation, Result, ShuffleConfig, ShuffleError,
};

/// shuffle proof: batch chaum-pedersen + scalar grand product
#[derive(Clone)]
pub struct ShuffleProof {
    /// batch chaum-pedersen proof for valid remasking
    pub remasking_proof: BatchRemaskingProof,
    /// remasking deltas (output[i] - input[π(i)])
    pub deltas: Vec<RemaskingDelta>,
    /// commitment to shuffled deck
    pub shuffled_deck_commitment: Vec<u8>,
    /// player who performed shuffle
    pub player_id: u8,
}

impl ShuffleProof {
    /// get player id
    pub fn player_id(&self) -> u8 {
        self.player_id
    }

    /// get deck commitment
    pub fn deck_commitment(&self) -> &[u8] {
        &self.shuffled_deck_commitment
    }

    /// serialize the proof for transmission
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut bytes = Vec::new();

        bytes.push(self.player_id);

        let commit_len = self.shuffled_deck_commitment.len() as u32;
        bytes.extend_from_slice(&commit_len.to_le_bytes());
        bytes.extend_from_slice(&self.shuffled_deck_commitment);

        let delta_count = self.deltas.len() as u32;
        bytes.extend_from_slice(&delta_count.to_le_bytes());

        for delta in &self.deltas {
            bytes.extend_from_slice(delta.delta_c0.compress().as_bytes());
            bytes.extend_from_slice(delta.delta_c1.compress().as_bytes());
        }

        bytes.extend_from_slice(self.remasking_proof.commitment_g.compress().as_bytes());
        bytes.extend_from_slice(self.remasking_proof.commitment_pk.compress().as_bytes());
        bytes.extend_from_slice(self.remasking_proof.response.as_bytes());

        bytes
    }
}

/// prove shuffle (permutation + remasking)
pub fn prove_shuffle<R: rand_core::RngCore + rand_core::CryptoRng>(
    config: &ShuffleConfig,
    player_id: u8,
    pk: &RistrettoPoint,
    input_deck: &[ElGamalCiphertext],
    output_deck: &[ElGamalCiphertext],
    permutation: &Permutation,
    randomness: &[Scalar],
    transcript: &mut ShuffleTranscript,
    rng: &mut R,
) -> Result<ShuffleProof> {
    // validate inputs
    if input_deck.len() != config.deck_size {
        return Err(ShuffleError::DeckSizeMismatch {
            expected: config.deck_size,
            got: input_deck.len(),
        });
    }
    if output_deck.len() != config.deck_size {
        return Err(ShuffleError::DeckSizeMismatch {
            expected: config.deck_size,
            got: output_deck.len(),
        });
    }
    if permutation.len() != config.deck_size || randomness.len() != config.deck_size {
        return Err(ShuffleError::DeckSizeMismatch {
            expected: config.deck_size,
            got: permutation.len(),
        });
    }

    // derive context from transcript for binding to game session
    let mut context = [0u8; 32];
    transcript.get_seed(b"remasking_context", &mut context);

    // generate remasking proof (batch chaum-pedersen + scalar grand product)
    let remasking_statement = RemaskingStatement {
        pk: *pk,
        input_deck: input_deck.to_vec(),
        output_deck: output_deck.to_vec(),
    };
    let remasking_witness = RemaskingWitness::new(
        randomness.to_vec(),
        permutation.mapping().to_vec(),
    );

    let (deltas, remasking_proof) = RemaskingProver::prove(
        &remasking_statement,
        &remasking_witness,
        Some(&context),
        rng,
    ).map_err(|e| ShuffleError::ProofError(format!("remasking: {}", e)))?;

    // compute deck commitment
    let shuffled_deck_commitment = compute_deck_commitment(output_deck);
    transcript.bind_shuffle(player_id, &shuffled_deck_commitment);

    Ok(ShuffleProof {
        remasking_proof,
        deltas,
        shuffled_deck_commitment,
        player_id,
    })
}

/// compute deck commitment from ciphertexts
pub fn compute_deck_commitment(deck: &[ElGamalCiphertext]) -> Vec<u8> {
    let mut t = Blake2Transcript::new(b"deck_commitment");
    for ct in deck {
        t.append_message(b"ct", &ct.to_bytes());
    }
    let mut bytes = [0u8; 32];
    t.challenge_bytes(b"commit", &mut bytes);
    bytes.to_vec()
}

#[cfg(test)]
mod tests {
    use super::*;
    use curve25519_dalek::constants::RISTRETTO_BASEPOINT_POINT as G;
    use rand::rngs::OsRng;

    fn make_test_deck(pk: &RistrettoPoint, n: usize) -> Vec<ElGamalCiphertext> {
        let mut rng = OsRng;
        (0..n).map(|i| {
            let msg = Scalar::from(i as u64) * G;
            let (ct, _) = ElGamalCiphertext::encrypt(&msg, pk, &mut rng);
            ct
        }).collect()
    }

    #[test]
    fn test_prove_shuffle() {
        let mut rng = OsRng;
        let sk = Scalar::random(&mut rng);
        let pk = sk * G;

        let config = ShuffleConfig::custom(4);
        let input_deck = make_test_deck(&pk, 4);

        let perm = Permutation::new(vec![1, 2, 3, 0]).unwrap();
        let mut output_deck = Vec::with_capacity(4);
        let mut randomness = Vec::with_capacity(4);

        for i in 0..4 {
            let pi_i = perm.get(i);
            let (remasked, r) = input_deck[pi_i].remask(&pk, &mut rng);
            output_deck.push(remasked);
            randomness.push(r);
        }

        let mut transcript = ShuffleTranscript::new(b"test", 1);

        let proof = prove_shuffle(
            &config, 0, &pk, &input_deck, &output_deck, &perm, &randomness,
            &mut transcript, &mut rng
        );

        assert!(proof.is_ok(), "proof should succeed: {:?}", proof.err());
        let proof = proof.unwrap();
        assert_eq!(proof.player_id(), 0);
        assert_eq!(proof.deltas.len(), 4);
    }

    #[test]
    fn test_deck_commitment() {
        let mut rng = OsRng;
        let sk = Scalar::random(&mut rng);
        let pk = sk * G;

        let deck1 = make_test_deck(&pk, 2);
        let deck2 = make_test_deck(&pk, 2);

        let c1 = compute_deck_commitment(&deck1);
        let c2 = compute_deck_commitment(&deck2);

        assert_ne!(c1, c2, "different decks should have different commitments");

        let c1_again = compute_deck_commitment(&deck1);
        assert_eq!(c1, c1_again, "same deck should have same commitment");
    }
}
