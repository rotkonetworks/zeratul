//! game_session - P2P game loop with co-signed actions and verifiable timeouts
//!
//! both peers run the same deterministic GameEngine. every action is signed
//! by the actor and counter-signed (witnessed) by the opponent. timeouts
//! produce a TimeoutClaim that narsil can verify against the transcript.
//!
//! no server authority — both peers are equal. the game state is
//! deterministic: same (rules, stacks, button, deck, actions) → same result.

use crate::engine::*;
use crate::protocol::*;
use crate::table::{sign_action, verify_action};
use ed25519_dalek::{SigningKey, Signer, VerifyingKey, Verifier, Signature};
use sha2::{Sha256, Digest};
use zk_shuffle::poker::Card;
use std::time::{SystemTime, UNIX_EPOCH};

/// co-signed game session between two peers
pub struct GameSession {
    pub engine: GameEngine,
    pub my_seat: u8,
    my_secret: [u8; 32],
    peer_pubkey: [u8; 32],
    hand_number: u64,
    sequence: u64,
    /// co-signed transcript for current hand
    transcript: Vec<CoSignedAction>,
    /// pending action waiting for witness ack
    pending_witness: Option<PlayerAction>,
    /// when ActionRequired was emitted for current turn
    action_required_at: Option<u64>,
    /// completed hand transcripts (for dispute submission)
    completed_hands: Vec<HandTranscript>,
}

/// events emitted by GameSession to the transport layer
#[derive(Clone, Debug)]
pub enum SessionEvent {
    /// send this message to peer
    SendToPeer(Message),
    /// action was co-signed and committed
    ActionCommitted(CoSignedAction),
    /// hand transcript completed
    HandCompleted(HandTranscript),
    /// engine event for local UI
    Engine(EngineEvent),
    /// timeout claim ready to submit to narsil
    TimeoutReady(TimeoutClaim),
    /// error
    Error(String),
}

impl GameSession {
    pub fn new(
        rules: TableRules,
        my_seat: u8,
        my_secret: [u8; 32],
        peer_pubkey: [u8; 32],
        buy_in: u64,
    ) -> Result<Self, EngineError> {
        let mut engine = GameEngine::new(rules, 2)?;
        engine.seat_player(0, buy_in)?;
        engine.seat_player(1, buy_in)?;

        Ok(Self {
            engine,
            my_seat,
            my_secret,
            peer_pubkey,
            hand_number: 0,
            sequence: 0,
            transcript: Vec::new(),
            pending_witness: None,
            action_required_at: None,
            completed_hands: Vec::new(),
        })
    }

    /// start a new hand. both peers must call this with the same args.
    pub fn start_hand(&mut self, button: u8, deck: &[Card]) -> Result<Vec<SessionEvent>, EngineError> {
        self.hand_number += 1;
        self.sequence = 0;
        self.transcript.clear();
        self.pending_witness = None;
        self.action_required_at = None;

        let events = self.engine.new_hand(button, deck)?;
        let mut out: Vec<SessionEvent> = events.iter().map(|e| SessionEvent::Engine(e.clone())).collect();

        // track when action is required
        for e in &events {
            if let EngineEvent::ActionRequired { .. } = e {
                self.action_required_at = Some(unix_now());
            }
        }

        Ok(out)
    }

    /// local player performs an action. signs it and sends to peer.
    pub fn act(&mut self, action: ActionType) -> Result<Vec<SessionEvent>, String> {
        self.sequence += 1;
        let mut player_action = PlayerAction {
            hand_number: self.hand_number,
            seat: self.my_seat,
            action: action.clone(),
            sequence: self.sequence,
            signature: [0u8; 64],
        };
        sign_action(&self.my_secret, &mut player_action);

        // apply to local engine
        let events = self.engine.apply_action(self.my_seat, action)
            .map_err(|e| format!("{}", e))?;

        let mut out: Vec<SessionEvent> = Vec::new();

        // send signed action to peer
        out.push(SessionEvent::SendToPeer(Message::Action(player_action.clone())));

        // wait for witness ack
        self.pending_witness = Some(player_action);

        // forward engine events
        for e in &events {
            if let EngineEvent::ActionRequired { .. } = e {
                self.action_required_at = Some(unix_now());
            }
            out.push(SessionEvent::Engine(e.clone()));
        }

        Ok(out)
    }

