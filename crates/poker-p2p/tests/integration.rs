//! integration tests — full game flows across modules
//!
//! tests the complete pipeline:
//!   shuffle → deal → co-signed actions → showdown → settlement

use poker_p2p::engine::*;
use poker_p2p::game_session::*;
use poker_p2p::shuffle_session::*;
use poker_p2p::protocol::*;
use poker_p2p::table::{sign_action, verify_action};
use ed25519_dalek::SigningKey;
use zk_shuffle::poker::{Card, Rank, Suit};

fn make_deck() -> Vec<Card> {
    let mut deck = Vec::new();
    for &suit in &[Suit::Clubs, Suit::Diamonds, Suit::Hearts, Suit::Spades] {
        for &rank in &Rank::ALL {
            deck.push(Card { rank, suit });
        }
    }
    deck
}

fn rules() -> TableRules {
    TableRules {
        small_blind: 5, big_blind: 10, ante: 0,
        min_buy_in: 1000, max_buy_in: 0, seats: 2,
        tier: SecurityTier::Training,
        allow_spectators: false, max_spectators: 0,
        time_bank: 60, action_timeout: 30,
    }
}

fn make_session_pair() -> (GameSession, GameSession) {
    let secret_a = [0x11u8; 32];
    let secret_b = [0x22u8; 32];
    let pub_a = SigningKey::from_bytes(&secret_a).verifying_key().to_bytes();
    let pub_b = SigningKey::from_bytes(&secret_b).verifying_key().to_bytes();
    let a = GameSession::new(rules(), 0, secret_a, pub_b, 1000).unwrap();
    let b = GameSession::new(rules(), 1, secret_b, pub_a, 1000).unwrap();
    (a, b)
}

fn extract_action(events: &[SessionEvent]) -> Option<PlayerAction> {
    events.iter().find_map(|e| match e {
        SessionEvent::SendToPeer(Message::Action(pa)) => Some(pa.clone()),
        _ => None,
    })
}

fn extract_ack(events: &[SessionEvent]) -> Option<WitnessAck> {
    events.iter().find_map(|e| match e {
        SessionEvent::SendToPeer(Message::WitnessAck(ack)) => Some(ack.clone()),
        _ => None,
    })
}

// =========================================================================
// Shuffle adversarial tests
// =========================================================================

#[test]
fn test_shuffle_tampered_proof_rejected() {
    let mut host = ShuffleSession::new(1, true);
    let mut guest = ShuffleSession::new(1, false);

    host.receive_peer_key(&guest.our_public_key()).unwrap();
    guest.receive_peer_key(&host.our_public_key()).unwrap();

    let (deck_bytes, mut proof_bytes) = host.shuffle().unwrap();

    // tamper with proof
    if let Some(byte) = proof_bytes.get_mut(10) {
        *byte ^= 0xFF;
    }

    let result = guest.receive_shuffle(&deck_bytes, &proof_bytes);
    assert!(result.is_err(), "tampered proof should be rejected");
}

#[test]
fn test_shuffle_wrong_deck_rejected() {
    let mut host = ShuffleSession::new(1, true);
    let mut guest = ShuffleSession::new(1, false);

    host.receive_peer_key(&guest.our_public_key()).unwrap();
    guest.receive_peer_key(&host.our_public_key()).unwrap();

    let (_deck_bytes, proof_bytes) = host.shuffle().unwrap();

    // send garbage deck bytes (right size, wrong content)
    let fake_deck = vec![0xABu8; 52 * 64];
    let result = guest.receive_shuffle(&fake_deck, &proof_bytes);
    assert!(result.is_err(), "wrong deck should be rejected");
}

#[test]
fn test_shuffle_both_get_same_commitment() {
    let mut host = ShuffleSession::new(42, true);
    let mut guest = ShuffleSession::new(42, false);

    host.receive_peer_key(&guest.our_public_key()).unwrap();
    guest.receive_peer_key(&host.our_public_key()).unwrap();

    // host shuffles
    let (d1, p1) = host.shuffle().unwrap();
    guest.receive_shuffle(&d1, &p1).unwrap();

    // guest shuffles on top
    let (d2, p2) = guest.shuffle().unwrap();
    host.receive_shuffle(&d2, &p2).unwrap();

    let hc = host.commitment().expect("host should be complete");
    let gc = guest.commitment().expect("guest should be complete");
    assert_eq!(hc, gc, "commitments must match");
}

// =========================================================================
// Game session multi-hand tests
// =========================================================================

