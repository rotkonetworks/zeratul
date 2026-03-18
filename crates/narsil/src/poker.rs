//! poker dispute resolution
//!
//! when players disagree on a hand outcome, either submits the co-signed
//! action log to the narsil jury. the jury replays every action through a
//! deterministic game engine and produces a verdict.
//!
//! the action log is self-authenticating: each action carries both players'
//! signatures. if the submitter fabricated or omitted actions, the other
//! player holds the real log as a fraud proof.
//!
//! the jury doesn't need to trust either player — it only trusts the
//! co-signed actions and the deterministic engine.

use sha2::{Sha256, Digest};
use crate::escrow::{JuryVerdict, DisputeLoser};
#[allow(unused_imports)]
use crate::escrow::ChannelState;

// -------------------------------------------------------------------------
// types (mirrors poker_p2p::protocol without the dependency)
// -------------------------------------------------------------------------

/// a single poker action
#[derive(Clone, Debug)]
pub struct Action {
    pub hand_number: u64,
    pub seat: u8,
    pub action_type: ActionKind,
    pub sequence: u64,
}

/// action kinds (mirrors poker_p2p::protocol::ActionType)
#[derive(Clone, Debug)]
pub enum ActionKind {
    Fold,
    Check,
    Call,
    Bet(u64),
    Raise(u64),
    AllIn,
}

/// co-signed action — both players attested to this action happening
#[derive(Clone, Debug)]
pub struct CoSignedAction {
    pub action: Action,
    /// signature from the acting player
    pub actor_sig: [u8; 64],
    /// counter-signature from the other player
    pub witness_sig: [u8; 64],
}

/// the full evidence submitted for a dispute
#[derive(Clone, Debug)]
pub struct DisputeEvidence {
    /// table configuration
    pub table_rules: TableRulesCompact,
    /// last co-signed state before the disputed hand
    /// (nonce, balances — both players signed this)
    pub last_agreed_nonce: u64,
    /// starting stacks for the disputed hand (from last agreed state)
    pub starting_stacks: Vec<u64>,
    /// button position
    pub button: u8,
    /// the shuffled deck (card indices 0..51)
    pub deck: Vec<u8>,
    /// deck commitment (hash of deck + randomness from both players)
    pub deck_commitment: [u8; 32],
    /// the co-signed action log for the disputed hand only
    pub actions: Vec<CoSignedAction>,
    /// hand number being disputed
    pub hand_number: u64,
    /// player A public key
    pub player_a: [u8; 32],
    /// player B public key
    pub player_b: [u8; 32],
    /// player A's claimed final balance (after this hand)
    pub claimed_payout_a: u64,
    /// player B's claimed final balance (after this hand)
    pub claimed_payout_b: u64,
}

/// compact table rules (no SCALE dependency)
#[derive(Clone, Debug)]
pub struct TableRulesCompact {
    pub small_blind: u64,
    pub big_blind: u64,
    pub ante: u64,
}

/// result of replaying a hand through the deterministic engine
#[derive(Clone, Debug)]
pub struct ReplayResult {
    /// final stacks after the hand
    pub final_stacks: Vec<u64>,
    /// total pot
    pub pot_total: u64,
    /// did the replay complete without errors
    pub valid: bool,
    /// error message if replay failed
    pub error: Option<String>,
}

// -------------------------------------------------------------------------
// dispute verification
// -------------------------------------------------------------------------