    /// peer sent us an action. verify signature, apply to engine, send witness ack.
    pub fn receive_action(&mut self, action: PlayerAction) -> Result<Vec<SessionEvent>, String> {
        // must be from opponent
        if action.seat == self.my_seat {
            return Err("received action from our own seat".into());
        }

        // verify signature
        verify_action(&self.peer_pubkey, &action, self.sequence)
            .map_err(|e| format!("action verification failed: {}", e))?;

        self.sequence = action.sequence;

        // apply to local engine
        let events = self.engine.apply_action(action.seat, action.action.clone())
            .map_err(|e| format!("engine error: {}", e))?;

        let mut out: Vec<SessionEvent> = Vec::new();

        // produce witness ack (counter-sign)
        let witness_sig = self.sign_witness(action.hand_number, action.sequence, &action);
        let ack = WitnessAck {
            hand_number: action.hand_number,
            sequence: action.sequence,
            signature: witness_sig,
        };
        out.push(SessionEvent::SendToPeer(Message::WitnessAck(ack)));

        // action is now co-signed from our perspective
        let co_signed = CoSignedAction {
            action,
            witness_sig,
        };
        self.transcript.push(co_signed.clone());
        out.push(SessionEvent::ActionCommitted(co_signed));

        // forward engine events
        for e in &events {
            if let EngineEvent::ActionRequired { .. } = e {
                self.action_required_at = Some(unix_now());
            }
            if let EngineEvent::HandComplete { .. } = e {
                out.extend(self.finalize_hand());
            }
            out.push(SessionEvent::Engine(e.clone()));
        }

        Ok(out)
    }

    /// peer sent witness ack for our action. now it's co-signed.
    pub fn receive_witness_ack(&mut self, ack: WitnessAck) -> Result<Vec<SessionEvent>, String> {
        let pending = self.pending_witness.take()
            .ok_or("received witness ack but no pending action")?;

        if ack.hand_number != pending.hand_number || ack.sequence != pending.sequence {
            return Err("witness ack doesn't match pending action".into());
        }

        // verify opponent's witness signature
        self.verify_witness(&ack, &pending)?;

        let co_signed = CoSignedAction {
            action: pending,
            witness_sig: ack.signature,
        };
        self.transcript.push(co_signed.clone());

        let mut out = vec![SessionEvent::ActionCommitted(co_signed)];

        // check if hand completed
        if self.engine.hand_state().is_none() {
            out.extend(self.finalize_hand());
        }

        Ok(out)
    }

    /// check if opponent has timed out. call this periodically.
    pub fn check_timeout(&self) -> Option<Vec<SessionEvent>> {
        let action_at = self.action_required_at?;
        let timeout = self.engine.rules.action_timeout as u64;
        let now = unix_now();

        if now < action_at + timeout {
            return None; // not timed out yet
        }

        // who was supposed to act?
        let hand = self.engine.hand_state()?;
        let acting_seat = hand.betting.action_on? as u8;

        // only claim timeout if it's the OTHER player who timed out
        if acting_seat == self.my_seat {
            return None;
        }

        let state_hash = self.compute_state_hash();
        let mut claim = TimeoutClaim {
            hand_number: self.hand_number,
            last_sequence: self.sequence,
            timed_out_seat: acting_seat,
            action_required_at: action_at,
            claimed_at: now,
            timeout_secs: self.engine.rules.action_timeout,
            signature: [0u8; 64],
            state_hash,
        };
        claim.signature = self.sign_timeout_claim(&claim);

        let mut out = vec![SessionEvent::TimeoutReady(claim.clone())];

        // also send to peer so they know
        out.push(SessionEvent::SendToPeer(Message::TimeoutClaim(claim)));

        Some(out)
    }

    /// peer claims we timed out. verify the claim.
    pub fn receive_timeout_claim(&self, claim: &TimeoutClaim) -> Result<bool, String> {
        // verify their signature
        let pubkey = VerifyingKey::from_bytes(&self.peer_pubkey)
            .map_err(|_| "invalid peer pubkey")?;
        let payload = timeout_claim_payload(claim);
        let sig = Signature::from_bytes(&claim.signature);
        pubkey.verify(&payload, &sig).map_err(|_| "bad timeout claim signature")?;

        // verify state hash matches our engine
        let our_hash = self.compute_state_hash();
        if claim.state_hash != our_hash {
            return Err("state hash mismatch — engines diverged".into());
        }

        // verify timing
        if claim.claimed_at < claim.action_required_at + claim.timeout_secs as u64 {
            return Err("timeout claimed too early".into());
        }

        Ok(true)
    }