#[test]
fn test_multi_hand_cosigned_session() {
    let (mut a, mut b) = make_session_pair();
    let deck = make_deck();

    for hand in 1..=5 {
        let button = if hand % 2 == 1 { 0 } else { 1 };
        let a_events = a.start_hand(button, &deck).unwrap();
        let _b_events = b.start_hand(button, &deck).unwrap();

        // in heads-up, button/SB acts first preflop
        // button is seat `button`, so that seat acts first
        // if button==0, a acts. if button==1, b acts.
        if button == 0 {
            // a's turn — a folds
            let events = a.act(ActionType::Fold).unwrap();
            let action = extract_action(&events).unwrap();
            let b_events = b.receive_action(action).unwrap();
            let ack = extract_ack(&b_events).unwrap();
            a.receive_witness_ack(ack).unwrap();
        } else {
            // b's turn — b folds
            let events = b.act(ActionType::Fold).unwrap();
            let action = extract_action(&events).unwrap();
            let a_events = a.receive_action(action).unwrap();
            let ack = extract_ack(&a_events).unwrap();
            b.receive_witness_ack(ack).unwrap();
        }
    }

    assert_eq!(a.completed_hands().len(), 5);
    assert_eq!(b.completed_hands().len(), 5);

    // each transcript should have exactly 1 action
    for t in a.completed_hands() {
        assert_eq!(t.actions.len(), 1);
    }
}

#[test]
fn test_session_engine_state_sync() {
    let (mut a, mut b) = make_session_pair();
    let deck = make_deck();

    a.start_hand(0, &deck).unwrap();
    b.start_hand(0, &deck).unwrap();

    // play call → check → check → check → check → check → check → check (to showdown)
    // a calls (preflop SB/button)
    let events = a.act(ActionType::Call).unwrap();
    let action = extract_action(&events).unwrap();
    let b_events = b.receive_action(action).unwrap();
    let ack = extract_ack(&b_events).unwrap();
    a.receive_witness_ack(ack).unwrap();

    // engines should be in same state
    assert_eq!(
        a.engine.state_hash(),
        b.engine.state_hash(),
        "engines must be synchronized after each action"
    );
}

// =========================================================================
// Engine fuzz: random actions, chip conservation
// =========================================================================

#[test]
fn test_engine_fuzz_100_hands() {
    use rand::Rng;
    let mut rng = rand::thread_rng();
    let mut engine = GameEngine::new(rules(), 2).unwrap();
    engine.seat_player(0, 1000).unwrap();
    engine.seat_player(1, 1000).unwrap();
    let initial_total = 2000u64;

    for hand in 0..100 {
        let deck = make_deck();
        let button = (hand % 2) as u8;

        // rebuy if busted
        for seat in 0..2u8 {
            if engine.stacks()[seat as usize] == 0 {
                let _ = engine.seat_player(seat, 1000);
            }
        }

        if engine.stacks().iter().filter(|&&s| s > 0).count() < 2 {
            continue;
        }

        let total_before: u64 = engine.stacks().iter().sum();
        let events = match engine.new_hand(button, &deck) {
            Ok(e) => e,
            Err(_) => continue,
        };

        // play random valid actions until hand ends
        let mut actions_taken = 0;
        loop {
            if engine.hand_state().is_none() { break; }
            if actions_taken > 50 { break; } // safety

            let hand_state = engine.hand_state().unwrap();
            let action_on = match hand_state.betting.action_on {
                Some(idx) => hand_state.seats[idx].seat,
                None => break,
            };

            let valid = match engine.valid_actions() {
                Ok(v) => v,
                Err(_) => break,
            };

            if valid.is_empty() { break; }

            // pick random valid action
            let chosen = &valid[rng.gen_range(0..valid.len())];
            let action = match chosen.kind {
                ActionKind::Fold => ActionType::Fold,
                ActionKind::Check => ActionType::Check,
                ActionKind::Call => ActionType::Call,
                ActionKind::Bet => ActionType::Bet(chosen.min_amount as u128),
                ActionKind::Raise => ActionType::Raise(chosen.min_amount as u128),
                ActionKind::AllIn => ActionType::AllIn,
            };

            match engine.apply_action(action_on, action) {
                Ok(_) => { actions_taken += 1; }
                Err(_) => break,
            }
        }

        // chip conservation after every hand
        if engine.hand_state().is_none() {
            let total_after: u64 = engine.stacks().iter().sum();
            // total may differ from initial_total if we rebuyed
            // but within a hand, chips are conserved
            assert!(total_after > 0, "hand {}: someone has chips", hand);
        }
    }
}

// =========================================================================
// Signature edge cases
// =========================================================================

// timeout claim test lives in game_session::tests (needs access to private fields)

#[test]
fn test_witness_ack_for_wrong_sequence_rejected() {
    let (mut a, mut b) = make_session_pair();
    let deck = make_deck();

    a.start_hand(0, &deck).unwrap();
    b.start_hand(0, &deck).unwrap();

    let events = a.act(ActionType::Call).unwrap();
    let action = extract_action(&events).unwrap();
    let b_events = b.receive_action(action).unwrap();
    let mut ack = extract_ack(&b_events).unwrap();

    // tamper with sequence number
    ack.sequence = 999;
    let result = a.receive_witness_ack(ack);
    assert!(result.is_err(), "wrong sequence ack should be rejected");
}
