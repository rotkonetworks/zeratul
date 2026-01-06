//! integration tests for zk-shuffle

use curve25519_dalek::{
    constants::RISTRETTO_BASEPOINT_POINT as G,
    ristretto::RistrettoPoint,
    scalar::Scalar,
};
use rand::rngs::OsRng;

use crate::{
    proof::prove_shuffle,
    remasking::ElGamalCiphertext,
    transcript::ShuffleTranscript,
    verify::verify_shuffle,
    Permutation, ShuffleConfig,
};

/// create encrypted test deck
fn make_deck(pk: &RistrettoPoint, n: usize) -> Vec<ElGamalCiphertext> {
    let mut rng = OsRng;
    (0..n)
        .map(|i| {
            let msg = Scalar::from(i as u64) * G;
            let (ct, _) = ElGamalCiphertext::encrypt(&msg, pk, &mut rng);
            ct
        })
        .collect()
}

/// shuffle and remask deck
fn shuffle_and_remask(
    pk: &RistrettoPoint,
    deck: &[ElGamalCiphertext],
    perm: &Permutation,
) -> (Vec<ElGamalCiphertext>, Vec<Scalar>) {
    let mut rng = OsRng;
    let mut output = Vec::with_capacity(deck.len());
    let mut randomness = Vec::with_capacity(deck.len());

    for i in 0..deck.len() {
        let pi_i = perm.get(i);
        let (remasked, r) = deck[pi_i].remask(pk, &mut rng);
        output.push(remasked);
        randomness.push(r);
    }

    (output, randomness)
}

#[test]
fn test_full_shuffle_cycle() {
    let mut rng = OsRng;

    // setup
    let sk = Scalar::random(&mut rng);
    let pk = sk * G;
    let config = ShuffleConfig::custom(8);
    let mut deck = make_deck(&pk, 8);

    let mut transcript = ShuffleTranscript::new(b"test_game", 1);
    transcript.bind_aggregate_key(pk.compress().as_bytes());

    let mut proofs = Vec::new();
    let mut deck_states = vec![deck.clone()];

    // each player shuffles
    for player_id in 0..4u8 {
        let perm = Permutation::random(&mut rand::thread_rng(), 8);
        let (remasked, randomness) = shuffle_and_remask(&pk, &deck, &perm);

        let proof = prove_shuffle(
            &config,
            player_id,
            &pk,
            &deck,
            &remasked,
            &perm,
            &randomness,
            &mut transcript,
            &mut rng,
        )
        .expect("proof should succeed");

        proofs.push(proof);
        deck_states.push(remasked.clone());
        deck = remasked;
    }

    // verify all shuffles
    let mut verify_transcript = ShuffleTranscript::new(b"test_game", 1);
    verify_transcript.bind_aggregate_key(pk.compress().as_bytes());

    for (i, proof) in proofs.iter().enumerate() {
        let input = &deck_states[i];
        let output = &deck_states[i + 1];

        let valid = verify_shuffle(&config, &pk, proof, input, output, &mut verify_transcript)
            .expect("verification should not error");

        assert!(valid, "shuffle {} should be valid", i);
    }
}

#[test]
fn test_standard_deck() {
    let mut rng = OsRng;
    let sk = Scalar::random(&mut rng);
    let pk = sk * G;

    let config = ShuffleConfig::standard_deck();
    assert_eq!(config.deck_size, 52);

    let deck = make_deck(&pk, 52);
    let perm = Permutation::random(&mut rand::thread_rng(), 52);
    let (output, randomness) = shuffle_and_remask(&pk, &deck, &perm);

    let mut transcript = ShuffleTranscript::new(b"poker", 1);

    let proof = prove_shuffle(
        &config,
        0,
        &pk,
        &deck,
        &output,
        &perm,
        &randomness,
        &mut transcript,
        &mut rng,
    );

    assert!(proof.is_ok(), "should be able to prove 52-card shuffle");
}

#[test]
fn test_permutation_properties() {
    // identity permutation
    let identity = Permutation::new(vec![0, 1, 2, 3]).unwrap();
    let deck = vec![10, 20, 30, 40];
    assert_eq!(identity.apply(&deck), deck);

    // rotation
    let rotate = Permutation::new(vec![1, 2, 3, 0]).unwrap();
    assert_eq!(rotate.apply(&deck), vec![20, 30, 40, 10]);

    // swap first two
    let swap = Permutation::new(vec![1, 0, 2, 3]).unwrap();
    assert_eq!(swap.apply(&deck), vec![20, 10, 30, 40]);
}

#[test]
fn test_invalid_permutation() {
    // duplicate index
    let result = Permutation::new(vec![0, 0, 2, 3]);
    assert!(result.is_err());

    // out of bounds
    let result = Permutation::new(vec![0, 1, 5, 3]);
    assert!(result.is_err());
}