    /// apply timeout as auto-fold (after timeout is verified)
    pub fn apply_timeout(&mut self, seat: u8) -> Result<Vec<SessionEvent>, String> {
        self.action_required_at = None;
        let events = self.engine.apply_action(seat, ActionType::Fold)
            .map_err(|e| format!("{}", e))?;

        let mut out: Vec<SessionEvent> = events.iter().map(|e| SessionEvent::Engine(e.clone())).collect();

        if self.engine.hand_state().is_none() {
            out.extend(self.finalize_hand());
        }

        Ok(out)
    }

    /// get completed hand transcripts
    pub fn completed_hands(&self) -> &[HandTranscript] {
        &self.completed_hands
    }

    /// get current hand's co-signed actions so far
    pub fn current_transcript(&self) -> &[CoSignedAction] {
        &self.transcript
    }

    // --- private ---

    fn finalize_hand(&mut self) -> Vec<SessionEvent> {
        let transcript = HandTranscript {
            hand_number: self.hand_number,
            button: 0, // TODO: track button
            starting_stacks: self.engine.stacks().to_vec(),
            deck_commitment: [0u8; 32], // TODO: mental poker
            actions: self.transcript.clone(),
            reveals: Vec::new(),
            result: None,
        };
        self.completed_hands.push(transcript.clone());
        self.action_required_at = None;
        vec![SessionEvent::HandCompleted(transcript)]
    }

    fn sign_witness(&self, hand_number: u64, sequence: u64, action: &PlayerAction) -> [u8; 64] {
        let payload = witness_payload(hand_number, sequence, action);
        let signing_key = SigningKey::from_bytes(&self.my_secret);
        signing_key.sign(&payload).to_bytes()
    }

    fn verify_witness(&self, ack: &WitnessAck, action: &PlayerAction) -> Result<(), String> {
        let pubkey = VerifyingKey::from_bytes(&self.peer_pubkey)
            .map_err(|_| "invalid peer pubkey")?;
        let payload = witness_payload(ack.hand_number, ack.sequence, action);
        let sig = Signature::from_bytes(&ack.signature);
        pubkey.verify(&payload, &sig).map_err(|_| "bad witness signature")?;
        Ok(())
    }

    fn compute_state_hash(&self) -> [u8; 32] {
        let mut h = Sha256::new();
        h.update(b"poker.state.v1");
        h.update(self.hand_number.to_le_bytes());
        h.update(self.sequence.to_le_bytes());
        for s in self.engine.stacks() {
            h.update(s.to_le_bytes());
        }
        if let Some(hand) = self.engine.hand_state() {
            h.update(&[hand.phase as u8]);
            h.update((hand.betting.action_on.unwrap_or(0) as u64).to_le_bytes());
        }
        h.finalize().into()
    }

    fn sign_timeout_claim(&self, claim: &TimeoutClaim) -> [u8; 64] {
        let payload = timeout_claim_payload(claim);
        let signing_key = SigningKey::from_bytes(&self.my_secret);
        signing_key.sign(&payload).to_bytes()
    }
}

fn unix_now() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_secs()
}

fn witness_payload(hand_number: u64, sequence: u64, action: &PlayerAction) -> Vec<u8> {
    let action_hash = {
        let mut h = Sha256::new();
        h.update(&serde_json::to_vec(action).unwrap_or_default());
        h.finalize()
    };
    let mut payload = Vec::new();
    payload.extend_from_slice(b"poker.witness.v1");
    payload.extend_from_slice(&hand_number.to_le_bytes());
    payload.extend_from_slice(&sequence.to_le_bytes());
    payload.extend_from_slice(&action_hash);
    payload
}