/// verify a dispute by replaying the action log
///
/// this is the core function called by jury nodes. it:
/// 1. verifies the action log signatures (both players signed each action)
/// 2. replays actions through a deterministic engine
/// 3. compares the engine's result with each player's claim
/// 4. produces a verdict with correct payouts
///
/// the jury node calls this, then signs the resulting settlement
/// transaction with its FROST share.
pub fn verify_dispute(
    evidence: &DisputeEvidence,
    verify_sigs: impl Fn(&[u8; 32], &[u8], &[u8; 64]) -> bool,
) -> Result<JuryVerdict, DisputeError> {
    // step 1: verify all co-signatures
    for (i, cosigned) in evidence.actions.iter().enumerate() {
        let action_bytes = serialize_action(&cosigned.action);

        // acting player signed it
        let actor_key = if cosigned.action.seat == 0 {
            &evidence.player_a
        } else {
            &evidence.player_b
        };
        if !verify_sigs(actor_key, &action_bytes, &cosigned.actor_sig) {
            return Err(DisputeError::InvalidActorSignature { action_index: i });
        }

        // other player witnessed it
        let witness_key = if cosigned.action.seat == 0 {
            &evidence.player_b
        } else {
            &evidence.player_a
        };
        if !verify_sigs(witness_key, &action_bytes, &cosigned.witness_sig) {
            return Err(DisputeError::InvalidWitnessSignature { action_index: i });
        }
    }

    // step 2: verify deck commitment
    let deck_hash = hash_deck(&evidence.deck);
    if deck_hash != evidence.deck_commitment {
        return Err(DisputeError::DeckCommitmentMismatch);
    }

    // step 3: replay through engine
    let replay = replay_hand(evidence)?;
    if !replay.valid {
        return Err(DisputeError::ReplayFailed {
            reason: replay.error.unwrap_or_default(),
        });
    }

    // step 4: determine correct payouts and who's wrong
    let correct_a = replay.final_stacks[0];
    let correct_b = replay.final_stacks[1];

    let a_lied = evidence.claimed_payout_a != correct_a;
    let b_lied = evidence.claimed_payout_b != correct_b;

    // the player who lied (or lied more) pays the jury deposit
    let deposit_loser = match (a_lied, b_lied) {
        (true, false) => DisputeLoser::PlayerA,
        (false, true) => DisputeLoser::PlayerB,
        (true, true) => {
            // both wrong — the bigger liar pays
            let a_diff = (evidence.claimed_payout_a as i64 - correct_a as i64).unsigned_abs();
            let b_diff = (evidence.claimed_payout_b as i64 - correct_b as i64).unsigned_abs();
            if a_diff >= b_diff { DisputeLoser::PlayerA } else { DisputeLoser::PlayerB }
        }
        // both correct — shouldn't be a dispute, but B pays as submitter
        (false, false) => DisputeLoser::PlayerB,
    };

    let action_log_hash = hash_action_log(&evidence.actions);

    Ok(JuryVerdict {
        hand_number: evidence.hand_number,
        last_agreed_nonce: evidence.last_agreed_nonce,
        correct_balances: vec![correct_a, correct_b],
        deposit_loser: Some(deposit_loser),
        action_log_hash,
    })
}

