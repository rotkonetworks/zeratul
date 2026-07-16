//! Specification / regression tests for the signed-message domain separation and
//! settlement state-binding introduced to fix the two money-critical audit bugs:
//!
//!   BUG 1 — handle_settle had no caller auth and no state binding: an arbitrary
//!           caller could drain the pot with fabricated payouts. The fix requires
//!           all active players to sign a settlement message bound to the stored
//!           (nonce, state_hash) AND to the exact payouts distribution.
//!
//!   BUG 2 — Cross-deploy signature replay: signed-state messages lacked chain-id
//!           / contract-address binding, so a signature valid on one deployment
//!           replayed on another. The fix folds chain_id + contract address into
//!           the signed message.
//!
//! The contract itself is a `#![no_std]` / `#![no_main]` PolkaVM binary whose
//! message hashing calls the host `keccak256` precompile, so the production
//! functions cannot be linked into a normal test harness. These tests instead
//! reproduce the EXACT preimage byte layout that `src/main.rs` commits to
//! (`state_message_hash` / `settle_message_hash`) and assert the security
//! properties hold. If the layout in `main.rs` changes, these mirrors must be
//! updated in lockstep — that is the point: they pin the wire format.

use keccak_const::Keccak256;

fn keccak256(data: &[u8]) -> [u8; 32] {
    Keccak256::new().update(data).finalize()
}

/// Mirror of `state_message_hash` in src/main.rs.
/// message = "POKER_STATE"(11) || chain_id(32) || contract_addr(20)
///           || game_id(32) || nonce_be(8) || state_hash(32)
fn state_message_hash(
    chain_id: &[u8; 32],
    contract_addr: &[u8; 20],
    game_id: &[u8; 32],
    nonce: u64,
    state_hash: &[u8; 32],
) -> [u8; 32] {
    const TAG: &[u8; 11] = b"POKER_STATE";
    let mut msg = Vec::new();
    msg.extend_from_slice(TAG);
    msg.extend_from_slice(chain_id);
    msg.extend_from_slice(contract_addr);
    msg.extend_from_slice(game_id);
    msg.extend_from_slice(&nonce.to_be_bytes());
    msg.extend_from_slice(state_hash);
    assert_eq!(msg.len(), 11 + 32 + 20 + 32 + 8 + 32);
    keccak256(&msg)
}

/// Mirror of `settle_message_hash` in src/main.rs.
/// message = "POKER_SETTLE"(12) || chain_id(32) || contract_addr(20)
///           || game_id(32) || nonce_be(8) || state_hash(32) || payouts_hash(32)
fn settle_message_hash(
    chain_id: &[u8; 32],
    contract_addr: &[u8; 20],
    game_id: &[u8; 32],
    nonce: u64,
    state_hash: &[u8; 32],
    payouts_hash: &[u8; 32],
) -> [u8; 32] {
    const TAG: &[u8; 12] = b"POKER_SETTLE";
    let mut msg = Vec::new();
    msg.extend_from_slice(TAG);
    msg.extend_from_slice(chain_id);
    msg.extend_from_slice(contract_addr);
    msg.extend_from_slice(game_id);
    msg.extend_from_slice(&nonce.to_be_bytes());
    msg.extend_from_slice(state_hash);
    msg.extend_from_slice(payouts_hash);
    assert_eq!(msg.len(), 12 + 32 + 20 + 32 + 8 + 32 + 32);
    keccak256(&msg)
}

/// Mirror of the `payouts_preimage` commitment computed in `handle_settle`:
/// keccak256( len_word(32) || payout_word_0(32) || ... ), where each word is the
/// raw 32-byte ABI word (u128 right-aligned in the low 16 bytes).
fn payouts_hash(payouts: &[u128]) -> [u8; 32] {
    let mut buf = Vec::new();
    let mut len_word = [0u8; 32];
    len_word[28..32].copy_from_slice(&(payouts.len() as u32).to_be_bytes());
    buf.extend_from_slice(&len_word);
    for &p in payouts {
        let mut w = [0u8; 32];
        w[16..32].copy_from_slice(&p.to_be_bytes());
        buf.extend_from_slice(&w);
    }
    keccak256(&buf)
}

const GAME_ID: [u8; 32] = [0x11; 32];
const STATE_HASH: [u8; 32] = [0x22; 32];
const ADDR_A: [u8; 20] = [0xAA; 20];
const ADDR_B: [u8; 20] = [0xBB; 20];