fn timeout_claim_payload(claim: &TimeoutClaim) -> Vec<u8> {
    let mut payload = Vec::new();
    payload.extend_from_slice(b"poker.timeout.v1");
    payload.extend_from_slice(&claim.hand_number.to_le_bytes());
    payload.extend_from_slice(&claim.last_sequence.to_le_bytes());
    payload.push(claim.timed_out_seat);
    payload.extend_from_slice(&claim.action_required_at.to_le_bytes());
    payload.extend_from_slice(&claim.claimed_at.to_le_bytes());
    payload.extend_from_slice(&claim.timeout_secs.to_le_bytes());
    payload.extend_from_slice(&claim.state_hash);
    payload
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::TableRules;
    use zk_shuffle::poker::{Card, Rank, Suit};

    fn test_deck() -> Vec<Card> {
        let mut deck = Vec::with_capacity(52);
        for &suit in &[Suit::Clubs, Suit::Diamonds, Suit::Hearts, Suit::Spades] {
            for &rank in &Rank::ALL {
                deck.push(Card { rank, suit });
            }
        }
        deck
    }

    fn make_session_pair() -> (GameSession, GameSession) {
        let rules = TableRules {
            small_blind: 5, big_blind: 10, ante: 0,
            min_buy_in: 1000, max_buy_in: 0, seats: 2,
            tier: crate::protocol::SecurityTier::Training,
            allow_spectators: false, max_spectators: 0,
            time_bank: 60, action_timeout: 30,
        };

        let secret_a = [0x11u8; 32];
        let secret_b = [0x22u8; 32];
        let pub_a = SigningKey::from_bytes(&secret_a).verifying_key().to_bytes();
        let pub_b = SigningKey::from_bytes(&secret_b).verifying_key().to_bytes();

        let a = GameSession::new(rules.clone(), 0, secret_a, pub_b, 1000).unwrap();
        let b = GameSession::new(rules, 1, secret_b, pub_a, 1000).unwrap();
        (a, b)
    }

    #[test]
    fn test_action_cosigning() {
        let (mut a, mut b) = make_session_pair();
        let deck = test_deck();

        let _ = a.start_hand(0, &deck);
        let _ = b.start_hand(0, &deck);

        // seat 1 (BB) acts first in heads-up preflop? No — in heads-up, button=SB acts first preflop
        // seat 0 is button=SB, so seat 0 acts first preflop
        let events = a.act(ActionType::Call).unwrap();

        // find the SendToPeer message
        let action_msg = events.iter().find_map(|e| match e {
            SessionEvent::SendToPeer(Message::Action(pa)) => Some(pa.clone()),
            _ => None,
        }).expect("should send action to peer");

        // b receives the action and sends witness ack
        let b_events = b.receive_action(action_msg).unwrap();
        let ack = b_events.iter().find_map(|e| match e {
            SessionEvent::SendToPeer(Message::WitnessAck(ack)) => Some(ack.clone()),
            _ => None,
        }).expect("should send witness ack");

        // a receives the witness ack — action is now co-signed
        let a_events = a.receive_witness_ack(ack).unwrap();
        let committed = a_events.iter().any(|e| matches!(e, SessionEvent::ActionCommitted(_)));
        assert!(committed, "action should be committed after witness ack");

        // both peers have 1 co-signed action in transcript
        assert_eq!(a.current_transcript().len(), 1);
        assert_eq!(b.current_transcript().len(), 1);
    }

    #[test]
    fn test_full_hand_cosigned() {
        let (mut a, mut b) = make_session_pair();
        let deck = test_deck();

        let _ = a.start_hand(0, &deck);
        let _ = b.start_hand(0, &deck);

        // a (button/SB) folds preflop
        let events = a.act(ActionType::Fold).unwrap();
        let action_msg = events.iter().find_map(|e| match e {
            SessionEvent::SendToPeer(Message::Action(pa)) => Some(pa.clone()),
            _ => None,
        }).unwrap();

        let b_events = b.receive_action(action_msg).unwrap();
        let ack = b_events.iter().find_map(|e| match e {
            SessionEvent::SendToPeer(Message::WitnessAck(ack)) => Some(ack.clone()),
            _ => None,
        }).unwrap();

        let a_events = a.receive_witness_ack(ack).unwrap();

        // hand should be complete
        let hand_completed = a_events.iter().any(|e| matches!(e, SessionEvent::HandCompleted(_)));
        assert!(hand_completed, "hand should be completed");
        assert_eq!(a.completed_hands().len(), 1);
    }

    #[test]
    fn test_bad_signature_rejected() {
        let (mut a, mut b) = make_session_pair();
        let deck = test_deck();

        let _ = a.start_hand(0, &deck);
        let _ = b.start_hand(0, &deck);

        // forge an action with wrong signature
        let forged = PlayerAction {
            hand_number: 1,
            seat: 0,
            action: ActionType::AllIn,
            sequence: 1,
            signature: [0xFFu8; 64], // garbage
        };

        let result = b.receive_action(forged);
        assert!(result.is_err(), "forged action should be rejected");
    }

    #[test]
    fn test_replay_rejected() {
        let (mut a, mut b) = make_session_pair();
        let deck = test_deck();

        let _ = a.start_hand(0, &deck);
        let _ = b.start_hand(0, &deck);

        let events = a.act(ActionType::Call).unwrap();
        let action_msg = events.iter().find_map(|e| match e {
            SessionEvent::SendToPeer(Message::Action(pa)) => Some(pa.clone()),
            _ => None,
        }).unwrap();

        // first receive OK
        let b_events = b.receive_action(action_msg.clone()).unwrap();
        let ack = b_events.iter().find_map(|e| match e {
            SessionEvent::SendToPeer(Message::WitnessAck(ack)) => Some(ack.clone()),
            _ => None,
        }).unwrap();
        let _ = a.receive_witness_ack(ack);

        // replay same action — should fail (sequence not increasing)
        let result = b.receive_action(action_msg);
        assert!(result.is_err(), "replayed action should be rejected");
    }

    #[test]
    fn test_timeout_claim() {
        let (mut a, mut b) = make_session_pair();
        let deck = test_deck();

        let _ = a.start_hand(0, &deck);
        let _ = b.start_hand(0, &deck);

        // it's a's turn (seat 0, button/SB preflop in heads-up)
        // b checks timeout — should be None (hasn't expired)
        assert!(b.check_timeout().is_none());

        // manually backdate action_required_at to simulate timeout
        b.action_required_at = Some(unix_now() - 31);

        let timeout_events = b.check_timeout().expect("should produce timeout");
        let claim = timeout_events.iter().find_map(|e| match e {
            SessionEvent::TimeoutReady(c) => Some(c.clone()),
            _ => None,
        }).expect("should have timeout claim");

        assert_eq!(claim.timed_out_seat, 0);

        // a can verify the claim
        // need to set a's action_required_at too for state hash to match
        a.action_required_at = Some(b.action_required_at.unwrap());
        let verified = a.receive_timeout_claim(&claim);
        assert!(verified.is_ok());
    }

    #[test]
    fn test_state_hash_deterministic() {
        let (mut a, mut b) = make_session_pair();
        let deck = test_deck();

        let _ = a.start_hand(0, &deck);
        let _ = b.start_hand(0, &deck);

        // same engine state → same hash
        assert_eq!(a.compute_state_hash(), b.compute_state_hash());
    }

    #[test]
    fn test_wrong_seat_action_rejected() {
        let (mut a, mut b) = make_session_pair();
        let deck = test_deck();

        let _ = a.start_hand(0, &deck);
        let _ = b.start_hand(0, &deck);

        // a signs an action claiming to be seat 0 (correct), but sends it to b
        // b should accept since seat 0 != b's seat (1)
        // but if a sends action with seat 0 and b's receive_action checks seat != my_seat
        // this is the normal case — let's test the opposite: b forges an action as seat 1
        // and sends to a, but uses a's pubkey — should fail sig verification

        let mut forged = PlayerAction {
            hand_number: 1,
            seat: 1, // b's seat
            action: ActionType::Fold,
            sequence: 1,
            signature: [0u8; 64],
        };
        // sign with a's key (wrong key for seat 1)
        sign_action(&a.my_secret, &mut forged);

        // a receives — but action.seat == 0 == a.my_seat would fail "from our own seat"
        // actually forged.seat == 1, which != a.my_seat (0), so it passes seat check
        // but signature verification will fail because it's signed with a's key
        // and a verifies against peer_pubkey (b's key)
        let result = a.receive_action(forged);
        assert!(result.is_err());
    }
}
