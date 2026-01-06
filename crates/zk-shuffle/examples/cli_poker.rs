//! cli poker demo
//!
//! demonstrates full mental poker flow:
//! 1. key generation
//! 2. deck encryption
//! 3. shuffle + prove (each player)
//! 4. card reveal
//! 5. hand evaluation
//! 6. audit + settlement

use zk_shuffle::{
    audit::{RevealAudit, RevealProof, RevealedCard},
    poker::{Card, evaluate_best_hand, determine_winners},
    proof::prove_shuffle,
    remasking::ElGamalCiphertext,
    transcript::ShuffleTranscript,
    verify::verify_shuffle,
    Permutation, ShuffleConfig,
};
use curve25519_dalek::{
    constants::RISTRETTO_BASEPOINT_POINT as G,
    ristretto::RistrettoPoint,
    scalar::Scalar,
};
use rand::rngs::OsRng;
use std::time::Instant;

const NUM_PLAYERS: usize = 4;

fn main() {
    println!("=== zk-shuffle cli poker demo ===\n");

    let mut rng = OsRng;
    let game_id = [0u8; 32]; // would be random in real game

    // =========================================================================
    // phase 1: key generation
    // =========================================================================
    println!("phase 1: key generation");

    let mut secret_keys = Vec::with_capacity(NUM_PLAYERS);
    let mut public_keys = Vec::with_capacity(NUM_PLAYERS);

    for i in 0..NUM_PLAYERS {
        let sk = Scalar::random(&mut rng);
        let pk = sk * G;
        println!("  player {}: pk = {:?}...", i, &pk.compress().as_bytes()[..8]);
        secret_keys.push(sk);
        public_keys.push(pk);
    }

    // aggregate public key (for threshold decryption)
    let aggregate_pk: RistrettoPoint = public_keys.iter().sum();
    println!("  aggregate pk: {:?}...\n", &aggregate_pk.compress().as_bytes()[..8]);

    // =========================================================================
    // phase 2: initial deck encryption
    // =========================================================================
    println!("phase 2: initial deck encryption");

    let config = ShuffleConfig::standard_deck();
    let mut deck: Vec<ElGamalCiphertext> = Vec::with_capacity(52);

    // encrypt each card (0-51) with aggregate pk
    for card_value in 0u64..52 {
        let msg = Scalar::from(card_value) * G;
        let (ct, _) = ElGamalCiphertext::encrypt(&msg, &aggregate_pk, &mut rng);
        deck.push(ct);
    }
    println!("  encrypted 52 cards with aggregate pk\n");

    // =========================================================================
    // phase 3: each player shuffles
    // =========================================================================
    println!("phase 3: shuffle phase");

    let mut transcript = ShuffleTranscript::new(&game_id, 1);
    transcript.bind_aggregate_key(aggregate_pk.compress().as_bytes());

    let mut all_proofs = Vec::new();
    let mut deck_history = vec![deck.clone()];

    for player_id in 0..NUM_PLAYERS {
        let start = Instant::now();

        // generate random permutation
        let perm = Permutation::random(&mut rand::thread_rng(), 52);

        // shuffle and remask
        let mut new_deck = Vec::with_capacity(52);
        let mut randomness = Vec::with_capacity(52);

        for i in 0..52 {
            let pi_i = perm.get(i);
            let (remasked, r) = deck[pi_i].remask(&aggregate_pk, &mut rng);
            new_deck.push(remasked);
            randomness.push(r);
        }

        // prove shuffle
        let proof = prove_shuffle(
            &config,
            player_id as u8,
            &aggregate_pk,
            &deck,
            &new_deck,
            &perm,
            &randomness,
            &mut transcript,
            &mut rng,
        ).expect("proof should succeed");

        let elapsed = start.elapsed();
        println!("  player {} shuffled and proved in {:?}", player_id, elapsed);

        all_proofs.push(proof);
        deck_history.push(new_deck.clone());
        deck = new_deck;
    }
    println!();

    // =========================================================================
    // phase 4: verify all shuffles
    // =========================================================================
    println!("phase 4: verify all shuffles");

    let mut verify_transcript = ShuffleTranscript::new(&game_id, 1);
    verify_transcript.bind_aggregate_key(aggregate_pk.compress().as_bytes());

    for (i, proof) in all_proofs.iter().enumerate() {
        let start = Instant::now();

        let input = &deck_history[i];
        let output = &deck_history[i + 1];

        let valid = verify_shuffle(
            &config,
            &aggregate_pk,
            proof,
            input,
            output,
            &mut verify_transcript,
        ).expect("verification should not error");

        let elapsed = start.elapsed();

        if valid {
            println!("  player {} shuffle: VALID ({:?})", i, elapsed);
        } else {
            println!("  player {} shuffle: INVALID", i);
            return;
        }
    }
    println!();

    // =========================================================================
    // phase 5: deal cards (decrypt specific positions)
    // =========================================================================
    println!("phase 5: deal cards");

    // for demo, we'll "reveal" by computing what card is at each position
    // in real game, this requires threshold decryption

    // deal hole cards: 2 per player
    let mut player_hole_cards: Vec<Vec<Card>> = vec![Vec::new(); NUM_PLAYERS];
    let mut community_cards: Vec<Card> = Vec::new();

    let final_deck = &deck;

    // simulate card reveals (in real game, players contribute decryption shares)
    fn decrypt_card(
        ct: &ElGamalCiphertext,
        secret_keys: &[Scalar],
    ) -> u8 {
        // threshold decryption: each player contributes sk_i * c0
        let decryption_shares: RistrettoPoint = secret_keys.iter()
            .map(|sk| sk * ct.c0)
            .sum();

        // message = c1 - sum(sk_i * c0)
        let msg = ct.c1 - decryption_shares;

        // brute force to find card value (in practice use baby-step giant-step)
        for v in 0u64..52 {
            if Scalar::from(v) * G == msg {
                return v as u8;
            }
        }
        255 // not found
    }

    // deal hole cards
    let mut card_idx = 0;
    for player in 0..NUM_PLAYERS {
        for _ in 0..2 {
            let card_value = decrypt_card(&final_deck[card_idx], &secret_keys);
            let card = Card::from_index(card_value).expect("valid card");
            player_hole_cards[player].push(card);
            card_idx += 1;
        }
    }

    // burn + flop (3 cards)
    card_idx += 1; // burn
    for _ in 0..3 {
        let card_value = decrypt_card(&final_deck[card_idx], &secret_keys);
        let card = Card::from_index(card_value).expect("valid card");
        community_cards.push(card);
        card_idx += 1;
    }

    // burn + turn
    card_idx += 1;
    let card_value = decrypt_card(&final_deck[card_idx], &secret_keys);
    community_cards.push(Card::from_index(card_value).expect("valid card"));
    card_idx += 1;

    // burn + river
    card_idx += 1;
    let card_value = decrypt_card(&final_deck[card_idx], &secret_keys);
    community_cards.push(Card::from_index(card_value).expect("valid card"));

    // print hands
    for (i, cards) in player_hole_cards.iter().enumerate() {
        println!("  player {}: {} {}", i, cards[0], cards[1]);
    }
    println!("  community: {}", community_cards.iter()
        .map(|c| c.to_string())
        .collect::<Vec<_>>()
        .join(" "));
    println!();

    // =========================================================================
    // phase 6: evaluate hands
    // =========================================================================
    println!("phase 6: evaluate hands");

    let mut player_hand_ranks = Vec::new();

    for (i, hole) in player_hole_cards.iter().enumerate() {
        let mut all_cards = hole.clone();
        all_cards.extend(community_cards.iter().cloned());

        let hand_rank = evaluate_best_hand(&all_cards).expect("7 cards should make a hand");
        println!("  player {}: {:?}", i, hand_rank);
        player_hand_ranks.push(hand_rank);
    }
    println!();

    // =========================================================================
    // phase 7: determine winners
    // =========================================================================
    println!("phase 7: determine winners");

    let winners = determine_winners(&player_hand_ranks);
    println!("  winner(s): {:?}\n", winners);

    // =========================================================================
    // phase 8: audit and settlement
    // =========================================================================
    println!("phase 8: audit and settlement");

    let encrypted_deck_commitment = [1u8; 32]; // simplified
    let mut audit = RevealAudit::new_poker(
        game_id,
        1,
        NUM_PLAYERS as u8,
        encrypted_deck_commitment,
    );

    // record reveals (simplified - would have real proofs)
    for (position, ct) in final_deck.iter().enumerate().take(15) {
        let card_value = decrypt_card(ct, &secret_keys);

        let revealed = RevealedCard {
            position: position as u8,
            card: card_value,
            revealed_at: 100,
            proof: RevealProof::PlayerReveal {
                player: 0,
                share: [1u8; 32],
                signature: [2u8; 64],
            },
        };

        if let Err(e) = audit.record_reveal(revealed) {
            println!("  cheat detected: {}", e);
        }
    }

    if audit.has_cheating() {
        println!("  cheating detected!");
        for (player, cheat) in audit.get_cheats() {
            println!("    player {}: {:?}", player, cheat);
        }
    } else {
        println!("  no cheating detected");
    }

    // compute settlement
    let pot = 1000u64;
    let winners_u8: Vec<u8> = winners.iter().map(|&w| w as u8).collect();
    let settlement = audit.compute_settlement(pot, winners_u8);
    println!("  settlement: {:?}", settlement);

    // reveal root for on-chain
    let reveal_root = audit.compute_reveal_root();
    println!("  reveal root: {:?}...", &reveal_root[..8]);

    println!("\n=== demo complete ===");
}
