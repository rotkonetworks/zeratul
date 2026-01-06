//! shuffle proof verification
//!
//! verifies batch chaum-pedersen proofs for valid remasking
//! and scalar grand product for permutation correctness

#[cfg(not(feature = "std"))]
use alloc::format;

use curve25519_dalek::ristretto::RistrettoPoint;

use crate::{
    proof::{ShuffleProof, compute_deck_commitment},
    remasking::{ElGamalCiphertext, RemaskingStatement, RemaskingVerifier},
    transcript::ShuffleTranscript,
    Result, ShuffleConfig, ShuffleError,
};

/// verify a shuffle proof (chaum-pedersen + scalar grand product)
pub fn verify_shuffle(
    config: &ShuffleConfig,
    pk: &RistrettoPoint,
    proof: &ShuffleProof,
    input_deck: &[ElGamalCiphertext],
    output_deck: &[ElGamalCiphertext],
    transcript: &mut ShuffleTranscript,
) -> Result<bool> {
    // validate deck sizes
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

    // 1. verify deck commitment
    let expected_commitment = compute_deck_commitment(output_deck);
    if expected_commitment != proof.shuffled_deck_commitment {
        return Err(ShuffleError::VerificationError(
            "deck commitment mismatch".into(),
        ));
    }

    // derive context from transcript (must match prover's context)
    let mut context = [0u8; 32];
    transcript.get_seed(b"remasking_context", &mut context);

    // 2. verify remasking proof (batch chaum-pedersen + scalar grand product)
    let remasking_statement = RemaskingStatement {
        pk: *pk,
        input_deck: input_deck.to_vec(),
        output_deck: output_deck.to_vec(),
    };

    let remasking_valid = RemaskingVerifier::verify(
        &remasking_statement,
        &proof.deltas,
        &proof.remasking_proof,
        Some(&context),
    ).map_err(|e| ShuffleError::VerificationError(format!("remasking: {}", e)))?;

    if !remasking_valid {
        return Ok(false);
    }

    // 3. bind to transcript
    transcript.bind_shuffle(proof.player_id, &proof.shuffled_deck_commitment);

    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;
    use curve25519_dalek::{constants::RISTRETTO_BASEPOINT_POINT as G, scalar::Scalar};
    use rand::rngs::OsRng;
    use crate::{proof::prove_shuffle, Permutation};

    fn make_test_deck(pk: &RistrettoPoint, n: usize) -> Vec<ElGamalCiphertext> {
        let mut rng = OsRng;
        (0..n)
            .map(|i| {
                let msg = Scalar::from(i as u64) * G;
                let (ct, _) = ElGamalCiphertext::encrypt(&msg, pk, &mut rng);
                ct
            })
            .collect()
    }

    #[test]
    fn test_verify_shuffle() {
        let mut rng = OsRng;
        let sk = Scalar::random(&mut rng);
        let pk = sk * G;

        let config = ShuffleConfig::custom(4);
        let input = make_test_deck(&pk, 4);
        let perm = Permutation::new(vec![1, 2, 3, 0]).unwrap();

        // shuffle and remask
        let mut output = Vec::with_capacity(4);
        let mut randomness = Vec::with_capacity(4);
        for i in 0..4 {
            let pi_i = perm.get(i);
            let (remasked, r) = input[pi_i].remask(&pk, &mut rng);
            output.push(remasked);
            randomness.push(r);
        }

        // prove
        let mut prove_transcript = ShuffleTranscript::new(b"test", 1);
        let proof = prove_shuffle(
            &config, 0, &pk, &input, &output, &perm, &randomness,
            &mut prove_transcript, &mut rng
        ).expect("proof should succeed");

        // verify
        let mut verify_transcript = ShuffleTranscript::new(b"test", 1);
        let valid = verify_shuffle(&config, &pk, &proof, &input, &output, &mut verify_transcript)
            .expect("verification should not error");

        assert!(valid, "valid proof should verify");
    }

    #[test]
    fn test_verify_wrong_deck() {
        let mut rng = OsRng;
        let sk = Scalar::random(&mut rng);
        let pk = sk * G;

        let config = ShuffleConfig::custom(4);
        let input = make_test_deck(&pk, 4);
        let perm = Permutation::new(vec![1, 2, 3, 0]).unwrap();

        let mut output = Vec::with_capacity(4);
        let mut randomness = Vec::with_capacity(4);
        for i in 0..4 {
            let pi_i = perm.get(i);
            let (remasked, r) = input[pi_i].remask(&pk, &mut rng);
            output.push(remasked);
            randomness.push(r);
        }

        // prove
        let mut prove_transcript = ShuffleTranscript::new(b"test", 1);
        let proof = prove_shuffle(
            &config, 0, &pk, &input, &output, &perm, &randomness,
            &mut prove_transcript, &mut rng
        ).expect("proof should succeed");

        // verify with WRONG output deck
        let wrong_output = make_test_deck(&pk, 4);

        let mut verify_transcript = ShuffleTranscript::new(b"test", 1);
        let result = verify_shuffle(&config, &pk, &proof, &input, &wrong_output, &mut verify_transcript);

        assert!(result.is_err(), "wrong deck should fail verification");
    }
}