/// replay a hand from dispute evidence using deterministic rules
///
/// this is a minimal replay engine that doesn't depend on poker_p2p.
/// it processes the action log and computes final stacks.
fn replay_hand(evidence: &DisputeEvidence) -> Result<ReplayResult, DisputeError> {
    if evidence.starting_stacks.len() < 2 {
        return Err(DisputeError::InvalidEvidence("need at least 2 players".into()));
    }

    let mut stacks = evidence.starting_stacks.clone();
    let mut pot: u64 = 0;
    let mut current_bets = vec![0u64; stacks.len()];
    let mut folded = vec![false; stacks.len()];

    let sb = evidence.table_rules.small_blind;
    let bb = evidence.table_rules.big_blind;

    // post blinds (heads-up: button=SB, other=BB)
    let sb_seat = evidence.button as usize;
    let bb_seat = 1 - sb_seat;

    let sb_amount = sb.min(stacks[sb_seat]);
    stacks[sb_seat] -= sb_amount;
    current_bets[sb_seat] = sb_amount;
    pot += sb_amount;

    let bb_amount = bb.min(stacks[bb_seat]);
    stacks[bb_seat] -= bb_amount;
    current_bets[bb_seat] = bb_amount;
    pot += bb_amount;

    // replay actions
    for cosigned in &evidence.actions {
        let seat = cosigned.action.seat as usize;
        if seat >= stacks.len() {
            return Ok(ReplayResult {
                final_stacks: stacks,
                pot_total: pot,
                valid: false,
                error: Some(format!("invalid seat {}", seat)),
            });
        }
        if folded[seat] {
            return Ok(ReplayResult {
                final_stacks: stacks,
                pot_total: pot,
                valid: false,
                error: Some(format!("seat {} already folded", seat)),
            });
        }

        match &cosigned.action.action_type {
            ActionKind::Fold => {
                folded[seat] = true;
            }
            ActionKind::Check => {
                // valid only if no outstanding bet
            }
            ActionKind::Call => {
                let to_call = current_bets.iter().max().copied().unwrap_or(0) - current_bets[seat];
                let amount = to_call.min(stacks[seat]);
                stacks[seat] -= amount;
                current_bets[seat] += amount;
                pot += amount;
            }
            ActionKind::Bet(amount) | ActionKind::Raise(amount) => {
                let amount = *amount;
                let actual = amount.min(stacks[seat]);
                stacks[seat] -= actual;
                current_bets[seat] += actual;
                pot += actual;
            }
            ActionKind::AllIn => {
                let amount = stacks[seat];
                current_bets[seat] += amount;
                pot += amount;
                stacks[seat] = 0;
            }
        }
    }

    // determine winner
    let active: Vec<usize> = (0..stacks.len()).filter(|&i| !folded[i]).collect();

    if active.len() == 1 {
        // everyone else folded
        stacks[active[0]] += pot;
    } else {
        // showdown — would need card evaluation here
        // for now: split pot among active players (the real engine uses
        // zk_shuffle::poker::showdown_holdem for this)
        let share = pot / active.len() as u64;
        let remainder = pot % active.len() as u64;
        for (i, &seat) in active.iter().enumerate() {
            stacks[seat] += share + if i == 0 { remainder } else { 0 };
        }
    }

    Ok(ReplayResult {
        final_stacks: stacks,
        pot_total: pot,
        valid: true,
        error: None,
    })
}

// -------------------------------------------------------------------------
// helpers
// -------------------------------------------------------------------------

fn serialize_action(action: &Action) -> Vec<u8> {
    let mut buf = Vec::with_capacity(32);
    buf.extend_from_slice(&action.hand_number.to_le_bytes());
    buf.push(action.seat);
    match &action.action_type {
        ActionKind::Fold => buf.push(0),
        ActionKind::Check => buf.push(1),
        ActionKind::Call => buf.push(2),
        ActionKind::Bet(v) => { buf.push(3); buf.extend_from_slice(&v.to_le_bytes()); }
        ActionKind::Raise(v) => { buf.push(4); buf.extend_from_slice(&v.to_le_bytes()); }
        ActionKind::AllIn => buf.push(5),
    }
    buf.extend_from_slice(&action.sequence.to_le_bytes());
    buf
}

fn hash_deck(deck: &[u8]) -> [u8; 32] {
    let mut hasher = Sha256::new();
    hasher.update(b"zk.poker/deck/v1");
    hasher.update(deck);
    hasher.finalize().into()
}

fn hash_action_log(actions: &[CoSignedAction]) -> [u8; 32] {
    let mut hasher = Sha256::new();
    hasher.update(b"zk.poker/actions/v1");
    for a in actions {
        hasher.update(&serialize_action(&a.action));
    }
    hasher.finalize().into()
}

// -------------------------------------------------------------------------
// errors
// -------------------------------------------------------------------------

#[derive(Clone, Debug)]
pub enum DisputeError {
    InvalidActorSignature { action_index: usize },
    InvalidWitnessSignature { action_index: usize },
    DeckCommitmentMismatch,
    ReplayFailed { reason: String },
    InvalidEvidence(String),
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_action(seat: u8, seq: u64, kind: ActionKind) -> CoSignedAction {
        CoSignedAction {
            action: Action {
                hand_number: 1,
                seat,
                action_type: kind,
                sequence: seq,
            },
            actor_sig: [0xAA; 64],
            witness_sig: [0xBB; 64],
        }
    }