fn chain(id: u64) -> [u8; 32] {
    let mut c = [0u8; 32];
    c[24..32].copy_from_slice(&id.to_be_bytes());
    c
}

// ---------------------------------------------------------------------------
// BUG 2: cross-deploy signature replay is prevented
// ---------------------------------------------------------------------------

#[test]
fn state_sig_is_bound_to_chain_id() {
    // Same game/nonce/state on two different chains must produce different
    // signing hashes, so a signature from chain 1 cannot be replayed on chain 2.
    let h1 = state_message_hash(&chain(1), &ADDR_A, &GAME_ID, 7, &STATE_HASH);
    let h2 = state_message_hash(&chain(2), &ADDR_A, &GAME_ID, 7, &STATE_HASH);
    assert_ne!(h1, h2, "state signature must differ across chain-ids");
}

#[test]
fn state_sig_is_bound_to_contract_address() {
    // Two contract instances on the SAME chain must not share signing hashes.
    let h1 = state_message_hash(&chain(1), &ADDR_A, &GAME_ID, 7, &STATE_HASH);
    let h2 = state_message_hash(&chain(1), &ADDR_B, &GAME_ID, 7, &STATE_HASH);
    assert_ne!(h1, h2, "state signature must differ across contract instances");
}

#[test]
fn settle_sig_is_bound_to_chain_id_and_address() {
    let ph = payouts_hash(&[100, 50]);
    let base = settle_message_hash(&chain(1), &ADDR_A, &GAME_ID, 7, &STATE_HASH, &ph);
    let other_chain = settle_message_hash(&chain(2), &ADDR_A, &GAME_ID, 7, &STATE_HASH, &ph);
    let other_addr = settle_message_hash(&chain(1), &ADDR_B, &GAME_ID, 7, &STATE_HASH, &ph);
    assert_ne!(base, other_chain, "settle sig must differ across chain-ids");
    assert_ne!(base, other_addr, "settle sig must differ across contract instances");
}

// ---------------------------------------------------------------------------
// BUG 1: settlement is bound to the agreed state and exact payouts
// ---------------------------------------------------------------------------

#[test]
fn settle_sig_is_bound_to_payouts() {
    // A signature authorizing payouts [100,50] must NOT authorize [150,0].
    // This is what stops an attacker resubmitting collected signatures with a
    // different distribution to drain the pot.
    let base = settle_message_hash(
        &chain(1), &ADDR_A, &GAME_ID, 7, &STATE_HASH, &payouts_hash(&[100, 50]),
    );
    let tampered = settle_message_hash(
        &chain(1), &ADDR_A, &GAME_ID, 7, &STATE_HASH, &payouts_hash(&[150, 0]),
    );
    assert_ne!(base, tampered, "settle sig must differ when payouts differ");
}

#[test]
fn settle_sig_is_bound_to_stored_nonce_and_state_hash() {
    let ph = payouts_hash(&[100, 50]);
    let base = settle_message_hash(&chain(1), &ADDR_A, &GAME_ID, 7, &STATE_HASH, &ph);
    let other_nonce = settle_message_hash(&chain(1), &ADDR_A, &GAME_ID, 8, &STATE_HASH, &ph);
    let other_state =
        settle_message_hash(&chain(1), &ADDR_A, &GAME_ID, 7, &[0x33; 32], &ph);
    assert_ne!(base, other_nonce, "settle sig must be bound to stored nonce");
    assert_ne!(base, other_state, "settle sig must be bound to stored state_hash");
}

#[test]
fn state_and_settle_domains_never_collide() {
    // A state-update signature must never be reinterpretable as a settlement
    // authorization (or vice-versa). The distinct domain tags guarantee this
    // even if every other field lines up.
    let ph = payouts_hash(&[]); // empty payouts, minimal settle preimage
    let state = state_message_hash(&chain(1), &ADDR_A, &GAME_ID, 7, &STATE_HASH);
    let settle = settle_message_hash(&chain(1), &ADDR_A, &GAME_ID, 7, &STATE_HASH, &ph);
    assert_ne!(state, settle, "state and settle domains must not collide");
}

#[test]
fn payouts_hash_order_sensitive() {
    // Seat ordering matters: [100,50] pays seat0=100, seat1=50; the reverse is a
    // different distribution and must commit to a different hash.
    assert_ne!(payouts_hash(&[100, 50]), payouts_hash(&[50, 100]));
}