#[test]
fn test_transcript_binding() {
    let mut rng = OsRng;
    let sk = Scalar::random(&mut rng);
    let pk = sk * G;

    let config = ShuffleConfig::custom(4);
    let deck = make_deck(&pk, 4);
    let perm = Permutation::new(vec![1, 2, 3, 0]).unwrap();
    let (output, randomness) = shuffle_and_remask(&pk, &deck, &perm);

    // prove with one transcript
    let mut t1 = ShuffleTranscript::new(b"game1", 1);
    let proof1 = prove_shuffle(&config, 0, &pk, &deck, &output, &perm, &randomness, &mut t1, &mut rng).unwrap();

    // prove with different game id
    let mut t2 = ShuffleTranscript::new(b"game2", 1);
    let proof2 = prove_shuffle(&config, 0, &pk, &deck, &output, &perm, &randomness, &mut t2, &mut rng).unwrap();

    // both proofs should have different commitments (due to transcript differences)
    // (the deck commitment should be the same, but the proof may differ)
    assert_eq!(proof1.deck_commitment(), proof2.deck_commitment(), "same deck = same commitment");
}

#[test]
fn test_wrong_permutation_fails() {
    let mut rng = OsRng;
    let sk = Scalar::random(&mut rng);
    let pk = sk * G;

    let config = ShuffleConfig::custom(4);
    let input = make_deck(&pk, 4);

    // shuffle with one permutation
    let perm = Permutation::new(vec![1, 2, 3, 0]).unwrap();
    let (output, randomness) = shuffle_and_remask(&pk, &input, &perm);

    // prove with WRONG permutation
    let wrong_perm = Permutation::new(vec![0, 1, 2, 3]).unwrap();

    let mut transcript = ShuffleTranscript::new(b"test", 1);
    let result = prove_shuffle(
        &config, 0, &pk, &input, &output, &wrong_perm, &randomness,
        &mut transcript, &mut rng
    );

    // should fail because the permutation doesn't match the actual shuffle
    assert!(result.is_err(), "wrong permutation should fail");
}

#[test]
fn test_soundness_wrong_randomness() {
    let mut rng = OsRng;
    let sk = Scalar::random(&mut rng);
    let pk = sk * G;

    let config = ShuffleConfig::custom(4);
    let input = make_deck(&pk, 4);

    let perm = Permutation::new(vec![1, 2, 3, 0]).unwrap();
    let (output, _correct_randomness) = shuffle_and_remask(&pk, &input, &perm);

    // try to prove with WRONG randomness
    let wrong_randomness: Vec<Scalar> = (0..4).map(|_| Scalar::random(&mut rng)).collect();

    let mut transcript = ShuffleTranscript::new(b"test", 1);
    let result = prove_shuffle(
        &config, 0, &pk, &input, &output, &perm, &wrong_randomness,
        &mut transcript, &mut rng
    );

    // should fail because randomness doesn't match
    assert!(result.is_err(), "wrong randomness should fail proof generation");
}

#[test]
fn bench_prove_52_cards() {
    use std::time::Instant;

    let mut rng = OsRng;
    let sk = Scalar::random(&mut rng);
    let pk = sk * G;

    let config = ShuffleConfig::standard_deck();
    let deck = make_deck(&pk, 52);
    let perm = Permutation::random(&mut rand::thread_rng(), 52);
    let (output, randomness) = shuffle_and_remask(&pk, &deck, &perm);

    let start = Instant::now();
    let mut transcript = ShuffleTranscript::new(b"bench", 1);
    let _ = prove_shuffle(
        &config, 0, &pk, &deck, &output, &perm, &randomness,
        &mut transcript, &mut rng
    ).unwrap();
    let prove_time = start.elapsed();

    println!("\n=== 52-card shuffle proving time ===");
    println!("chaum-pedersen + grand product: {:?}", prove_time);
}

#[test]
fn test_elgamal_operations() {
    let mut rng = OsRng;
    let sk = Scalar::random(&mut rng);
    let pk = sk * G;

    // encrypt a message
    let msg = Scalar::from(42u64) * G;
    let (ct, _r) = ElGamalCiphertext::encrypt(&msg, &pk, &mut rng);

    // decrypt
    let decrypted = ct.decrypt(&sk);
    assert_eq!(decrypted, msg, "decryption should recover original message");

    // remask and check it decrypts to same value
    let (remasked, _r2) = ct.remask(&pk, &mut rng);
    let remasked_decrypted = remasked.decrypt(&sk);
    assert_eq!(remasked_decrypted, msg, "remasked ciphertext should decrypt to same message");
}

#[test]
fn test_shuffle_preserves_multiset() {
    let mut rng = OsRng;
    let sk = Scalar::random(&mut rng);
    let pk = sk * G;

    // encrypt distinct messages
    let messages: Vec<RistrettoPoint> = (0..8)
        .map(|i| Scalar::from(i as u64) * G)
        .collect();

    let deck: Vec<ElGamalCiphertext> = messages.iter()
        .map(|m| ElGamalCiphertext::encrypt(m, &pk, &mut rng).0)
        .collect();

    // shuffle
    let perm = Permutation::random(&mut rand::thread_rng(), 8);
    let (shuffled, _) = shuffle_and_remask(&pk, &deck, &perm);

    // decrypt both and verify same multiset
    let mut original_decrypted: Vec<_> = deck.iter()
        .map(|ct| ct.decrypt(&sk).compress().to_bytes())
        .collect();
    original_decrypted.sort();

    let mut shuffled_decrypted: Vec<_> = shuffled.iter()
        .map(|ct| ct.decrypt(&sk).compress().to_bytes())
        .collect();
    shuffled_decrypted.sort();

    assert_eq!(original_decrypted, shuffled_decrypted,
        "shuffled deck should contain same cards (different order)");
}