    fn make_evidence(actions: Vec<CoSignedAction>) -> DisputeEvidence {
        let deck: Vec<u8> = (0..52).collect();
        let deck_commitment = hash_deck(&deck);

        DisputeEvidence {
            table_rules: TableRulesCompact {
                small_blind: 5,
                big_blind: 10,
                ante: 0,
            },
            last_agreed_nonce: 0,
            starting_stacks: vec![1000, 1000],
            button: 0,
            deck,
            deck_commitment,
            actions,
            hand_number: 1,
            player_a: [1u8; 32],
            player_b: [2u8; 32],
            claimed_payout_a: 0,
            claimed_payout_b: 0,
        }
    }

    /// always-true signature verifier for testing
    fn accept_all(_key: &[u8; 32], _msg: &[u8], _sig: &[u8; 64]) -> bool {
        true
    }

    #[test]
    fn test_fold_gives_pot_to_other() {
        // player A (seat 0) folds preflop → player B wins blinds
        let actions = vec![
            make_action(0, 1, ActionKind::Fold),
        ];

        let mut evidence = make_evidence(actions);
        // A posted SB=5, B posted BB=10. A folds. B wins pot=15.
        evidence.claimed_payout_a = 995;  // correct: 1000 - 5
        evidence.claimed_payout_b = 1005; // correct: 1000 - 10 + 15

        let verdict = verify_dispute(&evidence, accept_all).unwrap();
        assert_eq!(verdict.correct_balances, vec![995, 1005]);
    }

    #[test]
    fn test_call_and_fold() {
        // preflop: A (SB) calls BB, B checks. flop: B bets 20, A folds.
        let actions = vec![
            make_action(0, 1, ActionKind::Call),   // A calls BB (pays 5 more)
            make_action(1, 1, ActionKind::Check),  // B checks
            make_action(1, 2, ActionKind::Bet(20)), // B bets 20
            make_action(0, 2, ActionKind::Fold),   // A folds
        ];

        let evidence = make_evidence(actions);
        let verdict = verify_dispute(&evidence, accept_all).unwrap();

        // A: 1000 - 10 (called to BB) = 990
        // pot = SB(5→10 call) + BB(10) + bet(20) = 40. A folded.
        // B wins pot=40. B: 1000 - 10 - 20 + 40 = 1010. A: 1000 - 10 = 990.
        assert_eq!(verdict.correct_balances, vec![990, 1010]);
    }

    #[test]
    fn test_invalid_signature_rejected() {
        let actions = vec![make_action(0, 1, ActionKind::Fold)];
        let evidence = make_evidence(actions);

        fn reject_all(_key: &[u8; 32], _msg: &[u8], _sig: &[u8; 64]) -> bool {
            false
        }

        let err = verify_dispute(&evidence, reject_all).unwrap_err();
        assert!(matches!(err, DisputeError::InvalidActorSignature { action_index: 0 }));
    }

    #[test]
    fn test_wrong_deck_commitment() {
        let actions = vec![make_action(0, 1, ActionKind::Fold)];
        let mut evidence = make_evidence(actions);
        evidence.deck_commitment = [0xFF; 32]; // wrong

        let err = verify_dispute(&evidence, accept_all).unwrap_err();
        assert!(matches!(err, DisputeError::DeckCommitmentMismatch));
    }

    #[test]
    fn test_liar_detected() {
        let actions = vec![make_action(0, 1, ActionKind::Fold)];
        let mut evidence = make_evidence(actions);
        // A claims they won (lie — they folded)
        evidence.claimed_payout_a = 1015;
        evidence.claimed_payout_b = 985;

        let verdict = verify_dispute(&evidence, accept_all).unwrap();
        // engine says A gets 995, B gets 1005 — A lied about winning
        assert_eq!(verdict.deposit_loser, Some(DisputeLoser::PlayerA));
    }
}
