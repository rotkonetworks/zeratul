//! poker-escrow: FROST 2-of-3 escrow service for poker games.
//!
//! Manages:
//!   - Key generation (trusted dealer for demo, DKG for production)
//!   - Escrow address derivation
//!   - Deposit tracking (watches zidecar for incoming ZEC)
//!   - Rake fee transaction (from escrow -> house wallet)
//!   - Payout transaction (from escrow -> winners)
//!
//! Separate from the relay -- holds key material, the relay doesn't.
//!
//! API:
//!   POST /room                   -- create escrow for a new room
//!   GET  /room/{code}            -- escrow address + deposit status
//!   POST /room/{code}/deposit    -- player reports deposit tx
//!   POST /room/{code}/settle     -- record final stacks, compute payout plan
//!   GET  /room/{code}/payout     -- full payout plan (addresses + amounts for zafu)
//!   POST /room/{code}/sign       -- FROST round 1 (server commitment)
//!   POST /room/{code}/sign-round2-- FROST round 2 (server signature share)
//!   POST /room/{code}/cancel     -- cancel game + refund depositors
//!   GET  /health                 -- service health

use axum::{Router, Json, extract::{Path, State}, response::IntoResponse};
use serde::{Deserialize, Serialize};
use sha2::{Sha256, Digest};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::Mutex;

use osst::redpallas::zcash as frost;
use pasta_curves::pallas::Scalar as PallasScalar;

mod orchard_ua;
mod journal;
mod notify;
mod pin;
mod dispute;
mod frost_relay;
mod frost_dkg;
mod dkg_room;
mod scanner;
mod payout_signing;
mod tx_build;
mod persist;

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

#[derive(Clone, Serialize)]
struct EscrowInfo {
    code: String,
    escrow_address: String,
    player_a_deposited: u64,
    player_b_deposited: u64,
    required_deposit: u64,
    rake_paid: bool,
    game_active: bool,
    created_at: u64,
}

/// A single output in the payout transaction.
#[derive(Clone, Serialize, Deserialize)]
struct PayoutOutput {
    /// Zcash shielded address (Orchard UA or Sapling)
    address: String,
    /// Amount in zatoshis
    amount: u64,
    /// What this output is for
    memo: String,
}

/// Everything the client (zafu WASM) needs to build the payout transaction.
/// The escrow computes this at settle time; the client fetches it via GET /payout.
#[derive(Clone, Serialize, Deserialize)]
struct PayoutPlan {
    /// Room code
    room: String,
    /// Escrow note to spend (hex-encoded address for now; will be a full note reference)
    escrow_source: String,
    /// Total value being spent from escrow (zatoshis)
    escrow_value: u64,
    /// The outputs: player payouts + rake
    outputs: Vec<PayoutOutput>,
    /// Hash of the action log that produced these stacks
    action_log_hash: String,
    /// Timestamp of settlement
    settled_at: u64,
}

/// State of the PCZT-based payout flow for a room. Set by `POST /room/{code}/payout/initiate`,
/// advanced by the background signing task, read by `GET /room/{code}/payout/status`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "phase")]
pub enum PayoutStatus {
    /// no payout requested yet
    None,
    /// signing room opened; waiting for the player to FROST-sign their share
    Pending { relay_room: String },
    /// tx broadcast to zidecar; `txid` is the lowercase hex
    Broadcast { txid: String, relay_room: String },
    /// signing or broadcast failed; payout can be retried with a fresh /initiate
    Failed { reason: String },
}

struct EscrowRoom {
    code: String,
    /// Orchard UA (`u1...`) — `Some` once derivation is done. In trusted-dealer mode this is
    /// set synchronously inside `create_room`. In DKG mode it lands when the background
    /// DKG task finishes.
    escrow_ua: Option<String>,
    /// FROST relay coords for DKG mode — `Some` only when ESCROW_USE_DKG=true. Forwarded
    /// to clients so they can join the relay room and participate.
    frost_relay_url: Option<String>,
    frost_room_code: Option<String>,
    /// poker-escrow's own DKG piece (private). `Some` after DKG completes.
    dkg_key_package_hex: Option<String>,
    /// group's public key package — `Some` after DKG completes. Same on every party.
    dkg_public_key_package_hex: Option<String>,
    /// raw 96-byte FVK hex used by the zidecar compact-block scanner
    dkg_orchard_fvk_hex: Option<String>,
    /// host-broadcast sk; lets us derive new diversified addresses anytime
    dkg_sk_hex: Option<String>,
    /// our FROST identity seed (from dkg_part3) — needed to sign payouts
    dkg_ephemeral_seed_hex: Option<String>,
    /// per-seat deposit UAs (`u1…`) derived at diversifier_index 1, 2
    seat_addresses: Vec<Option<String>>,
    /// per-seat 43-byte raw addresses for matching decrypted notes back to a seat
    seat_addr_bytes: Vec<Option<[u8; 43]>>,
    /// per-seat refund/payout destination — recovered from the `zk.poker/v1/payout:` memo
    /// on the first valid deposit. `None` means the depositor forgot the memo; deposits still
    /// accrue but the game won't start until we know where to refund/pay.
    seat_payout_address: Vec<Option<String>>,
    /// per-seat Ed25519 identity pubkey, pinned on-chain from the deposit memo's `;id:<hex>`
    /// segment. Settlement (`POST /settle`) requires a signature from exactly this key for each
    /// seat, so the operator cannot decide payouts unilaterally. `None` = not yet pinned.
    seat_identity_pubkey: Vec<Option<[u8; 32]>>,
    /// every scanned incoming note kept around so the payout tx builder can spend it later.
    /// Position (orchard merkle tree leaf index) lands here when the tx builder fetches a
    /// witness from zidecar via `GetCommitmentProofs` keyed on `cmx`.
    notes: Vec<scanner::DepositNote>,
    /// resume point for the deposit scanner — last block whose actions we've trial-decrypted
    last_scanned_height: u32,
    /// current state of the PCZT payout flow; advances Pending → Broadcast (or Failed)
    payout_status: PayoutStatus,
    /// legacy 32-byte raw-hex form (osst-derived); retained for the unmigrated /sign endpoints
    escrow_address: [u8; 32],
    group_pubkey: pasta_curves::pallas::Point,
    // server's key share (signer #3)
    server_share: osst::SecretShare<PallasScalar>,
    // player shares (sent to players on creation)
    player_a_share_hex: String,
    player_b_share_hex: String,
    // deposit tracking
    player_a_deposit: u64,  // zatoshis
    player_b_deposit: u64,
    required_deposit: u64,
    rake_bps: u16,
    rake_paid: bool,
    game_active: bool,
    // player zcash addresses (provided at settle time)
    player_a_address: Option<String>,
    player_b_address: Option<String>,
    // payout
    final_stacks: Option<(u64, u64)>,
    payout_plan: Option<PayoutPlan>,
    // FROST signing state (ephemeral, consumed on round 2)
    pending_nonces: Option<osst::frost::Nonces<PallasScalar>>,
    created_at: u64,
    /// Per-room capability secret minted at creation. Whoever created the room (the poker-server
    /// acting for the seated players) receives it once in the create response. It authorizes the
    /// money-moving endpoints (currently `payout/initiate`). Compared in constant time. This binds
    /// a payout request to a party that knew the room's creation secret — an anonymous HTTP caller
    /// who only knows the room code (which is public) cannot drain the escrow.
    payout_token: [u8; 32],
    /// Nullifiers/txids already credited to the deposit counters, so a deposit observed by both
    /// the HTTP `/deposit` path and the background scanner is counted exactly once. Keyed by a
    /// canonical dedup id (nullifier hex when known from the scanner, else `txid:seat` for the
    /// self-reported HTTP path).
    counted_deposits: std::collections::HashSet<String>,
    /// MEMPOOL-seen (0-conf) deposit totals per seat. EPHEMERAL — never persisted; rebuilt from
    /// the live mempool within one poll after a restart. NEVER authoritative for money: only the
    /// DEAL gate (game start) may read confirmed+pending. Settlement / rake / payout read the
    /// confirmed `player_{a,b}_deposit` counters ONLY.
    player_a_deposit_pending: u64,
    player_b_deposit_pending: u64,
    /// Per-nullifier record of every mempool note currently contributing to a `_pending` counter.
    /// Keyed by nullifier hex (same key space as `counted_deposits`). Used to dedup pending
    /// credits, to decrement pending when a note confirms (promotion), and to evict pending when
    /// a note leaves the mempool without confirming. EPHEMERAL — never persisted.
    pending_deposits: std::collections::HashMap<String, PendingDeposit>,
    /// A co-signed `/settle` outcome that arrived while a deposit was still unconfirmed on-chain.
    /// FIX 1: instead of turning a valid co-signed plan into a terminal failure, we RECORD it here
    /// and the confirmed-deposit scanner completes it (builds `payout_plan`) the moment both
    /// deposits confirm. Fail-closed: it NEVER executes while a deposit is unconfirmed, and if a
    /// deposit is EVICTED (never confirms) the queued settlement is abandoned (see
    /// `evicted_shortfall`). Persisted so a restart still completes the queued settlement.
    settle_pending: Option<PendingSettlement>,
    /// FIX 2: set when a pending (mempool-seen) note is EVICTED for a seat whose CONFIRMED deposit
    /// is still short. Marks the room as having lost a never-confirmed buy-in, so the queued
    /// settled-plan path must NOT auto-execute a full-pot payout (that pot would be the other
    /// seat's real money). Recovery is via `/cancel` refund-each-own-confirmed or `/arbitrate`.
    /// Persisted so the guard survives a restart.
    evicted_shortfall: bool,
    /// FIX 3: reason string when DKG (`run_dkg`) errored, leaving the room permanently unable to
    /// derive an escrow address / co-sign. Surfaced via `get_room` (`dkg_failed`) so the room can
    /// be shown as terminally failed instead of "setting up forever". Persisted.
    dkg_failed: Option<String>,
}

/// A co-signed settlement recorded while a deposit is still unconfirmed (FIX 1). Captures exactly
/// the inputs `settle` needs to (re)build the payout plan once both deposits confirm — including
/// BOTH players' Ed25519 signatures over the outcome, so the queued plan stays non-repudiable and
/// re-verifiable. EPHEMERAL money is never moved from this; it only unblocks the CONFIRMED path.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PendingSettlement {
    pub player_a_stack: u64,
    pub player_b_stack: u64,
    pub player_a_address: String,
    pub player_b_address: String,
    pub action_log_hash: String,
    pub player_a_sig: String,
    pub player_b_sig: String,
    /// wall-clock ms the co-signed outcome was first queued (dedup / audit).
    pub queued_at: u64,
}

/// One mempool-seen deposit tracked in `EscrowRoom::pending_deposits`. EPHEMERAL — never on disk.
/// A pending deposit is a UX "we've seen it" signal only; it is NEVER spendable money and NEVER
/// enters `room.notes`.
#[derive(Clone, Debug)]
struct PendingDeposit {
    /// seat 0 = player A, 1 = player B
    seat: u8,
    /// note value in zatoshis, added to the seat's `_pending` counter
    value_zat: u64,
    /// wall-clock ms the note was FIRST observed in the mempool (journaling / age).
    first_seen_ms: u64,
    /// wall-clock ms the note was LAST seen in the mempool. Eviction measures absence from
    /// here, not from `first_seen_ms`: a note that MINES leaves the mempool, and the block
    /// scanner (20s poll) needs time to credit it — evicting on the first missing tick would
    /// falsely flag a normal confirmation as a double-spend. See `VANISH_GRACE_MS`.
    last_seen_ms: u64,
}

type Rooms = Arc<Mutex<HashMap<String, EscrowRoom>>>;

#[derive(Clone)]
pub struct AppState {
    rooms: Rooms,
    house_address: String,
    zidecar_url: String,
    verify_deposits: bool,
    network: zcash_protocol::consensus::NetworkType,
    /// when true, /room runs DKG via the FROST relay instead of trusted-dealer keygen
    use_dkg: bool,
    /// FROST relay WebSocket URL (used in DKG mode)
    frost_relay_url: String,
    /// durable per-room key-material + payout-state store. `None` = persistence disabled
    /// (room state will NOT survive a restart — logged loudly at startup).
    persist: Option<persist::Store>,
}

// ---------------------------------------------------------------------------
// Handlers
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
struct CreateRoomReq {
    code: String,
    required_deposit: u64,  // zatoshis per player
    rake_bps: u16,
}

#[derive(Serialize)]
struct CreateRoomResp {
    escrow_address: String,
    player_a_share: String,
    player_b_share: String,
    public_key_package: String,
}

/// Derive a real Orchard Unified Address for an escrow room via frost-spend's
/// trusted-dealer keygen. Returns the `u1...` string. Trusted-dealer is the
/// pragmatic shortcut for Phase 2.1 — Phase 2.2 replaces this with DKG.
fn derive_escrow_ua(network: zcash_protocol::consensus::NetworkType) -> Result<String, String> {
    let dealer = frost_spend::orchestrate::dealer_keygen(2, 3)
        .map_err(|e| format!("dealer_keygen: {:?}", e))?;
    let raw = frost_spend::orchestrate::derive_address_raw(&dealer.public_key_package_hex, 0)
        .map_err(|e| format!("derive_address_raw: {:?}", e))?;
    orchard_ua::encode_unified(raw, network)
}

/// Build the osst-derived legacy bits (will be removed in Phase 2.4). Pure / sync /
/// rng-not-Send — must run before any `.await`.
fn make_legacy_osst(req: &CreateRoomReq) -> Result<(dkg_room::LegacyOsstShim, String, String, String), String> {
    let mut rng = rand::thread_rng();
    let (player_a_share, player_b_share, jury_network, group_pubkey) =
        frost::setup_escrow(1, 1, &mut rng)
            .map_err(|e| format!("osst setup_escrow: {:?}", e))?;
    let escrow_address = frost::derive_address_bytes(&group_pubkey);
    let a_hex = hex::encode(&share_to_bytes(&player_a_share));
    let b_hex = hex::encode(&share_to_bytes(&player_b_share));
    let pubkey_hex = hex::encode(&point_to_bytes(&group_pubkey));
    let server_share = jury_network.node_shares.into_iter().next()
        .expect("jury_n=1 should produce exactly 1 share");
    let shim = dkg_room::LegacyOsstShim {
        escrow_address,
        group_pubkey,
        server_share,
        player_a_share_hex: a_hex.clone(),
        player_b_share_hex: b_hex.clone(),
    };
    let _ = req; // silence unused warning if req fields aren't used here yet
    Ok((shim, a_hex, b_hex, pubkey_hex))
}

async fn create_room(
    State(state): State<AppState>,
    Json(req): Json<CreateRoomReq>,
) -> impl IntoResponse {
    let (shim, a_share_hex, b_share_hex, pubkey_hex) = match make_legacy_osst(&req) {
        Ok(v) => v,
        Err(e) => return Json(serde_json::json!({"error": e})),
    };

    if state.use_dkg {
        create_room_dkg(state, req, shim, a_share_hex, b_share_hex, pubkey_hex).await
    } else {
        create_room_trusted_dealer(state, req, shim, a_share_hex, b_share_hex, pubkey_hex).await
    }
}

async fn create_room_trusted_dealer(
    state: AppState,
    req: CreateRoomReq,
    shim: dkg_room::LegacyOsstShim,
    a_share_hex: String,
    b_share_hex: String,
    pubkey_hex: String,
) -> Json<serde_json::Value> {
    let escrow_ua = match derive_escrow_ua(state.network) {
        Ok(u) => u,
        Err(e) => return Json(serde_json::json!({"error": e})),
    };

    let room = EscrowRoom {
        code: req.code.clone(),
        escrow_ua: Some(escrow_ua.clone()),
        frost_relay_url: None,
        frost_room_code: None,
        dkg_key_package_hex: None,
        dkg_public_key_package_hex: None,
        dkg_orchard_fvk_hex: None,
        dkg_sk_hex: None,
        dkg_ephemeral_seed_hex: None,
        seat_addresses: vec![None, None],
        seat_addr_bytes: vec![None, None],
        seat_payout_address: vec![None, None],
        seat_identity_pubkey: vec![None, None],
        notes: Vec::new(),
        last_scanned_height: 0,
        payout_status: PayoutStatus::None,
        escrow_address: shim.escrow_address,
        group_pubkey: shim.group_pubkey,
        server_share: shim.server_share,
        player_a_share_hex: shim.player_a_share_hex,
        player_b_share_hex: shim.player_b_share_hex,
        player_a_deposit: 0,
        player_b_deposit: 0,
        required_deposit: req.required_deposit,
        rake_bps: req.rake_bps,
        rake_paid: false,
        game_active: false,
        player_a_address: None,
        player_b_address: None,
        final_stacks: None,
        payout_plan: None,
        pending_nonces: None,
        created_at: now_ms(),
        payout_token: new_payout_token(),
        counted_deposits: std::collections::HashSet::new(),
        player_a_deposit_pending: 0,
        player_b_deposit_pending: 0,
        pending_deposits: std::collections::HashMap::new(),
        settle_pending: None,
        evicted_shortfall: false,
        dkg_failed: None,
    };

    let payout_token_hex = hex::encode(room.payout_token);
    if let Some(s) = state.persist.as_ref() {
        s.save_room(&room);
    }
    state.rooms.lock().await.insert(req.code.clone(), room);
    tracing::info!("escrow created (trusted-dealer): {} -> {}", req.code, &escrow_ua);

    Json(serde_json::json!({
        "escrow_address": escrow_ua,
        "player_a_share": a_share_hex,
        "player_b_share": b_share_hex,
        "public_key_package": pubkey_hex,
        "dkg_mode": false,
        "payout_token": payout_token_hex,
    }))
}

async fn create_room_dkg(
    state: AppState,
    req: CreateRoomReq,
    shim: dkg_room::LegacyOsstShim,
    a_share_hex: String,
    b_share_hex: String,
    pubkey_hex: String,
) -> Json<serde_json::Value> {
    let prov = match dkg_room::provision(
        state.rooms.clone(),
        req.code.clone(),
        state.frost_relay_url.clone(),
        state.network,
        state.zidecar_url.clone(),
        state.house_address.clone(),
        state.persist.clone(),
    ).await {
        Ok(p) => p,
        Err(e) => return Json(serde_json::json!({"error": format!("dkg provision: {}", e)})),
    };

    let room = dkg_room::empty_room(
        req.code.clone(),
        req.required_deposit,
        req.rake_bps,
        state.frost_relay_url.clone(),
        prov.frost_room_code.clone(),
        shim,
        new_payout_token(),
    );
    let payout_token_hex = hex::encode(room.payout_token);
    if let Some(s) = state.persist.as_ref() {
        s.save_room(&room);
    }
    state.rooms.lock().await.insert(req.code.clone(), room);

    journal::record(&req.code, "room_created", serde_json::json!({
        "required_deposit": req.required_deposit,
        "rake_bps": req.rake_bps,
        "frost_room_code": prov.frost_room_code,
        "mode": "dkg",
    }));

    tracing::info!(
        "escrow created (dkg): {} -> frost_room={} (UA pending until peers join)",
        req.code, prov.frost_room_code
    );

    Json(serde_json::json!({
        "escrow_address": serde_json::Value::Null,
        "player_a_share": a_share_hex,
        "player_b_share": b_share_hex,
        "public_key_package": pubkey_hex,
        "dkg_mode": true,
        "frost_relay_url": state.frost_relay_url,
        "frost_room_code": prov.frost_room_code,
        "payout_token": payout_token_hex,
    }))
}

async fn get_room(
    State(state): State<AppState>,
    Path(code): Path<String>,
) -> impl IntoResponse {
    let rooms = state.rooms.lock().await;
    match rooms.get(&code) {
        Some(room) => Json(serde_json::json!({
            "code": room.code,
            "escrow_address": room.escrow_ua,
            "dkg_pending": room.frost_relay_url.is_some() && room.escrow_ua.is_none(),
            "frost_relay_url": room.frost_relay_url,
            "frost_room_code": room.frost_room_code,
            "seat_addresses": room.seat_addresses,
            "seat_payout_addresses": room.seat_payout_address,
            "unspent_notes_count": room.notes.len(),
            "last_scanned_height": room.last_scanned_height,
            "player_a_deposit": room.player_a_deposit,
            "player_b_deposit": room.player_b_deposit,
            // MEMPOOL-seen (0-conf) totals — UX signal only, NOT spendable. `both_deposited`
            // below stays CONFIRMED-only on purpose (it gates money paths downstream).
            "player_a_deposit_pending": room.player_a_deposit_pending,
            "player_b_deposit_pending": room.player_b_deposit_pending,
            "required_deposit": room.required_deposit,
            "rake_bps": room.rake_bps,
            "rake_paid": room.rake_paid,
            "game_active": room.game_active,
            "settled": room.payout_plan.is_some(),
            "both_deposited": room.player_a_deposit >= room.required_deposit
                && room.player_b_deposit >= room.required_deposit,
            "both_payout_addresses_known": room.seat_payout_address.iter().take(2).all(|a| a.is_some()),
            // FIX 1: a co-signed settlement is queued and will execute once both deposits confirm.
            // Client shows "settling — waiting for confirmation" instead of a terminal failure.
            "settle_pending_confirmation": room.settle_pending.is_some() && room.payout_plan.is_none(),
            // FIX 2: a pending buy-in was evicted while its seat's confirmed deposit was short —
            // the queued settlement can no longer pay a full pot; recovery is refund/arbitrate.
            "evicted_shortfall": room.evicted_shortfall,
            // FIX 3: DKG errored — the room is terminally unable to co-sign (not "setting up").
            "dkg_failed": room.dkg_failed.is_some(),
            "dkg_failed_reason": room.dkg_failed,
        })),
        None => Json(serde_json::json!({"error": "room not found"})),
    }
}

/// player reports their deposit tx (0-conf)
#[derive(Deserialize)]
struct DepositReport {
    seat: u8,  // 0 = player A, 1 = player B
    txid: String,
    amount: u64,
}

async fn report_deposit(
    State(state): State<AppState>,
    Path(code): Path<String>,
    Json(req): Json<DepositReport>,
) -> impl IntoResponse {
    if req.seat > 1 {
        return Json(serde_json::json!({"error": "invalid seat"}));
    }

    // BUG 2 — fail closed under verification. When ESCROW_VERIFY_DEPOSITS=true the authoritative
    // deposit signal is the background Orchard compact-block scanner (see dkg_room::run_deposit_poll),
    // which trial-decrypts real on-chain notes and credits by nullifier. A self-reported 0-conf HTTP
    // amount is NOT verified against the chain here, so we must NOT credit it on trust — doing so
    // would let a caller inflate their balance with a fabricated txid/amount. Reject and let the
    // scanner credit the real note when it lands.
    if state.verify_deposits {
        let rooms = state.rooms.lock().await;
        if rooms.get(&code).is_none() {
            return Json(serde_json::json!({"error": "room not found"}));
        }
        tracing::warn!(
            "deposit: room={} seat={} REJECTED self-report under ESCROW_VERIFY_DEPOSITS — \
             on-chain scanner is the authoritative signal",
            code, req.seat,
        );
        return Json(serde_json::json!({
            "ok": false,
            "error": "self-reported deposits are not credited when verification is enabled; \
                      the on-chain scanner credits confirmed notes automatically",
            "verify_deposits": true,
        }));
    }

    // Demo mode (verify_deposits=false): trust the self-report, but dedup so the same txid/seat
    // is only ever counted once, even if reported repeatedly (BUG 3 — the HTTP path and the
    // scanner both += the same counters; dedup keys keep a deposit counted exactly once).
    let mut rooms = state.rooms.lock().await;
    let room = match rooms.get_mut(&code) {
        Some(r) => r,
        None => return Json(serde_json::json!({"error": "room not found"})),
    };

    let dedup_key = format!("http:{}:{}", req.txid.trim().to_lowercase(), req.seat);
    if !room.counted_deposits.insert(dedup_key) {
        let both = room.player_a_deposit >= room.required_deposit
            && room.player_b_deposit >= room.required_deposit;
        tracing::info!("deposit: room={} seat={} txid already counted — ignoring duplicate", code, req.seat);
        return Json(serde_json::json!({
            "ok": true,
            "duplicate": true,
            "player_a_deposit": room.player_a_deposit,
            "player_b_deposit": room.player_b_deposit,
            "both_deposited": both,
            "game_active": room.game_active,
        }));
    }

    match req.seat {
        0 => room.player_a_deposit += req.amount,
        1 => room.player_b_deposit += req.amount,
        _ => unreachable!("seat validated above"),
    }

    let both = both_deposits_satisfied(room.player_a_deposit, room.player_b_deposit, room.required_deposit);

    tracing::info!("deposit: room={} seat={} amount={} txid={} both={}",
        code, req.seat, req.amount, &req.txid[..req.txid.len().min(16)], both);

    if both && !room.game_active {
        room.game_active = true;
    }

    // persist updated deposit counters / dedup set / game_active
    if let Some(s) = state.persist.as_ref() {
        s.save_room(room);
    }

    Json(serde_json::json!({
        "ok": true,
        "player_a_deposit": room.player_a_deposit,
        "player_b_deposit": room.player_b_deposit,
        "both_deposited": both,
        "game_active": room.game_active,
    }))
}

/// game ended -- record final stacks, compute full payout plan for zafu
#[derive(Deserialize)]
struct SettleReq {
    player_a_stack: u64,
    player_b_stack: u64,
    /// Zcash address for player A's payout
    player_a_address: String,
    /// Zcash address for player B's payout
    player_b_address: String,
    action_log_hash: String,
    /// hex Ed25519 signature by seat 0's pinned identity key over `settlement_message(..)`
    #[serde(default)]
    player_a_sig: String,
    /// hex Ed25519 signature by seat 1's pinned identity key over `settlement_message(..)`
    #[serde(default)]
    player_b_sig: String,
}

/// Canonical settlement bytes that BOTH players sign. Any change to who-gets-what
/// (stacks or destination addresses) changes these bytes, so a valid pair of
/// signatures proves both seats agreed to this exact outcome. Field order and
/// separators are fixed — the client must build the identical string.
fn settlement_message(
    code: &str, a_stack: u64, b_stack: u64, a_addr: &str, b_addr: &str, log_hash: &str,
) -> String {
    format!(
        "zk.poker/settle/v1:{}:{}:{}:{}:{}:{}",
        code, a_stack, b_stack, a_addr, b_addr, log_hash,
    )
}

/// Verify a hex Ed25519 signature (RFC8032, as produced by WebCrypto 'Ed25519')
/// by `pubkey` over `msg`. Returns false on any decode/length/verify failure.
fn verify_settlement_sig(pubkey: &[u8; 32], msg: &[u8], sig_hex: &str) -> bool {
    use ed25519_dalek::{Signature, Verifier, VerifyingKey};
    let Ok(vk) = VerifyingKey::from_bytes(pubkey) else { return false };
    let Ok(sig_bytes) = hex::decode(sig_hex.trim()) else { return false };
    let Ok(sig_arr) = <[u8; 64]>::try_from(sig_bytes.as_slice()) else { return false };
    vk.verify(msg, &Signature::from_bytes(&sig_arr)).is_ok()
}

/// On-chain tx fee reserved from the pot for the payout transaction. The settle
/// plan and the payout builder MUST agree on this value or the tx won't balance.
const TX_PAYOUT_FEE_ZAT: u64 = 10_000;

/// Pure, testable settlement split. Given the pot and the co-signed final stacks,
/// True when `addr` is a real, payable Zcash address rather than an unset/placeholder
/// value. Rake is only ever routed to a house address that passes this check, so an
/// operator who hasn't set HOUSE_ADDRESS can never break a winner's payout on a bad
/// recipient. Deliberately conservative: known network prefixes only, and obvious
/// placeholder markers (`placeholder`, `demo`, `…`) are rejected outright.
fn house_addr_payable(addr: &str) -> bool {
    let a = addr.trim();
    if a.len() < 20 { return false; }
    if a.contains("placeholder") || a.contains("demo") || a.contains("...") { return false; }
    // The rake is paid as an ORCHARD output, so the house MUST be an Orchard-capable unified
    // address. DECODE it (not just prefix-check): a typo'd/truncated UA — or a sapling-only /
    // transparent address — would otherwise pass a prefix check, reserve rake, and then break
    // the ENTIRE winner payout at PCZT recipient-decode time. On a decode failure we treat the
    // house as unpayable → rake is dropped and the winner's payout still succeeds. Network is
    // inferred from the UA prefix (u1 = mainnet, utest1 = testnet).
    let mainnet = if a.starts_with("u1") { true }
        else if a.starts_with("utest1") { false }
        else { return false; };
    crate::tx_build::parse_orchard_ua(a, mainnet).is_ok()
}

/// produce the payout outputs. Reserves BOTH the rake (to `house_addr`) AND the
/// on-chain tx fee, so `sum(outputs) + TX_PAYOUT_FEE_ZAT == total_pot` exactly —
/// `payout_b` is the exact remainder of `distributable`, so proportional truncation
/// never strands funds. Winner-take-all skips the zero output. Seat 0 = A, seat 1 = B.
/// Rake is charged only when `house_addr` is payable (see `house_addr_payable`).
fn compute_settlement_outputs(
    code: &str,
    total_pot: u64,
    rake_bps: u16,
    a_stack: u64,
    b_stack: u64,
    a_addr: &str,
    b_addr: &str,
    house_addr: &str,
) -> (Vec<PayoutOutput>, u64, u64, u64) {
    // A rake output is only reserved when the house address is a real, payable
    // Zcash address. If HOUSE_ADDRESS is unset or a placeholder (demo/…), we charge
    // NO rake and fold that value back to the players — otherwise an unconfigured
    // operator would strand the winner's payout on an invalid recipient (the whole
    // PCZT build fails at recipient-decode time, before broadcast). Set a real
    // HOUSE_ADDRESS to start collecting rake; this is a no-op once configured.
    let effective_rake_bps = if house_addr_payable(house_addr) { rake_bps } else {
        if rake_bps > 0 {
            tracing::warn!(
                "room {}: rake_bps={} requested but house address '{}' is not payable — charging 0 rake (set HOUSE_ADDRESS)",
                code, rake_bps, house_addr,
            );
        }
        0
    };
    let rake = (total_pot as u128 * effective_rake_bps as u128 / 10_000) as u64;
    // reserve rake AND the tx fee; the fee is covered by players' pre-funded fee_per_seat.
    let distributable = total_pot.saturating_sub(rake).saturating_sub(TX_PAYOUT_FEE_ZAT);
    let total_stacks = a_stack + b_stack;
    let payout_a = if total_stacks > 0 {
        (distributable as u128 * a_stack as u128 / total_stacks as u128) as u64
    } else {
        distributable / 2
    };
    let payout_b = distributable - payout_a; // exact remainder — no truncation loss
    let mut outputs = Vec::new();
    if payout_a > 0 {
        outputs.push(PayoutOutput { address: a_addr.to_string(), amount: payout_a, memo: format!("zk.poker payout room={}", code) });
    }
    if payout_b > 0 {
        outputs.push(PayoutOutput { address: b_addr.to_string(), amount: payout_b, memo: format!("zk.poker payout room={}", code) });
    }
    if rake > 0 {
        outputs.push(PayoutOutput { address: house_addr.to_string(), amount: rake, memo: format!("zk.poker rake room={}", code) });
    }
    (outputs, payout_a, payout_b, rake)
}

/// One depositor for the refund path: where its own money goes back, and how much it
/// CONFIRMED into the vault. Modular / N-seat: `/cancel` and the unsigned-exit guard build a
/// `Vec<SeatRefund>` (today 2 seats; a future multiway table has N) rather than hardcoding
/// seat0/seat1. `address` is the seat's on-chain-pinned payout/refund address.
#[derive(Clone, Debug)]
struct SeatRefund {
    address: String,
    confirmed_deposit: u64,
}

/// The single, escrow-authoritative refund split: give each seat back EXACTLY its own confirmed
/// deposit, minus a proportional share of the on-chain tx fee. This is the ONLY unsigned money
/// exit the escrow will build itself — it can never pay a seat more than that seat confirmed, so
/// it is always safe under a 0-conf shortfall (an unconfirmed/evicted buy-in simply isn't in
/// `confirmed_deposit`). No rake is charged on a refund. Conservation holds exactly:
/// `sum(outputs) + fee == sum(confirmed_deposit)`. The last non-zero seat absorbs the fee
/// remainder so proportional truncation never strands a zatoshi. Generalises to N seats.
///
/// Returns `(outputs, per_seat_refund_amounts)`. A seat with a zero refund emits no output.
fn compute_refund_outputs(code: &str, seats: &[SeatRefund]) -> (Vec<PayoutOutput>, Vec<u64>) {
    let total_confirmed: u64 = seats.iter().map(|s| s.confirmed_deposit).sum();
    // reserve the on-chain tx fee from the pot, distributed proportionally to each seat's stake.
    let distributable = total_confirmed.saturating_sub(TX_PAYOUT_FEE_ZAT);
    let mut amounts = vec![0u64; seats.len()];
    // index of the last seat with a non-zero confirmed deposit — it absorbs the fee remainder so
    // the sum is exact (no proportional-truncation loss).
    let last_funded = seats.iter().rposition(|s| s.confirmed_deposit > 0);
    let mut allocated: u64 = 0;
    for (i, s) in seats.iter().enumerate() {
        if total_confirmed == 0 || s.confirmed_deposit == 0 { continue; }
        if Some(i) == last_funded {
            // exact remainder → never strands a zatoshi.
            amounts[i] = distributable - allocated;
        } else {
            let share = (distributable as u128 * s.confirmed_deposit as u128 / total_confirmed as u128) as u64;
            amounts[i] = share;
            allocated += share;
        }
    }
    let mut outputs = Vec::new();
    for (i, s) in seats.iter().enumerate() {
        if amounts[i] > 0 {
            outputs.push(PayoutOutput {
                address: s.address.clone(),
                amount: amounts[i],
                memo: format!("zk.poker refund room={}", code),
            });
        }
    }
    (outputs, amounts)
}

/// Snapshot each seat's refund basis (pinned refund address + CONFIRMED deposit) from a room, as a
/// per-seat vector. Seat 0 = A, seat 1 = B today; extend `seat_payout_address`/the deposit vector
/// for N seats. A seat with no pinned address is dropped from the basis (nowhere to refund it) —
/// its funds strand until an address is pinned or `/arbitrate` rules, which is the safe default.
fn room_seat_refunds(room: &EscrowRoom) -> Vec<SeatRefund> {
    let confirmed = [room.player_a_deposit, room.player_b_deposit];
    let mut seats = Vec::new();
    for (i, dep) in confirmed.iter().enumerate() {
        if let Some(Some(addr)) = room.seat_payout_address.get(i) {
            seats.push(SeatRefund { address: addr.clone(), confirmed_deposit: *dep });
        }
    }
    seats
}

/// Does a caller-supplied unsigned output set match the escrow's OWN refund-each-own-confirmed
/// split? This is the ONLY unsigned split the escrow will spend without a co-signed plan
/// (Settle-C1). We recompute the authoritative refund and require the caller's outputs to be a
/// subset-equal match (same address→amount pairs, in any order). Anything else — a winner-take-all
/// split, a shifted amount, an extra recipient — is a non-refund exit and must be rejected unless
/// it carries a co-signed `/settle` plan or an `/arbitrate` operator ruling.
fn outputs_match_refund(req_outputs: &[(String, u64)], refund_outputs: &[PayoutOutput]) -> bool {
    if req_outputs.len() != refund_outputs.len() {
        return false;
    }
    // match as a multiset of (address, amount) — order-independent, no double-matching.
    let mut remaining: Vec<&PayoutOutput> = refund_outputs.iter().collect();
    for (addr, amt) in req_outputs {
        if let Some(pos) = remaining.iter().position(|o| &o.address == addr && o.amount == *amt) {
            remaining.remove(pos);
        } else {
            return false;
        }
    }
    remaining.is_empty()
}

/// Outcome of verifying the co-signing gate for a settlement request. Pure of side effects on the
/// room so it can be reused by both `/settle` and the confirmed scanner completing a queued plan.
enum CosignCheck {
    Ok,
    /// a JSON error body the handler should return verbatim (bad sigs / unpinned identities / …)
    Reject(serde_json::Value),
}

/// Verify BOTH seats co-signed the exact outcome with their on-chain-pinned Ed25519 identity keys,
/// and that signed destinations match any on-chain-pinned payout address. No room mutation. Mirrors
/// the gate previously inlined in `settle` so a queued settlement re-uses the identical rule.
fn verify_settle_cosign(
    room: &EscrowRoom,
    code: &str,
    a_stack: u64, b_stack: u64,
    a_addr: &str, b_addr: &str,
    log_hash: &str,
    a_sig: &str, b_sig: &str,
) -> CosignCheck {
    let allow_unsigned = std::env::var("ESCROW_ALLOW_UNSIGNED_SETTLE")
        .ok().as_deref() == Some("1");
    let pk_a = room.seat_identity_pubkey.get(0).and_then(|p| *p);
    let pk_b = room.seat_identity_pubkey.get(1).and_then(|p| *p);
    match (pk_a, pk_b) {
        (Some(pk_a), Some(pk_b)) => {
            let msg = settlement_message(code, a_stack, b_stack, a_addr, b_addr, log_hash);
            let a_ok = verify_settlement_sig(&pk_a, msg.as_bytes(), a_sig);
            let b_ok = verify_settlement_sig(&pk_b, msg.as_bytes(), b_sig);
            if !a_ok || !b_ok {
                tracing::warn!("settle REJECTED room={} — bad player signatures (a_ok={} b_ok={})", code, a_ok, b_ok);
                return CosignCheck::Reject(serde_json::json!({
                    "error": "settlement requires a valid Ed25519 signature from BOTH seats' pinned identity keys",
                    "player_a_sig_ok": a_ok,
                    "player_b_sig_ok": b_ok,
                }));
            }
            // defence in depth: if a payout address was pinned on-chain, the signed
            // destination must match it — a signature can't redirect to a fresh addr.
            for (i, (req_addr, pinned)) in [
                (a_addr, room.seat_payout_address.get(0).and_then(|a| a.clone())),
                (b_addr, room.seat_payout_address.get(1).and_then(|a| a.clone())),
            ].into_iter().enumerate() {
                if let Some(p) = pinned {
                    if p != req_addr {
                        tracing::warn!("settle REJECTED room={} seat={} — payout addr != on-chain pinned addr", code, i);
                        return CosignCheck::Reject(serde_json::json!({"error": "payout address does not match the on-chain pinned address"}));
                    }
                }
            }
            tracing::info!("settle room={} — both player signatures verified", code);
            CosignCheck::Ok
        }
        _ if allow_unsigned => {
            tracing::warn!("settle room={} — UNSIGNED (ESCROW_ALLOW_UNSIGNED_SETTLE=1, demo only)", code);
            CosignCheck::Ok
        }
        _ => {
            tracing::warn!("settle REJECTED room={} — seat identities not pinned on-chain; cannot verify co-signing", code);
            CosignCheck::Reject(serde_json::json!({
                "error": "seat identities are not pinned on-chain (deposit memo must carry `;id:<pubkey>`); \
                          settlement cannot be verified. set ESCROW_ALLOW_UNSIGNED_SETTLE=1 for demo only.",
            }))
        }
    }
}

/// Build + record the CONFIRMED payout plan for a co-signed outcome. Callers MUST already have
/// verified (a) both deposits confirmed via `both_deposits_satisfied` and (b) the co-signing gate.
/// Idempotent: if a `payout_plan` is already present it is a no-op (a duplicate `/settle` or a
/// scanner re-entry must not double-apply). Returns the plan actually in effect.
#[allow(clippy::too_many_arguments)]
fn finalize_settlement(
    room: &mut EscrowRoom,
    house_address: &str,
    persist: Option<&persist::Store>,
    code: &str,
    a_stack: u64, b_stack: u64,
    a_addr: &str, b_addr: &str,
    log_hash: &str,
    a_sig: &str, b_sig: &str,
) -> PayoutPlan {
    // IDEMPOTENCY: a plan already exists → don't rebuild or re-journal. Clear any queued
    // settlement so we don't try to complete it twice.
    if let Some(existing) = room.payout_plan.clone() {
        room.settle_pending = None;
        return existing;
    }

    room.player_a_address = Some(a_addr.to_string());
    room.player_b_address = Some(b_addr.to_string());

    let total_pot = room.player_a_deposit + room.player_b_deposit;
    let (outputs, payout_a, payout_b, rake) = compute_settlement_outputs(
        code, total_pot, room.rake_bps,
        a_stack, b_stack, a_addr, b_addr, house_address,
    );

    room.final_stacks = Some((payout_a, payout_b));
    room.game_active = false;

    let plan = PayoutPlan {
        room: code.to_string(),
        escrow_source: hex::encode(&room.escrow_address),
        escrow_value: total_pot,
        outputs: outputs.clone(),
        action_log_hash: log_hash.to_string(),
        settled_at: now_ms(),
    };
    room.payout_plan = Some(plan.clone());
    // a queued settlement (if any) is now fulfilled — clear it.
    room.settle_pending = None;

    if let Some(s) = persist {
        s.save_room(room);
    }

    journal::record(code, "settlement_finalized", serde_json::json!({
        "player_a_stack": a_stack,
        "player_b_stack": b_stack,
        "player_a_address": a_addr,
        "player_b_address": b_addr,
        "action_log_hash": log_hash,
        "player_a_sig": a_sig,
        "player_b_sig": b_sig,
        "payout_a": payout_a,
        "payout_b": payout_b,
        "rake": rake,
        "total_pot": total_pot,
    }));

    tracing::info!("settle: room={} A={} B={} rake={} outputs={} log={}",
        code, payout_a, payout_b, rake, outputs.len(),
        &log_hash[..log_hash.len().min(16)]);

    plan
}

/// FIX 1 completion hook, called by the confirmed-deposit scanner after it credits new confirmed
/// notes. If a co-signed settlement was queued while a deposit was unconfirmed, and BOTH deposits
/// are NOW confirmed, and no eviction-shortfall taints the room, this re-verifies the co-signing
/// against the (possibly newly-pinned) identity keys and builds the CONFIRMED payout plan. Holds
/// the room lock (caller passes `&mut EscrowRoom`). Idempotent via `finalize_settlement`.
///
/// Fail-closed conditions that leave the queued settlement untouched (no payout):
///   - either deposit still short (`both_deposits_satisfied` false),
///   - `evicted_shortfall` set (a never-confirmed buy-in was evicted — see FIX 2),
///   - co-signing no longer verifies (identity keys weren't pinned / signatures invalid).
pub fn try_complete_pending_settlement(
    room: &mut EscrowRoom,
    house_address: &str,
    persist: Option<&persist::Store>,
    code: &str,
) {
    // nothing queued, or already finalized → nothing to do.
    if room.payout_plan.is_some() || room.settle_pending.is_none() {
        return;
    }
    // FIX 2: a pending buy-in was evicted for a still-short seat — the pot is not fully confirmed
    // by both seats' own money. Do NOT auto-execute; leave recovery to /cancel or /arbitrate.
    if room.evicted_shortfall {
        tracing::warn!("pending settle NOT completed room={} — evicted_shortfall set (unconfirmed buy-in); use refund/arbitrate", code);
        return;
    }
    // both deposits must be CONFIRMED (money gate).
    if !both_deposits_satisfied(room.player_a_deposit, room.player_b_deposit, room.required_deposit) {
        return;
    }
    let pending = room.settle_pending.clone().expect("checked is_some above");
    // re-verify the co-signing now that both deposits (and their pinned identity keys) are on-chain.
    if let CosignCheck::Reject(_) = verify_settle_cosign(
        room, code,
        pending.player_a_stack, pending.player_b_stack,
        &pending.player_a_address, &pending.player_b_address,
        &pending.action_log_hash, &pending.player_a_sig, &pending.player_b_sig,
    ) {
        tracing::error!("pending settle NOT completed room={} — queued co-signing failed re-verification; leaving queued", code);
        return;
    }
    journal::record(code, "settlement_completed_on_confirmation", serde_json::json!({
        "queued_at": pending.queued_at,
        "player_a_deposit": room.player_a_deposit,
        "player_b_deposit": room.player_b_deposit,
    }));
    tracing::info!("pending settle COMPLETING room={} — both deposits confirmed, executing queued co-signed plan", code);
    let _ = finalize_settlement(
        room, house_address, persist, code,
        pending.player_a_stack, pending.player_b_stack,
        &pending.player_a_address, &pending.player_b_address,
        &pending.action_log_hash, &pending.player_a_sig, &pending.player_b_sig,
    );
}

async fn settle(
    State(state): State<AppState>,
    Path(code): Path<String>,
    Json(req): Json<SettleReq>,
) -> impl IntoResponse {
    let mut rooms = state.rooms.lock().await;
    let room = match rooms.get_mut(&code) {
        Some(r) => r,
        None => return Json(serde_json::json!({"error": "room not found"})),
    };

    // IDEMPOTENCY: a settlement already finalized → return the existing plan. A duplicate
    // /settle with the same (or any) outcome must NOT double-apply.
    if let Some(plan) = room.payout_plan.clone() {
        return Json(serde_json::json!({"settled": true, "payout_plan": plan, "duplicate": true}));
    }

    if !room.game_active && room.settle_pending.is_none() {
        return Json(serde_json::json!({"error": "game not active"}));
    }

    // ── player co-signing gate ──────────────────────────────────────────────
    // Settlement decides who gets the pot. Require BOTH seats to sign the exact outcome with
    // the Ed25519 identity key each pinned on-chain in their deposit memo. The operator holds no
    // signing key here, so it cannot forge a payout. Fail closed. We verify the co-signing FIRST
    // (before the confirmed-deposit guard) so a valid co-signed plan can be QUEUED even when a
    // deposit hasn't confirmed yet — see FIX 1 below.
    if let CosignCheck::Reject(body) = verify_settle_cosign(
        room, &code,
        req.player_a_stack, req.player_b_stack,
        &req.player_a_address, &req.player_b_address,
        &req.action_log_hash, &req.player_a_sig, &req.player_b_sig,
    ) {
        return Json(body);
    }

    // ── 0-conf theft guard (fail-closed) — FIX 1: QUEUE instead of discard ──────────────────
    // The hand may have been DEALT the instant both buy-ins were seen in the mempool
    // (deal_ready = confirmed+pending), but the payout must NEVER move more than the value that
    // actually CONFIRMED into the vault. If either seat's on-chain-confirmed deposit is still
    // short, we do NOT execute the payout. But — unlike before — we also do NOT discard the
    // co-signed outcome (which turned into a terminal PayoutFailed upstream with no retry).
    // Instead we RECORD it as `settle_pending`; the confirmed-deposit scanner completes it
    // (builds the payout plan) the instant both deposits confirm. If a deposit is EVICTED rather
    // than confirmed, the queued settlement is abandoned (`evicted_shortfall`) and funds stay
    // recoverable via /cancel refund-each-own-confirmed or /arbitrate. Money paths stay
    // confirmed-only; see run_deposit_poll's completion hook + run_mempool_watch eviction.
    if !both_deposits_satisfied(room.player_a_deposit, room.player_b_deposit, room.required_deposit) {
        room.settle_pending = Some(PendingSettlement {
            player_a_stack: req.player_a_stack,
            player_b_stack: req.player_b_stack,
            player_a_address: req.player_a_address.clone(),
            player_b_address: req.player_b_address.clone(),
            action_log_hash: req.action_log_hash.clone(),
            player_a_sig: req.player_a_sig.clone(),
            player_b_sig: req.player_b_sig.clone(),
            queued_at: now_ms(),
        });
        // persist the queued co-signed outcome so a restart still completes it on confirmation.
        if let Some(s) = state.persist.as_ref() {
            s.save_room(room);
        }
        journal::record(&code, "settlement_queued_pending_confirmation", serde_json::json!({
            "player_a_stack": req.player_a_stack,
            "player_b_stack": req.player_b_stack,
            "player_a_address": req.player_a_address,
            "player_b_address": req.player_b_address,
            "action_log_hash": req.action_log_hash,
            "player_a_sig": req.player_a_sig,
            "player_b_sig": req.player_b_sig,
            "player_a_deposit": room.player_a_deposit,
            "player_b_deposit": room.player_b_deposit,
            "required_deposit": room.required_deposit,
        }));
        tracing::warn!(
            "settle QUEUED room={} — co-signed outcome accepted but a deposit is unconfirmed \
             (a={} b={} required={}); will execute automatically on confirmation",
            code, room.player_a_deposit, room.player_b_deposit, room.required_deposit,
        );
        return Json(serde_json::json!({
            "settled": false,
            "settle_pending_confirmation": true,
            "message": "settlement accepted and queued — it executes automatically once both \
                        deposits confirm on-chain. no payout moves until then.",
            "player_a_deposit": room.player_a_deposit,
            "player_b_deposit": room.player_b_deposit,
            "required_deposit": room.required_deposit,
        }));
    }

    // both confirmed + co-signed → finalize now.
    let plan = finalize_settlement(
        room, &state.house_address, state.persist.as_ref(), &code,
        req.player_a_stack, req.player_b_stack,
        &req.player_a_address, &req.player_b_address,
        &req.action_log_hash, &req.player_a_sig, &req.player_b_sig,
    );

    Json(serde_json::json!({
        "settled": true,
        "payout_plan": plan,
    }))
}

/// GET /room/{code}/payout -- returns the full payout plan for zafu to build the tx.
/// Available after settlement. The client uses this to construct the Zcash transaction.
async fn get_payout(
    State(state): State<AppState>,
    Path(code): Path<String>,
) -> impl IntoResponse {
    let rooms = state.rooms.lock().await;
    let room = match rooms.get(&code) {
        Some(r) => r,
        None => return Json(serde_json::json!({"error": "room not found"})),
    };

    match &room.payout_plan {
        Some(plan) => Json(serde_json::json!({
            "payout_plan": plan,
            "group_pubkey": hex::encode(&point_to_bytes(&room.group_pubkey)),
        })),
        None => Json(serde_json::json!({"error": "game not settled yet"})),
    }
}

// ── PCZT-based payout (Phase 5.2-LEAVE) ────────────────────────────────────

#[derive(Debug, Deserialize)]
struct InitiatePayoutReq {
    outputs: Vec<PayoutOutputReq>,
    #[serde(default)]
    fee_zat: u64,
    #[serde(default)]
    anchor_height: Option<u32>,
    /// Per-room capability token minted at room creation. Authorizes this payout. May also be
    /// supplied via the `X-Payout-Token` request header; the header takes precedence.
    #[serde(default)]
    payout_token: Option<String>,
}

#[derive(Debug, Deserialize)]
struct PayoutOutputReq {
    address: String,
    amount_zat: u64,
}

async fn initiate_payout(
    State(state): State<AppState>,
    Path(code): Path<String>,
    headers: axum::http::HeaderMap,
    Json(req): Json<InitiatePayoutReq>,
) -> axum::response::Response {
    let header_token = headers
        .get("x-payout-token")
        .and_then(|v| v.to_str().ok())
        .map(str::to_string);

    // snapshot key material + notes; reject if DKG never completed or a payout is already underway
    let snap = {
        let rooms = state.rooms.lock().await;
        let Some(room) = rooms.get(&code) else {
            return json_response(serde_json::json!({"error": "room not found"}));
        };
        // AUTH: bind this money-moving request to the room's creation secret. Fail closed.
        if let Err((status, msg)) = check_payout_token(
            &room.payout_token,
            header_token.as_deref(),
            req.payout_token.as_deref(),
        ) {
            tracing::warn!("payout/initiate REJECTED for {}: {}", code, msg);
            return (status, Json(serde_json::json!({"error": msg}))).into_response();
        }
        match &room.payout_status {
            PayoutStatus::Pending { relay_room } => {
                return json_response(serde_json::json!({"error": "payout already pending", "relay_room": relay_room}));
            }
            PayoutStatus::Broadcast { txid, .. } => {
                return json_response(serde_json::json!({"error": "payout already broadcast", "txid": txid}));
            }
            _ => {}
        }
        match (
            room.dkg_orchard_fvk_hex.as_ref(),
            room.dkg_key_package_hex.as_ref(),
            room.dkg_ephemeral_seed_hex.as_ref(),
            room.dkg_public_key_package_hex.as_ref(),
        ) {
            (Some(fvk), Some(kp), Some(seed), Some(pkg)) => {
                let both_confirmed = both_deposits_satisfied(
                    room.player_a_deposit, room.player_b_deposit, room.required_deposit);
                // Basis for the escrow's OWN refund split (Settle-C1): each seat's pinned refund
                // address paired with its CONFIRMED deposit. Per-seat vector so this generalises
                // beyond 2 seats.
                let refund_basis = room_seat_refunds(room);
                (fvk.clone(), kp.clone(), seed.clone(), pkg.clone(), room.notes.clone(), room.payout_plan.clone(), both_confirmed, refund_basis)
            }
            _ => return json_response(serde_json::json!({"error": "DKG not complete; no key material to sign with"})),
        }
    };
    let (fvk_hex, kp_hex, seed_hex, pkg_hex, notes, settled_plan, both_confirmed, refund_basis) = snap;

    // 0-CONF THEFT GUARD (choke point — every payout flows through here). A co-signed
    // /settle plan already passed the confirmed-deposit guard when it was recorded, so
    // `settled_plan == Some` is safe. But the UNSETTLED path (Leave / abandonment refund,
    // `settled_plan == None`) spends caller-supplied `req.outputs`; it MUST NOT pay out
    // unless BOTH deposits are CONFIRMED on-chain — otherwise a winner could be paid from
    // the honest seat's mempool-only (never-confirmed) deposit (0-conf free-roll theft).
    // Fail-closed: funds stay in the vault; recover via /cancel or /arbitrate refund.
    if settled_plan.is_none() && !both_confirmed {
        tracing::warn!("payout/initiate BLOCKED for {} — unsettled payout while a deposit is unconfirmed", code);
        return json_response(serde_json::json!({
            "error": "unsettled payout blocked: a deposit is not confirmed on-chain — a payout \
                      cannot exceed confirmed funds. settle after confirmation, or use cancel/arbitrate \
                      to refund confirmed deposits.",
            "deposit_pending_confirmation": true,
        }));
    }

    if notes.is_empty() {
        return json_response(serde_json::json!({"error": "no unspent notes — nothing to spend"}));
    }

    // 96-byte FVK
    let fvk_bytes = match hex::decode(&fvk_hex) {
        Ok(b) if b.len() == 96 => {
            let mut a = [0u8; 96];
            a.copy_from_slice(&b);
            a
        }
        _ => return json_response(serde_json::json!({"error": "stored FVK is not 96 bytes"})),
    };

    let mainnet = matches!(state.network, zcash_protocol::consensus::NetworkType::Main);
    // Settle-C1 / HIGH-1: money leaves the escrow by exactly TWO gated paths.
    //   (1) A co-signed /settle (or /arbitrate) plan is recorded → spend the escrow's OWN
    //       `payout_plan` (bound to both seats' signatures, or the operator PIN). The caller's
    //       `req.outputs` are IGNORED here, so a token holder cannot redirect a settled pot.
    //   (2) No co-signed plan → the ONLY permissible unsigned exit is the escrow's OWN
    //       refund-each-seat-its-own-confirmed-deposit split, which the escrow COMPUTES itself.
    //       A caller-chosen winner/amount split (e.g. winner-take-all) is a non-refund exit with
    //       no loser signature and is REJECTED. Winner-take-all without a co-sign must go through
    //       /arbitrate (operator PIN). This closes the un-gated money exit the auditor found.
    let (resolved_outputs, fee_zat): (Vec<(String, u64)>, u64) = match &settled_plan {
        Some(plan) => (
            plan.outputs.iter().map(|o| (o.address.clone(), o.amount)).collect(),
            TX_PAYOUT_FEE_ZAT,
        ),
        None => {
            // The escrow's authoritative refund split — each seat gets back only its own
            // confirmed deposit. This is the single source of the unsigned split.
            let (refund_outputs, _amounts) = compute_refund_outputs(&code, &refund_basis);
            let req_pairs: Vec<(String, u64)> =
                req.outputs.iter().map(|o| (o.address.clone(), o.amount_zat)).collect();
            // If the caller sent no outputs, they are asking for the default refund. Otherwise the
            // caller's outputs MUST equal the escrow's refund split — any other split is a
            // non-refund exit that requires a co-signed /settle or an /arbitrate ruling.
            if !req_pairs.is_empty() && !outputs_match_refund(&req_pairs, &refund_outputs) {
                tracing::warn!("payout/initiate REJECTED for {} — unsigned non-refund split (no co-signed plan)", code);
                return json_response(serde_json::json!({
                    "error": "unsigned payout must be the escrow's refund-each-own-confirmed-deposit split. \
                              a winner/amount split requires a co-signed /settle (both seats) or /arbitrate \
                              (operator PIN). the escrow will not spend a caller-chosen split without a co-sign.",
                    "refund_required": true,
                }));
            }
            if refund_outputs.is_empty() {
                return json_response(serde_json::json!({
                    "error": "nothing to refund: no seat has both a pinned refund address and a confirmed \
                              deposit. use /arbitrate once addresses/deposits are on-chain.",
                }));
            }
            (
                refund_outputs.iter().map(|o| (o.address.clone(), o.amount)).collect(),
                TX_PAYOUT_FEE_ZAT,
            )
        }
    };
    let mut outputs = Vec::with_capacity(resolved_outputs.len());
    for (i, (addr_str, amount_zat)) in resolved_outputs.iter().enumerate() {
        let addr = match tx_build::parse_orchard_ua(addr_str, mainnet) {
            Ok(a) => a,
            Err(e) => return json_response(serde_json::json!({"error": format!("output {} address: {}", i, e)})),
        };
        outputs.push(tx_build::PayoutOutput {
            address: addr,
            amount_zat: *amount_zat,
            memo: [0u8; 512],
        });
    }
    let plan = tx_build::PayoutPlan { outputs, fee_zat };

    let zidecar = match zecli::client::ZidecarClient::connect(&state.zidecar_url).await {
        Ok(c) => c,
        Err(e) => return json_response(serde_json::json!({"error": format!("zidecar connect: {}", e)})),
    };
    let anchor_height = match req.anchor_height {
        Some(h) => h,
        None => match zidecar.get_tip().await {
            Ok((tip, _)) => tip,
            Err(e) => return json_response(serde_json::json!({"error": format!("get_tip: {}", e)})),
        },
    };

    let pczt_result = match tx_build::build_payout_pczt(
        &zidecar, &fvk_bytes, &notes, &plan, anchor_height, mainnet,
    ).await {
        Ok(s) => s,
        Err(e) => return json_response(serde_json::json!({"error": format!("build_pczt: {}", e)})),
    };

    let nick = format!("escrow-payout-{}", code);
    let mut relay = match crate::frost_relay::FrostRelayClient::connect(&state.frost_relay_url, nick).await {
        Ok(c) => c,
        Err(e) => return json_response(serde_json::json!({"error": format!("relay connect: {:?}", e)})),
    };
    let relay_room = match relay.create_room().await {
        Ok(r) => r,
        Err(e) => return json_response(serde_json::json!({"error": format!("relay create_room: {:?}", e)})),
    };

    {
        let mut rooms = state.rooms.lock().await;
        if let Some(room) = rooms.get_mut(&code) {
            room.payout_status = PayoutStatus::Pending { relay_room: relay_room.clone() };
            if let Some(s) = state.persist.as_ref() {
                s.save_room(room);
            }
        }
    }
    tracing::info!("payout initiated for {}: relay_room={} actions={}", code, relay_room, pczt_result.alphas.len());

    let (disp_recipient, disp_amount) = resolved_outputs.iter()
        .find(|(_, amt)| *amt > 0)
        .map(|(addr, amt)| (addr.clone(), *amt))
        .unwrap_or_else(|| (String::new(), 0));

    let bg_rooms = state.rooms.clone();
    let bg_code = code.clone();
    let bg_relay_room = relay_room.clone();
    let bg_zidecar_url = state.zidecar_url.clone();
    let bg_fee_zat = fee_zat;
    let bg_store = state.persist.clone();
    tokio::spawn(async move {
        run_payout_signing(bg_rooms, bg_code, bg_relay_room, bg_zidecar_url, relay,
            pkg_hex, kp_hex, seed_hex, pczt_result,
            disp_recipient, disp_amount, bg_fee_zat, bg_store).await;
    });

    json_response(serde_json::json!({"relay_room": relay_room}))
}

#[allow(clippy::too_many_arguments)]
async fn run_payout_signing(
    rooms: Rooms,
    code: String,
    relay_room: String,
    zidecar_url: String,
    mut relay: crate::frost_relay::FrostRelayClient,
    pkg_hex: String,
    kp_hex: String,
    seed_hex: String,
    pczt_result: crate::tx_build::PcztBuildResult,
    disp_recipient: String,
    disp_amount_zat: u64,
    fee_zat: u64,
    store: Option<persist::Store>,
) {
    let pczt_hex = hex::encode(&pczt_result.pczt_bytes);
    let secrets = crate::payout_signing::PayoutSignSecrets {
        key_package_hex: kp_hex,
        ephemeral_seed_hex: seed_hex,
    };
    let sigs = match crate::payout_signing::host_sign_pczt(
        &mut relay, &pkg_hex, &secrets,
        pczt_result.sighash, &pczt_result.alphas,
        &disp_recipient, disp_amount_zat, fee_zat,
        &pczt_hex,
        std::time::Duration::from_secs(600),
    ).await {
        Ok(s) => s,
        Err(e) => {
            tracing::error!("payout host_sign for {}: {:?}", code, e);
            mark_payout_failed(rooms, code, format!("host_sign_pczt: {:?}", e), store).await;
            return;
        }
    };

    let tx_bytes = match crate::tx_build::complete_payout_pczt(
        &pczt_result.pczt_bytes, &sigs, &pczt_result.spend_indices,
    ) {
        Ok(b) => b,
        Err(e) => {
            tracing::error!("complete_payout_pczt for {}: {}", code, e);
            mark_payout_failed(rooms, code, format!("complete_payout_pczt: {}", e), store).await;
            return;
        }
    };

    let zidecar = match zecli::client::ZidecarClient::connect(&zidecar_url).await {
        Ok(c) => c,
        Err(e) => {
            mark_payout_failed(rooms, code, format!("zidecar connect: {}", e), store).await;
            return;
        }
    };
    match zidecar.send_transaction(tx_bytes).await {
        Ok(res) if res.is_success() => {
            tracing::info!("payout broadcast for {}: tx={}", code, res.txid);
            journal::record(&code, "payout_broadcast", serde_json::json!({
                "txid": res.txid,
                "recipient": disp_recipient,
                "amount_zat": disp_amount_zat,
                "fee_zat": fee_zat,
            }));
            // Positive push so the operator gets a real-time reconciliation signal every time
            // money actually leaves the vault (not only on failures).
            notify::dispute_alert(
                "✅ zk.poker payout broadcast",
                &format!("room {} paid out: {} zat (fee {}) tx={}", code, disp_amount_zat, fee_zat, res.txid),
                "payout",
                &code,
            );
            let mut rl = rooms.lock().await;
            if let Some(room) = rl.get_mut(&code) {
                room.payout_status = PayoutStatus::Broadcast { txid: res.txid, relay_room };
                // funds have LEFT the vault — persist the terminal Broadcast status, then remove
                // the file (remove_room only deletes when status is Broadcast).
                if let Some(s) = store.as_ref() {
                    s.save_room(room);
                    s.remove_room(room);
                }
            }
        }
        Ok(res) => {
            mark_payout_failed(rooms, code,
                format!("zidecar rejected tx ({}): {}", res.error_code, res.error_message), store).await;
        }
        Err(e) => {
            mark_payout_failed(rooms, code, format!("send_transaction: {}", e), store).await;
        }
    }
}

async fn mark_payout_failed(rooms: Rooms, code: String, reason: String, store: Option<persist::Store>) {
    tracing::error!("payout FAILED room={} reason={}", code, reason);
    journal::record(&code, "payout_failed", serde_json::json!({ "reason": reason }));
    notify::dispute_alert(
        "🚨 zk.poker payout failed",
        &format!("room {} payout failed: {}", code, reason),
        "rotating_light",
        &code,
    );
    let mut rl = rooms.lock().await;
    if let Some(room) = rl.get_mut(&code) {
        room.payout_status = PayoutStatus::Failed { reason };
        // persist the Failed status but NEVER remove the file — the vault may still hold funds
        // and a retry (fresh /initiate) needs the key material.
        if let Some(s) = store.as_ref() {
            s.save_room(room);
        }
    }
}

async fn payout_status(
    State(state): State<AppState>,
    Path(code): Path<String>,
) -> impl IntoResponse {
    let rooms = state.rooms.lock().await;
    match rooms.get(&code) {
        Some(room) => Json(serde_json::to_value(&room.payout_status)
            .unwrap_or_else(|_| serde_json::json!({"error": "serialize"}))),
        None => Json(serde_json::json!({"error": "room not found"})),
    }
}

/// FROST signing -- escrow provides its signature share for a transaction.
/// Client sends: sighash + their commitments. Server returns its commitment + share.
#[derive(Deserialize)]
struct SignReq {
    /// which tx: "rake" or "payout"
    tx_type: String,
    /// the sighash to sign (hex)
    sighash: String,
    /// player commitments (JSON from frost_sign_round1)
    player_commitments: String,
}

async fn frost_sign(
    State(state): State<AppState>,
    Path(code): Path<String>,
    Json(req): Json<SignReq>,
) -> impl IntoResponse {
    let mut rooms = state.rooms.lock().await;
    let room = match rooms.get_mut(&code) {
        Some(r) => r,
        None => return Json(serde_json::json!({"error": "room not found"})),
    };

    // validate: only sign rake if deposits confirmed, only sign payout if game ended
    match req.tx_type.as_str() {
        "rake" => {
            if room.player_a_deposit < room.required_deposit || room.player_b_deposit < room.required_deposit {
                return Json(serde_json::json!({"error": "deposits not confirmed"}));
            }
        }
        "payout" => {
            if room.payout_plan.is_none() {
                return Json(serde_json::json!({"error": "game not settled"}));
            }
        }
        "refund" => {
            // refund always allowed
        }
        _ => return Json(serde_json::json!({"error": "unknown tx_type"})),
    }

    // FROST round 1: generate commitment + store nonces for round 2
    let (nonces, commitment) = {
        let mut rng = rand::thread_rng();
        frost::commit(room.server_share.index, &mut rng)
    };
    let commitment_bytes = commitment.to_bytes();

    // store nonces for round 2 (consumed when sign-round2 is called)
    room.pending_nonces = Some(nonces);

    tracing::info!("frost_sign round1: room={} type={} server_idx={}",
        code, req.tx_type, room.server_share.index);

    Json(serde_json::json!({
        "approved": true,
        "server_index": room.server_share.index,
        "server_commitment": hex::encode(&commitment_bytes),
        "tx_type": req.tx_type,
    }))
}

/// FROST round 2 -- server produces its signature share.
/// Matches zafu WASM format: hex-encoded commitments + sighash.
#[derive(Deserialize)]
struct SignRound2Req {
    /// sighash to sign (hex, 32 bytes)
    sighash: String,
    /// all signers' commitments as JSON array of hex strings
    /// each commitment is 68 bytes: [index:4][D:32][E:32]
    commitments: Vec<String>,
}

async fn frost_sign_round2(
    State(state): State<AppState>,
    Path(code): Path<String>,
    Json(req): Json<SignRound2Req>,
) -> impl IntoResponse {
    let mut rooms = state.rooms.lock().await;
    let room = match rooms.get_mut(&code) {
        Some(r) => r,
        None => return Json(serde_json::json!({"error": "room not found"})),
    };

    // consume nonces (one-time use, zeroized on drop)
    let nonces = match room.pending_nonces.take() {
        Some(n) => n,
        None => return Json(serde_json::json!({"error": "no pending nonces -- call /sign first"})),
    };

    // parse sighash
    let message = match hex::decode(&req.sighash) {
        Ok(b) if b.len() == 32 => b,
        _ => return Json(serde_json::json!({"error": "invalid sighash (need 32 bytes hex)"})),
    };

    // parse commitments
    let mut parsed_commitments = Vec::new();
    for c_hex in &req.commitments {
        let bytes = match hex::decode(c_hex) {
            Ok(b) if b.len() == 68 => {
                let mut arr = [0u8; 68];
                arr.copy_from_slice(&b);
                arr
            }
            _ => return Json(serde_json::json!({"error": "invalid commitment (need 68 bytes hex each)"})),
        };
        match osst::frost::SigningCommitments::<pasta_curves::pallas::Point>::from_bytes(&bytes) {
            Ok(c) => parsed_commitments.push(c),
            Err(e) => return Json(serde_json::json!({"error": format!("commitment parse error: {:?}", e)})),
        }
    }

    // build RedPallasPackage
    let package = match frost::RedPallasPackage::new(message, parsed_commitments) {
        Ok(p) => p,
        Err(e) => return Json(serde_json::json!({"error": format!("package error: {:?}", e)})),
    };

    // produce signature share
    let share = match frost::sign(&package, nonces, &room.server_share, &room.group_pubkey) {
        Ok(s) => s,
        Err(e) => return Json(serde_json::json!({"error": format!("signing error: {:?}", e)})),
    };

    let share_bytes = share.to_bytes();

    tracing::info!("frost_sign round2: room={} server_idx={} share={}",
        code, room.server_share.index, hex::encode(&share_bytes[..8]));

    Json(serde_json::json!({
        "signed": true,
        "server_index": room.server_share.index,
        "signature_share": hex::encode(&share_bytes),
    }))
}

async fn health() -> &'static str { "ok" }

/// GET /status — read-only operational snapshot for the operator (loopback-only). Surfaces the
/// config/health that matters at a glance: is persistence on (else a restart strands in-flight
/// funds), is the house payable (rake collectible), verify/DKG/network mode, and live room counts.
async fn get_status(State(state): State<AppState>) -> impl IntoResponse {
    let rooms = state.rooms.lock().await;
    let total = rooms.len();
    let (mut game_active, mut awaiting_deposits, mut pending_settle, mut dkg_failed) = (0u64, 0u64, 0u64, 0u64);
    for r in rooms.values() {
        if r.game_active { game_active += 1; }
        if !both_deposits_satisfied(r.player_a_deposit, r.player_b_deposit, r.required_deposit) { awaiting_deposits += 1; }
        if r.settle_pending.is_some() { pending_settle += 1; }
        if r.dkg_failed.is_some() { dkg_failed += 1; }
    }
    drop(rooms);
    let persistence = state.persist.is_some();
    let house_payable = house_addr_payable(&state.house_address);
    let mainnet = matches!(state.network, zcash_protocol::consensus::NetworkType::Main);
    // Go-live gate for handling REAL money on mainnet: verified deposits, DKG custody, a payable
    // house, durable audit journal, AND persistence (so a restart never strands in-flight funds).
    // Surfaces exactly what's missing so the operator isn't guessing.
    let mut blockers: Vec<&str> = Vec::new();
    if !persistence { blockers.push("persistence_disabled"); }
    if !house_payable { blockers.push("house_not_payable"); }
    if !state.verify_deposits { blockers.push("deposit_verification_off"); }
    if !state.use_dkg { blockers.push("dkg_off"); }
    if !journal::is_enabled() { blockers.push("journal_disabled"); }
    let ready_for_real_money = mainnet && blockers.is_empty();
    Json(serde_json::json!({
        "network": format!("{:?}", state.network),
        "verify_deposits": state.verify_deposits,
        "use_dkg": state.use_dkg,
        "zidecar_url": state.zidecar_url,
        "frost_relay_url": state.frost_relay_url,
        // the operational red flag: persistence OFF means a restart strands in-flight funds.
        "persistence_enabled": persistence,
        "house_payable": house_payable,
        "journal_enabled": journal::is_enabled(),
        // tamper-evidence: false means the audit log's hash chain doesn't verify (altered/truncated).
        "journal_chain_ok": journal::verify_chain().is_ok(),
        // one-glance go-live readiness + exactly what's blocking it.
        "ready_for_real_money": ready_for_real_money,
        "go_live_blockers": blockers,
        "rooms_total": total,
        "rooms_game_active": game_active,
        "rooms_awaiting_deposits": awaiting_deposits,
        "settlements_pending_confirmation": pending_settle,
        "rooms_dkg_failed": dkg_failed,
    }))
}

/// Body for `POST /room/{code}/fault` — a client-detected escrow fault, forwarded by
/// the poker-server relay. Recorded to the durable journal as a `client_fault` event.
#[derive(Deserialize)]
struct FaultReq {
    #[serde(default)]
    seat: Option<u8>,
    /// coarse phase where it broke: "dkg" | "deposit" | "settle" | "payout" | "connect"
    phase: String,
    /// free-text detail (error message / mismatch description)
    detail: String,
}

/// POST /room/{code}/fault — record a client-reported escrow fault to the journal.
/// Fail-soft: always 200 so a reporting client never hangs; the value is the durable
/// record for later triage, not a live control action.
async fn report_fault(
    Path(code): Path<String>,
    Json(req): Json<FaultReq>,
) -> impl IntoResponse {
    tracing::warn!(
        "client_fault room={} seat={:?} phase={} detail={}",
        code, req.seat, req.phase, req.detail,
    );
    journal::record(&code, "client_fault", serde_json::json!({
        "seat": req.seat,
        "phase": req.phase,
        "detail": req.detail,
    }));
    notify::dispute_alert(
        "⚠️ zk.poker fault",
        &format!("room {} — {} fault: {}", code, req.phase, req.detail),
        "warning",
        &code,
    );
    Json(serde_json::json!({ "recorded": true }))
}

/// GET /audit/{code} — the durable dispute bundle for one room: every journalled
/// event in chronological order (room_created, dkg_completed, deposit_detected with
/// txids, settlement_finalized with BOTH player co-signatures, payout_broadcast txid,
/// payout_failed, client_fault). This is what survives a restart and lets a dispute
/// be adjudicated from cryptographic evidence rather than volatile memory.
/// Constant-time byte compare — no token length/timing oracle.
fn ct_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() { return false; }
    let mut diff = 0u8;
    for (x, y) in a.iter().zip(b.iter()) { diff |= x ^ y; }
    diff == 0
}

/// Optional bearer-token gate for the sensitive read endpoints (/accounting = revenue,
/// /audit = player addresses + co-sigs). Open when ESCROW_READ_TOKEN is unset — the endpoints
/// are loopback+firewalled, so this is defense-in-depth for a future re-exposure. 401 on mismatch.
fn check_read_token(headers: &axum::http::HeaderMap) -> Result<(), axum::response::Response> {
    let want = match std::env::var("ESCROW_READ_TOKEN") {
        Ok(t) if !t.trim().is_empty() => t,
        _ => return Ok(()),
    };
    let got = headers
        .get(axum::http::header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.strip_prefix("Bearer "))
        .unwrap_or("");
    if ct_eq(got.as_bytes(), want.trim().as_bytes()) {
        Ok(())
    } else {
        Err((axum::http::StatusCode::UNAUTHORIZED, "read token required").into_response())
    }
}

async fn get_audit(headers: axum::http::HeaderMap, Path(code): Path<String>) -> axum::response::Response {
    if let Err(r) = check_read_token(&headers) { return r; }
    let events = journal::read_room(&code);
    Json(serde_json::json!({
        "code": code,
        "count": events.len(),
        "events": events,
    })).into_response()
}

/// GET /accounting — business rollup over the durable journal: house revenue (Σrake),
/// volume (Σpot), games settled, deposits, payouts, disputes. Read-only aggregate (no
/// addresses/sigs). Counts `settlement_finalized` only — a queued settlement that later
/// completes still emits one `settlement_finalized`, so pots/rake are never double-counted.
async fn get_accounting(headers: axum::http::HeaderMap) -> axum::response::Response {
    if let Err(r) = check_read_token(&headers) { return r; }
    let events = journal::read_all();
    let (mut rooms, mut settled, mut volume, mut rake, mut dep_n, mut dep_v,
         mut pay_ok, mut pay_fail, mut disputes, mut dkg_fail, mut rulings) =
        (0u64, 0u64, 0u64, 0u64, 0u64, 0u64, 0u64, 0u64, 0u64, 0u64, 0u64);
    let (mut first_ms, mut last_ms) = (u64::MAX, 0u64);
    for e in &events {
        if let Some(ts) = e.get("ts").and_then(|t| t.as_u64()) {
            first_ms = first_ms.min(ts);
            last_ms = last_ms.max(ts);
        }
        let d = e.get("data");
        let u = |k: &str| d.and_then(|d| d.get(k)).and_then(|v| v.as_u64()).unwrap_or(0);
        match e.get("kind").and_then(|k| k.as_str()).unwrap_or("") {
            "room_created" => rooms += 1,
            "settlement_finalized" => { settled += 1; volume += u("total_pot"); rake += u("rake"); }
            "deposit_detected" => { dep_n += 1; dep_v += u("value_zat"); }
            "payout_broadcast" => pay_ok += 1,
            "payout_failed" => pay_fail += 1,
            "client_fault" => disputes += 1,
            "dkg_failed" => dkg_fail += 1,
            "arbiter_ruling" => rulings += 1,
            _ => {}
        }
    }
    let zec = |z: u64| z as f64 / 1e8;
    Json(serde_json::json!({
        "events_total": events.len(),
        "first_event_ms": if first_ms == u64::MAX { 0 } else { first_ms },
        "last_event_ms": last_ms,
        "rooms_created": rooms,
        "games_settled": settled,
        "deposits_detected": dep_n,
        "deposits_value_zat": dep_v,
        "deposits_value_zec": zec(dep_v),
        "volume_zat": volume,
        "volume_zec": zec(volume),
        "rake_collected_zat": rake,        // house revenue
        "rake_collected_zec": zec(rake),   // house revenue
        "payouts_broadcast": pay_ok,
        "payouts_failed": pay_fail,
        "disputes": disputes,
        "dkg_failed": dkg_fail,
        "arbiter_rulings": rulings,
    })).into_response()
}

// ── Dispute-resolution dashboard ───────────────────────────────────────────

/// GET /disputes — operator overview page (rooms needing attention first).
async fn get_disputes() -> axum::response::Html<String> {
    axum::response::Html(dispute::render_list(&journal::read_all()))
}

/// GET /dispute/{code} — evidence timeline + PIN + ruling buttons.
async fn get_dispute(Path(code): Path<String>) -> axum::response::Html<String> {
    axum::response::Html(dispute::render_detail(&code, &journal::read_room(&code)))
}

#[derive(Deserialize)]
struct ArbitrateReq {
    pin: String,
    /// "pay_a" | "pay_b" | "refund" | "postpone"
    ruling: String,
}

/// POST /room/{code}/arbitrate — operator ruling on a dispute. PIN-gated (argon2 +
/// per-IP 3-try lockout, fail-closed). `postpone` just records; a pay/refund ruling
/// sets the bound payout plan and fires the existing FROST payout path (which still
/// needs the beneficiary's share to co-sign — the house alone is 1-of-3). Fire-and-
/// forget: a failure self-surfaces as a `payout_failed` journal event + ntfy alert.
async fn arbitrate(
    State(state): State<AppState>,
    Path(code): Path<String>,
    headers: axum::http::HeaderMap,
    Json(req): Json<ArbitrateReq>,
) -> axum::response::Response {
    use axum::http::StatusCode;
    let ip = dispute::client_ip(&headers);

    // ── PIN gate (fail-closed) ──────────────────────────────────────────────
    match pin::check(&ip, &req.pin) {
        pin::PinResult::Ok => {}
        pin::PinResult::Wrong { remaining } => {
            tracing::warn!("arbitrate {} REJECTED — wrong PIN from {} ({} left)", code, ip, remaining);
            return (StatusCode::UNAUTHORIZED,
                Json(serde_json::json!({"error": format!("wrong PIN — {} tries left", remaining)}))).into_response();
        }
        pin::PinResult::Locked { secs } => {
            return (StatusCode::TOO_MANY_REQUESTS,
                Json(serde_json::json!({"error": format!("locked — try again in {}s", secs)}))).into_response();
        }
        pin::PinResult::NotConfigured => {
            return (StatusCode::SERVICE_UNAVAILABLE,
                Json(serde_json::json!({"error": "arbiter PIN not configured on this escrow"}))).into_response();
        }
    }

    // record the ruling first — durable, regardless of what the payout does next.
    journal::record(&code, "arbiter_ruling", serde_json::json!({
        "ruling": req.ruling, "by_ip": ip,
    }));

    if req.ruling == "postpone" {
        return Json(serde_json::json!({"ok": true, "message": "postponed — recorded, no funds moved"})).into_response();
    }

    // ── build the ruled payout plan ─────────────────────────────────────────
    let plan_result = {
        let mut rooms = state.rooms.lock().await;
        let Some(room) = rooms.get_mut(&code) else {
            return (StatusCode::NOT_FOUND, Json(serde_json::json!({"error": "room not found (may be gone after restart)"}))).into_response();
        };
        let total_pot = room.player_a_deposit + room.player_b_deposit;
        if total_pot == 0 {
            return (StatusCode::BAD_REQUEST, Json(serde_json::json!({"error": "no credited deposits to distribute"}))).into_response();
        }
        let addr_a = room.seat_payout_address.get(0).and_then(|a| a.clone());
        let addr_b = room.seat_payout_address.get(1).and_then(|a| a.clone());
        // arbiter rulings charge NO rake — only the on-chain fee is reserved.
        let (a_stack, b_stack, need_a, need_b) = match req.ruling.as_str() {
            "pay_a"  => (1u64, 0u64, true, false),
            "pay_b"  => (0, 1, false, true),
            "refund" => (room.player_a_deposit, room.player_b_deposit, true, true),
            other => return (StatusCode::BAD_REQUEST, Json(serde_json::json!({"error": format!("unknown ruling '{}'", other)}))).into_response(),
        };
        // 0-conf safety (defense-in-depth): pay_a/pay_b award the winner the whole confirmed
        // pot. If a deposit never confirmed, that pot is the OTHER seat's real money — paying it
        // to the winner is theft. Only 'refund' (each seat gets its own confirmed deposit back)
        // is safe under a shortfall. The automatic /settle path enforces the same invariant.
        if (req.ruling == "pay_a" || req.ruling == "pay_b")
            && !both_deposits_satisfied(room.player_a_deposit, room.player_b_deposit, room.required_deposit) {
            return (StatusCode::BAD_REQUEST, Json(serde_json::json!({
                "error": "a deposit is unconfirmed on-chain — pay_a/pay_b could pay the winner from \
                          the other seat's deposit. use 'refund' to return each seat's confirmed deposit.",
                "player_a_deposit": room.player_a_deposit,
                "player_b_deposit": room.player_b_deposit,
                "required_deposit": room.required_deposit,
            }))).into_response();
        }
        if need_a && addr_a.is_none() {
            return (StatusCode::BAD_REQUEST, Json(serde_json::json!({"error": "player A payout address not pinned on-chain — cannot pay"}))).into_response();
        }
        if need_b && addr_b.is_none() {
            return (StatusCode::BAD_REQUEST, Json(serde_json::json!({"error": "player B payout address not pinned on-chain — cannot pay"}))).into_response();
        }
        let (outputs, payout_a, payout_b, _rake) = compute_settlement_outputs(
            &code, total_pot, 0,
            a_stack, b_stack,
            addr_a.as_deref().unwrap_or_default(),
            addr_b.as_deref().unwrap_or_default(),
            &state.house_address,
        );
        room.game_active = false;
        room.final_stacks = Some((payout_a, payout_b));
        let plan = PayoutPlan {
            room: code.clone(),
            escrow_source: hex::encode(&room.escrow_address),
            escrow_value: total_pot,
            outputs,
            action_log_hash: format!("arbiter:{}", req.ruling),
            settled_at: now_ms(),
        };
        room.payout_plan = Some(plan);
        // persist the arbiter-set payout plan so a restart can still pay it out
        if let Some(s) = state.persist.as_ref() {
            s.save_room(room);
        }
        (room.payout_token, payout_a, payout_b)
    };
    let (token, payout_a, payout_b) = plan_result;

    journal::record(&code, "arbiter_payout_plan", serde_json::json!({
        "ruling": req.ruling, "payout_a": payout_a, "payout_b": payout_b,
    }));

    // fire the existing payout path in-process, using the room's own token. It consumes
    // the bound plan we just set (HIGH-1). Completion needs the beneficiary's co-sign.
    let token_hex = hex::encode(token);
    tokio::spawn(async move {
        let mut h = axum::http::HeaderMap::new();
        if let Ok(v) = axum::http::HeaderValue::from_str(&token_hex) {
            h.insert("x-payout-token", v);
        }
        let req = InitiatePayoutReq { outputs: vec![], fee_zat: 0, anchor_height: None, payout_token: None };
        let _ = initiate_payout(State(state), Path(code), h, Json(req)).await;
    });

    Json(serde_json::json!({
        "ok": true,
        "message": "ruling recorded, payout initiated — needs the paid player's wallet online to co-sign (2-of-3)"
    })).into_response()
}

#[derive(Debug, Deserialize)]
struct CancelReq {
    /// Per-room capability token (same secret as `payout/initiate`). May also be supplied via the
    /// `X-Payout-Token` header; the header takes precedence.
    #[serde(default)]
    payout_token: Option<String>,
}

/// POST /room/{code}/cancel — SELF-SERVICE recovery (Settle-C2). Sets the room's `payout_plan` to
/// the escrow's OWN refund-each-seat-its-own-CONFIRMED-deposit split, then fires the normal FROST
/// payout path (which spends that recorded plan — HIGH-1). Token-gated like every money exit.
///
/// This is the `/cancel` route the comments promised: when a co-signed /settle can't complete
/// (e.g. a mempool buy-in was evicted, leaving a 0-conf shortfall), honest confirmed funds no
/// longer strand on operator-PIN availability — either seat's driver can call this to get each
/// player their own confirmed money back.
///
/// SAFE under a shortfall by construction: `compute_refund_outputs` pays each seat only what that
/// seat CONFIRMED, so an unconfirmed/evicted buy-in is simply never in the split — no seat is ever
/// paid from the other's money. NO rake. Idempotent: if a `payout_plan` already exists (a prior
/// cancel/settle/arbitrate), it is returned unchanged. Journaled + persisted like the other plans.
async fn cancel_room(
    State(state): State<AppState>,
    Path(code): Path<String>,
    headers: axum::http::HeaderMap,
    Json(req): Json<CancelReq>,
) -> axum::response::Response {
    use axum::http::StatusCode;
    let header_token = headers
        .get("x-payout-token")
        .and_then(|v| v.to_str().ok())
        .map(str::to_string);

    let plan_result = {
        let mut rooms = state.rooms.lock().await;
        let Some(room) = rooms.get_mut(&code) else {
            return (StatusCode::NOT_FOUND, Json(serde_json::json!({"error": "room not found"}))).into_response();
        };
        // AUTH: bind this money-moving request to the room's creation secret. Fail closed.
        if let Err((status, msg)) = check_payout_token(
            &room.payout_token, header_token.as_deref(), req.payout_token.as_deref(),
        ) {
            tracing::warn!("cancel REJECTED for {}: {}", code, msg);
            return (status, Json(serde_json::json!({"error": msg}))).into_response();
        }

        // IDEMPOTENCY: a plan already exists (settle / arbitrate / a prior cancel) → return it
        // unchanged. Never rebuild or double-journal.
        if let Some(plan) = room.payout_plan.clone() {
            return Json(serde_json::json!({
                "ok": true, "refund_plan": plan, "duplicate": true,
                "message": "a payout/refund plan already exists for this room — returned unchanged",
            })).into_response();
        }

        // Build the escrow's OWN refund split from each seat's CONFIRMED deposit + pinned address.
        let seats = room_seat_refunds(room);
        let (outputs, amounts) = compute_refund_outputs(&code, &seats);
        let total_confirmed: u64 = seats.iter().map(|s| s.confirmed_deposit).sum();
        if outputs.is_empty() {
            return (StatusCode::BAD_REQUEST, Json(serde_json::json!({
                "error": "nothing to refund: no seat has both a pinned refund address and a confirmed \
                          deposit above the tx fee.",
                "total_confirmed": total_confirmed,
            }))).into_response();
        }

        room.game_active = false;
        // final_stacks mirrors the refund amounts (seat 0 = A, seat 1 = B) for the accounting view.
        room.final_stacks = Some((amounts.first().copied().unwrap_or(0), amounts.get(1).copied().unwrap_or(0)));
        let plan = PayoutPlan {
            room: code.clone(),
            escrow_source: hex::encode(&room.escrow_address),
            escrow_value: total_confirmed,
            outputs,
            action_log_hash: "cancel:refund-each-own-confirmed".to_string(),
            settled_at: now_ms(),
        };
        room.payout_plan = Some(plan.clone());
        // a queued co-signed settlement (if any) is superseded by the refund — clear it so the
        // scanner won't also try to complete it.
        room.settle_pending = None;
        if let Some(s) = state.persist.as_ref() {
            s.save_room(room);
        }
        (room.payout_token, plan, amounts)
    };
    let (token, plan, amounts) = plan_result;

    journal::record(&code, "cancel_refund_plan", serde_json::json!({
        "refund_amounts": amounts,
        "escrow_value": plan.escrow_value,
        "outputs": plan.outputs.len(),
    }));
    tracing::info!("cancel: room={} refund plan set ({} outputs, {} zat) — firing payout", code, plan.outputs.len(), plan.escrow_value);

    // fire the existing payout path in-process, using the room's own token. It consumes the bound
    // refund plan we just set (HIGH-1). Completion still needs a beneficiary's share to co-sign.
    let token_hex = hex::encode(token);
    let bg_state = state.clone();
    let bg_code = code.clone();
    tokio::spawn(async move {
        let mut h = axum::http::HeaderMap::new();
        if let Ok(v) = axum::http::HeaderValue::from_str(&token_hex) {
            h.insert("x-payout-token", v);
        }
        let ireq = InitiatePayoutReq { outputs: vec![], fee_zat: 0, anchor_height: None, payout_token: None };
        let _ = initiate_payout(State(bg_state), Path(bg_code), h, Json(ireq)).await;
    });

    Json(serde_json::json!({
        "ok": true,
        "refund_plan": plan,
        "message": "cancelled — refunding each seat its own confirmed deposit; needs a wallet online to co-sign (2-of-3)",
    })).into_response()
}

// ---------------------------------------------------------------------------
// Background deposit monitor
// ---------------------------------------------------------------------------

/// Runs every 5 seconds. For each room with pending deposits, checks if both
/// players have met the required deposit and auto-activates the game.
///
/// When ESCROW_VERIFY_DEPOSITS=true (future), this would also poll zidecar
/// to trial-decrypt incoming Orchard notes for the escrow address.
async fn deposit_monitor(state: AppState) {
    let mut interval = tokio::time::interval(std::time::Duration::from_secs(5));
    loop {
        interval.tick().await;

        let mut rooms = state.rooms.lock().await;
        let mut activated = Vec::new();

        for (code, room) in rooms.iter_mut() {
            if room.game_active {
                continue;
            }

            // DEAL GATE: the game may START as soon as both seats are covered by CONFIRMED +
            // PENDING (mempool-seen) deposits, so players don't wait ~75s for a block. This is the
            // ONLY place pending is allowed to count. Settlement / rake / payout stay CONFIRMED-only.
            let both = deal_ready(
                room.player_a_deposit,
                room.player_a_deposit_pending,
                room.player_b_deposit,
                room.player_b_deposit_pending,
                room.required_deposit,
            );

            if both {
                room.game_active = true;
                if let Some(s) = state.persist.as_ref() {
                    s.save_room(room);
                }
                activated.push(code.clone());
            } else if state.verify_deposits && (room.player_a_deposit > 0 || room.player_b_deposit > 0) {
                // future: poll zidecar for new notes at this escrow address
                // let notes = poll_zidecar_notes(&state.zidecar_url, &room.escrow_address).await;
                tracing::debug!("monitor: room={} waiting (A={}/{} B={}/{})",
                    code, room.player_a_deposit, room.required_deposit,
                    room.player_b_deposit, room.required_deposit);
            }
        }

        for code in activated {
            tracing::info!("monitor: room={} both deposits confirmed -- game_active=true", code);
        }
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// The gate that flips an escrow room to `game_active`: both seats have funded at least the
/// required deposit. Pure helper so the rule is unit-testable without a full `EscrowRoom`.
fn both_deposits_satisfied(a_deposit: u64, b_deposit: u64, required: u64) -> bool {
    a_deposit >= required && b_deposit >= required
}

/// DEAL gate: the game may START once both seats are covered by CONFIRMED + PENDING deposits.
/// This is the ONLY gate allowed to count mempool-seen (pending) value; every money path
/// (settlement / rake / payout) uses the confirmed counters via `both_deposits_satisfied`.
/// Kept as a pure fn so the confirmed-vs-pending rule is unit-testable in isolation.
fn deal_ready(a_conf: u64, a_pend: u64, b_conf: u64, b_pend: u64, required: u64) -> bool {
    both_deposits_satisfied(
        a_conf.saturating_add(a_pend),
        b_conf.saturating_add(b_pend),
        required,
    )
}

fn share_to_bytes(share: &osst::SecretShare<PallasScalar>) -> [u8; 36] {
    let mut buf = [0u8; 36];
    buf[0..4].copy_from_slice(&share.index.to_le_bytes());
    use pasta_curves::group::ff::PrimeField;
    let scalar_bytes = share.scalar().to_repr();
    buf[4..36].copy_from_slice(scalar_bytes.as_ref());
    buf
}

fn point_to_bytes(point: &pasta_curves::pallas::Point) -> [u8; 32] {
    use pasta_curves::group::GroupEncoding;
    let compressed = point.to_bytes();
    compressed.into()
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis() as u64
}

/// Wrap a JSON value in a 200 OK response. Used by handlers that return `Response` so their
/// error/success bodies stay uniform.
fn json_response(v: serde_json::Value) -> axum::response::Response {
    Json(v).into_response()
}

/// Mint a fresh 256-bit per-room capability token from the OS CSPRNG. Returned once to the room
/// creator; required to authorize money-moving endpoints (payout initiation).
fn new_payout_token() -> [u8; 32] {
    use rand::RngCore;
    let mut t = [0u8; 32];
    rand::thread_rng().fill_bytes(&mut t);
    t
}

/// Constant-time equality for the 32-byte payout token. Avoids leaking how many leading bytes
/// of a guessed token were correct via response timing.
fn ct_eq_token(a: &[u8; 32], b: &[u8; 32]) -> bool {
    let mut diff = 0u8;
    for i in 0..32 {
        diff |= a[i] ^ b[i];
    }
    diff == 0
}

/// Extract the caller-supplied payout token from either the `X-Payout-Token` header or the
/// request body's `payout_token` field, decode the hex, and constant-time compare against the
/// room's minted token. Returns `Ok(())` on match; `Err(status, message)` otherwise. Fails
/// closed: a missing, malformed, or mismatched token is rejected.
fn check_payout_token(
    expected: &[u8; 32],
    header_token: Option<&str>,
    body_token: Option<&str>,
) -> Result<(), (axum::http::StatusCode, &'static str)> {
    use axum::http::StatusCode;
    let supplied = header_token.or(body_token).map(str::trim).filter(|s| !s.is_empty());
    let Some(supplied) = supplied else {
        return Err((StatusCode::UNAUTHORIZED, "missing payout token"));
    };
    let bytes = hex::decode(supplied).map_err(|_| (StatusCode::UNAUTHORIZED, "malformed payout token"))?;
    if bytes.len() != 32 {
        return Err((StatusCode::UNAUTHORIZED, "payout token wrong length"));
    }
    let mut got = [0u8; 32];
    got.copy_from_slice(&bytes);
    if ct_eq_token(expected, &got) {
        Ok(())
    } else {
        Err((StatusCode::FORBIDDEN, "invalid payout token"))
    }
}

// ---------------------------------------------------------------------------
// Main
// ---------------------------------------------------------------------------

/// Args resolve in order: CLI flag → env var → default. Values also load from a `.env`
/// in the working directory at startup (silent if absent).
#[derive(clap::Parser, Debug)]
#[command(version, about = "Poker escrow — FROST 2-of-3 key management, deposit tracking, payout signing")]
struct Args {
    /// House rake address (placeholder until prod wallet wired)
    #[arg(long, env = "HOUSE_ADDRESS", default_value = "ztestsapling1...")]
    house_address: String,
    /// zidecar gRPC-web endpoint
    #[arg(long, env = "ZIDECAR_URL", default_value = "https://zcash.rotko.net")]
    zidecar_url: String,
    /// On-chain deposit verification (vs. self-reported counters)
    #[arg(long, env = "ESCROW_VERIFY_DEPOSITS", default_value_t = false)]
    verify_deposits: bool,
    /// Network: `main` / `test` / `regtest`
    #[arg(long, env = "ESCROW_NETWORK", default_value = "test")]
    network: String,
    /// Use DKG mode (vs. trusted-dealer) for key generation
    #[arg(long, env = "ESCROW_USE_DKG", default_value_t = false)]
    use_dkg: bool,
    /// FROST relay WebSocket URL (used for DKG + payout signing)
    #[arg(long, env = "ESCROW_RELAY_URL", default_value = "ws://127.0.0.1:50053/ws")]
    relay_url: String,
    /// HTTP port to bind
    #[arg(long, env = "ESCROW_PORT", default_value_t = 3034)]
    port: u16,
    /// HTTP bind host. Defaults to 127.0.0.1 (LOOPBACK): the escrow is an INTERNAL service the
    /// relay reaches over localhost, and it serves revenue/audit/room data that must NOT be
    /// public. Only set 0.0.0.0 if you have deliberately firewalled the port / front it with an
    /// authenticated reverse proxy.
    #[arg(long, env = "ESCROW_BIND", default_value = "127.0.0.1")]
    bind: String,
    /// Durable event-journal file (dispute/audit trail). Empty = disabled.
    #[arg(long, env = "ESCROW_JOURNAL", default_value = "journal/events.jsonl")]
    journal: String,
    /// Directory holding per-room persisted key material + payout state. MONEY-CRITICAL:
    /// without it, a restart strands funds in any room's vault forever. Empty = disabled.
    #[arg(long, env = "ESCROW_STATE_DIR", default_value = "state")]
    state_dir: String,
    /// ntfy topic URL for dispute alerts (empty = no push). e.g. https://ntfy.rotko.net/zkpoker-disputes
    #[arg(long, env = "ESCROW_NTFY_URL", default_value = "")]
    ntfy_url: String,
    /// Bearer token for an auth-protected ntfy server (ntfy.rotko.net requires it).
    #[arg(long, env = "ESCROW_NTFY_TOKEN", default_value = "")]
    ntfy_token: String,
    /// Public base URL for dashboard click-through links in alerts.
    #[arg(long, env = "ESCROW_DASHBOARD_BASE", default_value = "https://zkbtc.org")]
    dashboard_base: String,
    /// argon2 hash of the operator PIN (gates the dispute dashboard). Empty = actions disabled.
    #[arg(long, env = "ESCROW_ARBITER_PIN_HASH", default_value = "")]
    arbiter_pin_hash: String,
    /// Helper: print the argon2 hash of the given PIN and exit (for ESCROW_ARBITER_PIN_HASH).
    #[arg(long)]
    hash_pin: Option<String>,
}

#[tokio::main]
async fn main() {
    let _ = dotenvy::dotenv();
    let args = <Args as clap::Parser>::parse();

    // helper mode: `poker-escrow --hash-pin 123456` prints the argon2 hash and exits.
    if let Some(pin) = args.hash_pin {
        match pin::hash_pin(&pin) {
            Ok(h) => { println!("{}", h); std::process::exit(0); }
            Err(e) => { eprintln!("hash-pin failed: {}", e); std::process::exit(1); }
        }
    }

    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env()
            .add_directive("poker_escrow=info".parse().unwrap()))
        .init();

    // durable per-room key-material store — the whole point of this crate surviving a restart
    let store = persist::Store::open(&args.state_dir);

    let state = AppState {
        rooms: Arc::new(Mutex::new(HashMap::new())),
        house_address: args.house_address,
        zidecar_url: args.zidecar_url,
        verify_deposits: args.verify_deposits,
        network: orchard_ua::network_from_str(&args.network),
        use_dkg: args.use_dkg,
        frost_relay_url: args.relay_url,
        persist: store,
    };

    tracing::info!("house address: {}", state.house_address);
    // Validate the house address at BOOT (not mid-settlement) so a typo/wrong-pool/placeholder
    // address is surfaced immediately. house_addr_payable now actually decodes it as an
    // Orchard-capable UA. An unpayable house never breaks a winner's payout (rake is just
    // dropped), but the operator should know up-front whether rake will actually be collected.
    if state.house_address.trim().is_empty() {
        tracing::warn!("house address UNSET — rake DISABLED (set HOUSE_ADDRESS to collect rake)");
    } else if house_addr_payable(&state.house_address) {
        tracing::info!("house address VALIDATED — payable Orchard UA, rake collectible");
    } else {
        tracing::warn!(
            "house address is a placeholder or NOT an Orchard-capable UA — rake DISABLED \
             (winner payouts unaffected; set a real HOUSE_ADDRESS to collect rake)"
        );
    }
    tracing::info!("zidecar: {}", state.zidecar_url);
    tracing::info!("verify_deposits: {}", state.verify_deposits);
    tracing::info!("network: {:?}", state.network);
    tracing::info!("use_dkg: {} (relay: {})", state.use_dkg, state.frost_relay_url);

    // FAIL-CLOSED guardrail: never run on MAINNET while trusting self-reported deposits. With
    // verify_deposits=false the HTTP /deposit path credits a caller's claimed amount without an
    // on-chain check — on mainnet that is instant theft (deal + settle against ZEC never sent).
    // A real-money escrow with this combo is a misconfiguration; refuse to start rather than
    // custody funds unsafely. (Testnet/demo may run trusted-dealer mode.)
    let mainnet = matches!(state.network, zcash_protocol::consensus::NetworkType::Main);
    if mainnet && !state.verify_deposits {
        tracing::error!(
            "REFUSING TO START: network=Main but verify_deposits=false — self-reported deposits \
             would be credited with REAL ZEC. Set ESCROW_VERIFY_DEPOSITS=true for mainnet."
        );
        std::process::exit(1);
    }
    // Sibling guardrail: the settle path honours ESCROW_ALLOW_UNSIGNED_SETTLE=1 as a DEMO bypass of
    // the player co-signing gate. On mainnet that lets the operator settle/pay out WITHOUT both
    // players' signatures — i.e. forge payouts against real ZEC. Never allow the bypass on mainnet.
    if mainnet && std::env::var("ESCROW_ALLOW_UNSIGNED_SETTLE").ok().as_deref() == Some("1") {
        tracing::error!(
            "REFUSING TO START: network=Main with ESCROW_ALLOW_UNSIGNED_SETTLE=1 — settlements would \
             bypass player co-signing (forgeable payouts). Unset it for mainnet."
        );
        std::process::exit(1);
    }

    // durable dispute/audit journal — survives restart (unlike in-memory room state)
    if !args.journal.trim().is_empty() {
        journal::init(&args.journal);
        tracing::info!("journal: {}", args.journal);
    }

    // dispute alerting + operator-PIN gate for the dashboard
    notify::init(
        Some(args.ntfy_url.clone()).filter(|u| !u.trim().is_empty()),
        args.dashboard_base.clone(),
        Some(args.ntfy_token.clone()).filter(|t| !t.trim().is_empty()),
    );
    pin::init(Some(args.arbiter_pin_hash.clone()).filter(|h| !h.trim().is_empty()));

    // restore persisted rooms BEFORE the server accepts requests, so a restart can still
    // co-sign payouts for vaults that hold funds. Rooms are inserted into the in-memory map;
    // for any DKG-complete room that has not yet settled we ALSO resume its deposit-poll
    // scanner (from the persisted cursor, not the tip) so deposits made during downtime are
    // still credited.
    if let Some(store) = state.persist.as_ref() {
        let restored = store.load_all();
        let n = restored.len();
        // (code, fvk_hex, seat_addr_bytes) for rooms whose deposit scanner should resume.
        let mut to_resume: Vec<(String, String, Vec<Option<[u8; 43]>>)> = Vec::new();
        let mut rooms = state.rooms.lock().await;
        for room in restored {
            // Resume scanning only while the room is still taking deposits: DKG material
            // present and no payout plan committed yet. Settled/paid rooms don't need it.
            if room.payout_plan.is_none() {
                if let Some(fvk_hex) = room.dkg_orchard_fvk_hex.clone() {
                    to_resume.push((room.code.clone(), fvk_hex, room.seat_addr_bytes.clone()));
                }
            }
            rooms.insert(room.code.clone(), room);
        }
        drop(rooms);
        if n > 0 {
            tracing::info!("persist: restored {} room(s) from disk", n);
        } else {
            tracing::info!("persist: no rooms to restore");
        }
        for (code, fvk_hex, seat_addr_bytes) in to_resume {
            tracing::info!("persist: resuming deposit scanner for restored room {}", code);
            dkg_room::resume_deposit_poll(
                state.rooms.clone(),
                code,
                state.zidecar_url.clone(),
                fvk_hex,
                seat_addr_bytes,
                state.house_address.clone(),
                state.persist.clone(),
            );
        }
    }

    // spawn background deposit monitor (checks every 5s)
    tokio::spawn(deposit_monitor(state.clone()));

    let app = Router::new()
        .route("/room", axum::routing::post(create_room))
        .route("/room/{code}", axum::routing::get(get_room))
        .route("/room/{code}/deposit", axum::routing::post(report_deposit))
        .route("/room/{code}/settle", axum::routing::post(settle))
        .route("/room/{code}/payout", axum::routing::get(get_payout))
        .route("/room/{code}/payout/initiate", axum::routing::post(initiate_payout))
        .route("/room/{code}/payout/status", axum::routing::get(payout_status))
        .route("/room/{code}/sign", axum::routing::post(frost_sign))
        .route("/room/{code}/sign-round2", axum::routing::post(frost_sign_round2))
        .route("/room/{code}/fault", axum::routing::post(report_fault))
        .route("/room/{code}/arbitrate", axum::routing::post(arbitrate))
        .route("/room/{code}/cancel", axum::routing::post(cancel_room))
        .route("/audit/{code}", axum::routing::get(get_audit))
        .route("/accounting", axum::routing::get(get_accounting))
        .route("/disputes", axum::routing::get(get_disputes))
        .route("/dispute/{code}", axum::routing::get(get_dispute))
        .route("/health", axum::routing::get(health))
        .route("/status", axum::routing::get(get_status))
        .with_state(state);

    let addr = format!("{}:{}", args.bind, args.port);
    if args.bind == "0.0.0.0" || args.bind == "::" {
        tracing::warn!("poker-escrow bound to {} — PUBLICLY reachable unless firewalled; it serves \
                        /accounting, /audit and room state. Prefer ESCROW_BIND=127.0.0.1.", args.bind);
    }
    tracing::info!("poker-escrow listening on http://{}", addr);

    let listener = tokio::net::TcpListener::bind(addr).await.unwrap();
    axum::serve(listener, app).await.unwrap();
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::StatusCode;
    use std::collections::HashSet;

    #[test]
    fn ct_eq_is_correct_for_auth_compare() {
        // a bug here would be an auth-bypass on the read-token gate.
        assert!(ct_eq(b"", b""));
        assert!(ct_eq(b"secret-token", b"secret-token"));
        assert!(!ct_eq(b"secret-token", b"secret-toke"));   // length differs
        assert!(!ct_eq(b"secret-token", b"secret-tokeX"));   // last byte differs
        assert!(!ct_eq(b"Xecret-token", b"secret-token"));   // first byte differs
        assert!(!ct_eq(b"token", b""));                      // empty vs non-empty
        assert!(!ct_eq(b"", b"token"));
    }

    // ── settlement split arithmetic (HIGH-1/HIGH-2 fix) ────────────────────────
    // The critical invariant for a payout that BALANCES on-chain:
    //   sum(outputs) + TX_PAYOUT_FEE_ZAT == total_pot
    // i.e. player payouts + rake + tx fee exactly consume the pot (deposits), so the
    // payout tx has zero change and never under/over-funds. Players pre-fund the fee.
    fn sum_outputs(outs: &[PayoutOutput]) -> u64 { outs.iter().map(|o| o.amount).sum() }

    #[test]
    fn settlement_balances_exactly_no_rake() {
        // heads-up: each seat deposited buyin(100k) + fee_per_seat(5k) => pot 210k
        let pot = 210_000;
        let (outs, a, b, rake) = compute_settlement_outputs("R", pot, 0, 1500, 500, "u1a", "u1b", "u1house");
        assert_eq!(rake, 0);
        assert_eq!(a + b, pot - TX_PAYOUT_FEE_ZAT);           // distributable = pot - fee
        assert_eq!(sum_outputs(&outs) + TX_PAYOUT_FEE_ZAT, pot); // balances
        assert_eq!(outs.len(), 2);                            // A + B, no rake output
        assert!(a > b);                                       // A had the bigger stack
    }

    #[test]
    fn settlement_winner_take_all_skips_zero_output() {
        let pot = 210_000;
        let (outs, a, b, _) = compute_settlement_outputs("R", pot, 0, 2000, 0, "u1a", "u1b", "u1house");
        assert_eq!(a, pot - TX_PAYOUT_FEE_ZAT);
        assert_eq!(b, 0);
        assert_eq!(outs.len(), 1);                            // only the winner's output
        assert_eq!(sum_outputs(&outs) + TX_PAYOUT_FEE_ZAT, pot);
    }

    // a REAL, decodable mainnet Orchard UA (house_addr_payable now actually decodes, so a
    // gibberish fixture would be — correctly — treated as unpayable and charge 0 rake).
    const HOUSE_OK: &str = "u14hnh66qv4qhy8psewqkljghznh265rna5k77zwy6dvyyqq6x6r8dum59h4l0krpl0m5jhyjnj5szkjyj2k836fkvtqst6eqeg5z68lve";

    #[test]
    fn settlement_with_rake_balances_and_pays_house() {
        let pot = 210_000;
        let (outs, a, b, rake) = compute_settlement_outputs("R", pot, 100, 1000, 1000, "u1a", "u1b", HOUSE_OK);
        assert_eq!(rake, pot * 100 / 10_000);                 // 1%
        // distributable = pot - rake - fee, split evenly
        assert_eq!(a + b, pot - rake - TX_PAYOUT_FEE_ZAT);
        assert_eq!(sum_outputs(&outs) + TX_PAYOUT_FEE_ZAT, pot); // A + B + rake + fee = pot
        assert_eq!(outs.len(), 3);                            // A, B, house
        assert!(outs.iter().any(|o| o.address == HOUSE_OK && o.amount == rake));
    }

    #[test]
    fn placeholder_house_charges_no_rake_and_never_strands_funds() {
        // With an unconfigured / placeholder house address, a rake-bearing table must
        // NOT emit a house output (which would break the payout tx on an invalid
        // recipient). Instead rake→0 and the value folds back to the players.
        let pot = 210_000;
        for bad in ["u1placeholderhouse", "demo_house_addr", "", "u1house", "ztestsapling1..."] {
            let (outs, a, b, rake) = compute_settlement_outputs("R", pot, 250, 1000, 1000, "u1a", "u1b", bad);
            assert_eq!(rake, 0, "house '{}' must yield 0 rake", bad);
            assert_eq!(outs.len(), 2, "house '{}' must not emit a rake output", bad);
            // full pot (minus fee) still goes to the players — nothing stranded
            assert_eq!(a + b, pot - TX_PAYOUT_FEE_ZAT);
            assert_eq!(sum_outputs(&outs) + TX_PAYOUT_FEE_ZAT, pot);
        }
    }

    #[test]
    fn house_addr_payable_requires_decodable_orchard_ua() {
        // real mainnet Orchard UA decodes → payable
        assert!(house_addr_payable(HOUSE_OK));
        // rake is an ORCHARD output, so none of these can receive it (or don't decode) → unpayable,
        // rake is dropped, winner payout is never broken:
        assert!(!house_addr_payable("u1placeholderhouse"));                       // placeholder
        assert!(!house_addr_payable("demo_house_addr"));                          // demo marker
        assert!(!house_addr_payable(""));                                         // empty
        assert!(!house_addr_payable("u1house"));                                  // too short
        assert!(!house_addr_payable("t1transparentaddressnotsupported12345"));    // transparent
        assert!(!house_addr_payable("ztestsapling1qqqqqqqqqqqqqqqqqqqqqqqqqqqqreal")); // sapling-only
        // right prefix + realistic length but garbage body → must NOT decode as Orchard:
        assert!(!house_addr_payable("u1garbagenotarealunifiedaddress00000000000000000000000000000000000000"));
    }

    #[test]
    fn settlement_even_split_no_truncation_loss() {
        // odd distributable must not lose a zatoshi: payout_b is the exact remainder
        let pot = 30_001 + TX_PAYOUT_FEE_ZAT; // distributable = 30_001 (odd)
        let (_outs, a, b, _) = compute_settlement_outputs("R", pot, 0, 1, 1, "u1a", "u1b", "u1h");
        assert_eq!(a + b, pot - TX_PAYOUT_FEE_ZAT);           // no zatoshi stranded
        assert_eq!(a, 15_000);
        assert_eq!(b, 15_001);                               // remainder to B
    }

    #[test]
    fn deposits_gate_requires_both_seats() {
        let required = 15_000;
        assert!(!both_deposits_satisfied(0, 0, required));
        assert!(!both_deposits_satisfied(15_000, 0, required));
        assert!(!both_deposits_satisfied(0, 15_000, required));
        assert!(both_deposits_satisfied(15_000, 15_000, required));
    }

    #[test]
    fn deposits_gate_allows_overpayment_blocks_underpayment() {
        let required = 15_000;
        assert!(!both_deposits_satisfied(14_999, 15_000, required));
        assert!(both_deposits_satisfied(15_001, 20_000, required));
    }

    #[test]
    fn settle_guard_blocks_0conf_theft() {
        // The 0-conf attack the /settle + arbitrate guards defend against: attacker A's deposit
        // is only ever SEEN in the mempool (enough for deal_ready to deal the hand) but never
        // confirms — a double-spend/RBF. B's deposit confirms for real. A wins the hand.
        // At settlement the CONFIRMED counters are a=0, b=required: the vault holds only B's
        // money. The guard (both_deposits_satisfied over CONFIRMED) must be false so /settle
        // refuses — otherwise A would be paid out of B's deposit (free-roll theft).
        let required = 15_000;
        // attacker unconfirmed, honest confirmed -> guard shut (settle refused, funds safe)
        assert!(!both_deposits_satisfied(0, required, required), "A never confirmed → must block");
        assert!(!both_deposits_satisfied(required, 0, required), "B never confirmed → must block");
        // both actually confirmed (the normal case: a block lands mid-hand) -> guard opens
        assert!(both_deposits_satisfied(required, required, required), "both confirmed → allow payout");
    }

    // ── MEMPOOL (0-conf) pending-deposit lifecycle ────────────────────────────
    //
    // Exercises the exact pending-ledger arithmetic the mempool watcher and the confirmed
    // credit path perform (credit / promote / evict), and proves the two gates diverge:
    //   - DEAL gate  (`deal_ready`)             = CONFIRMED + PENDING  → opens on 0-conf.
    //   - MONEY gate (`both_deposits_satisfied`) = CONFIRMED only       → stays shut on 0-conf.
    #[test]
    fn pending_deposit_lifecycle() {
        let required = 15_000u64;

        // per-seat (confirmed, pending) ledgers; both seats start empty.
        let (mut a_conf, mut a_pend) = (0u64, 0u64);
        let (mut b_conf, mut b_pend) = (0u64, 0u64);

        // helper closures mirroring the watcher / credit-path arithmetic ────────
        // (a) mempool note credits PENDING, never confirmed.
        let credit_pending = |pend: &mut u64, v: u64| *pend = pend.saturating_add(v);
        // (c) confirmation: confirmed += v, and if it was pending, pending -= v (promotion).
        let promote = |conf: &mut u64, pend: &mut u64, v: u64, was_pending: bool| {
            *conf = conf.saturating_add(v);
            if was_pending {
                *pend = pend.saturating_sub(v);
            }
        };
        // (d) eviction: a mempool note that left without confirming reverses its pending credit.
        let evict = |pend: &mut u64, v: u64| *pend = pend.saturating_sub(v);

        // 1. both seats' deposits appear in the MEMPOOL.
        credit_pending(&mut a_pend, required);
        credit_pending(&mut b_pend, required);

        // (a) pending credited, confirmed untouched.
        assert_eq!((a_conf, a_pend), (0, required));
        assert_eq!((b_conf, b_pend), (0, required));

        // (b) DEAL gate opens on pending alone…
        assert!(deal_ready(a_conf, a_pend, b_conf, b_pend, required), "deal gate opens on pending");
        // (e) …but the SETTLEMENT/money gate stays shut (confirmed still 0).
        assert!(!both_deposits_satisfied(a_conf, b_conf, required), "money gate shut until confirmed");

        // 2. seat A's note CONFIRMS in a block (promotion): confirmed += v, pending -= v.
        promote(&mut a_conf, &mut a_pend, required, /*was_pending=*/ true);
        // (c) total (confirmed+pending) for A is stable across the transition.
        assert_eq!(a_conf + a_pend, required, "confirmed+pending stable across promotion");
        assert_eq!((a_conf, a_pend), (required, 0));
        // deal gate still open (A confirmed, B still pending).
        assert!(deal_ready(a_conf, a_pend, b_conf, b_pend, required));
        // money gate still shut — B not yet confirmed.
        assert!(!both_deposits_satisfied(a_conf, b_conf, required));

        // 3. seat B's mempool note DROPS OUT without confirming (eviction, past grace window).
        evict(&mut b_pend, required);
        assert_eq!((b_conf, b_pend), (0, 0));
        // (d) deal gate CLOSES again — B no longer covered by confirmed+pending.
        assert!(!deal_ready(a_conf, a_pend, b_conf, b_pend, required), "deal gate closes on eviction");

        // 4. B re-broadcasts, is seen in mempool, then confirms.
        credit_pending(&mut b_pend, required);
        assert!(deal_ready(a_conf, a_pend, b_conf, b_pend, required));
        promote(&mut b_conf, &mut b_pend, required, /*was_pending=*/ true);
        assert_eq!(b_conf + b_pend, required, "B confirmed+pending stable across promotion");

        // (e) NOW both are confirmed → the money gate finally opens.
        assert!(both_deposits_satisfied(a_conf, b_conf, required), "money gate opens once both confirmed");
        assert_eq!((a_pend, b_pend), (0, 0), "no phantom pending left after both confirm");
    }

    // ── BUG 1: payout auth ────────────────────────────────────────────────

    #[test]
    fn payout_token_missing_is_rejected() {
        let expected = new_payout_token();
        // no header, no body token — an anonymous caller who only knows the (public) room code
        let res = check_payout_token(&expected, None, None);
        assert_eq!(res.unwrap_err().0, StatusCode::UNAUTHORIZED);
    }

    // ── player co-signed settlement ──────────────────────────────────────────

    #[test]
    fn settlement_sig_roundtrip_and_tamper() {
        use ed25519_dalek::{SigningKey, Signer};
        let sk = SigningKey::from_bytes(&[7u8; 32]);
        let pk: [u8; 32] = sk.verifying_key().to_bytes();
        let msg = settlement_message("ROOM1", 1500, 500, "u1alice", "u1bob", "deadbeef");
        let sig = hex::encode(sk.sign(msg.as_bytes()).to_bytes());

        // valid signature over the exact settlement verifies
        assert!(verify_settlement_sig(&pk, msg.as_bytes(), &sig));

        // any change to the outcome (stacks) invalidates the signature
        let tampered = settlement_message("ROOM1", 2000, 0, "u1alice", "u1bob", "deadbeef");
        assert!(!verify_settlement_sig(&pk, tampered.as_bytes(), &sig));

        // a different key does not verify
        let other: [u8; 32] = SigningKey::from_bytes(&[9u8; 32]).verifying_key().to_bytes();
        assert!(!verify_settlement_sig(&other, msg.as_bytes(), &sig));

        // garbage signature hex is rejected, not panicked
        assert!(!verify_settlement_sig(&pk, msg.as_bytes(), "not-hex"));
        assert!(!verify_settlement_sig(&pk, msg.as_bytes(), ""));
    }

    #[test]
    fn payout_memo_pins_identity_pubkey() {
        // the on-chain memo binds the seat to a settlement key the operator can't forge
        let pk = [0xabu8; 32];
        let memo = format!("zk.poker/v1/payout:u1exampleaddressxxxxxxxx;id:{}", hex::encode(pk));
        let mut buf = [0u8; 512];
        buf[..memo.len()].copy_from_slice(memo.as_bytes());
        let (addr, parsed_pk) = super::scanner::parse_payout_memo(&buf).expect("parses");
        assert_eq!(addr, "u1exampleaddressxxxxxxxx");
        assert_eq!(parsed_pk, Some(pk));
    }

    #[test]
    fn payout_token_empty_is_rejected() {
        let expected = new_payout_token();
        assert_eq!(check_payout_token(&expected, Some(""), None).unwrap_err().0, StatusCode::UNAUTHORIZED);
        assert_eq!(check_payout_token(&expected, None, Some("   ")).unwrap_err().0, StatusCode::UNAUTHORIZED);
    }

    #[test]
    fn payout_token_wrong_value_is_rejected() {
        let expected = new_payout_token();
        let mut wrong = expected;
        wrong[0] ^= 0xff; // flip a byte — different token
        let res = check_payout_token(&expected, Some(&hex::encode(wrong)), None);
        assert_eq!(res.unwrap_err().0, StatusCode::FORBIDDEN);
    }

    #[test]
    fn payout_token_malformed_hex_is_rejected() {
        let expected = new_payout_token();
        assert_eq!(check_payout_token(&expected, Some("not-hex-zz"), None).unwrap_err().0, StatusCode::UNAUTHORIZED);
        // right hex, wrong length
        assert_eq!(check_payout_token(&expected, Some("abcd"), None).unwrap_err().0, StatusCode::UNAUTHORIZED);
    }

    #[test]
    fn payout_token_correct_is_accepted() {
        let expected = new_payout_token();
        // via header
        assert!(check_payout_token(&expected, Some(&hex::encode(expected)), None).is_ok());
        // via body
        assert!(check_payout_token(&expected, None, Some(&hex::encode(expected))).is_ok());
        // header takes precedence over a bad body token
        assert!(check_payout_token(&expected, Some(&hex::encode(expected)), Some("garbage")).is_ok());
    }

    #[test]
    fn tokens_are_unique_per_room() {
        // two rooms must not share a token (CSPRNG); collision would let one room's creator
        // authorize another room's payout.
        let a = new_payout_token();
        let b = new_payout_token();
        assert_ne!(a, b);
    }

    // ── FIX 1/2: pending-settlement lifecycle ─────────────────────────────────
    //
    // Exercises `try_complete_pending_settlement` — the state machine that turns a co-signed
    // /settle recorded while a deposit was unconfirmed into an executed CONFIRMED payout plan:
    //   accepted-while-unconfirmed  → does NOT execute (no plan)
    //   both deposits confirm        → executes (builds payout_plan, clears settle_pending)
    //   deposit evicted instead      → does NOT execute (evicted_shortfall guard)
    //   idempotency                  → a second completion is a no-op

    /// Build a minimal DKG-complete-ish EscrowRoom with seat 0/1 identity keys pinned and a
    /// co-signed `settle_pending` queued. Uses a real osst keygen so the curve fields are valid.
    fn room_with_queued_settlement(a_conf: u64, b_conf: u64, required: u64) -> (EscrowRoom, String) {
        use ed25519_dalek::{SigningKey, Signer};
        let code = "QUEUE1".to_string();

        // real osst material (mirrors make_legacy_osst)
        let mut rng = rand::thread_rng();
        let (_a, _b, jury, group_pubkey) =
            osst::redpallas::zcash::setup_escrow(1, 1, &mut rng).expect("setup_escrow");
        let escrow_address = osst::redpallas::zcash::derive_address_bytes(&group_pubkey);
        let server_share = jury.node_shares.into_iter().next().unwrap();

        // two seat identity keys + a co-signed outcome over the exact settlement message
        let sk_a = SigningKey::from_bytes(&[3u8; 32]);
        let sk_b = SigningKey::from_bytes(&[4u8; 32]);
        let pk_a = sk_a.verifying_key().to_bytes();
        let pk_b = sk_b.verifying_key().to_bytes();
        let (a_addr, b_addr) = ("u1alicepayoutaddr", "u1bobpayoutaddr");
        let (a_stack, b_stack, log_hash) = (1500u64, 500u64, "deadbeefcafe");
        let msg = settlement_message(&code, a_stack, b_stack, a_addr, b_addr, log_hash);
        let a_sig = hex::encode(sk_a.sign(msg.as_bytes()).to_bytes());
        let b_sig = hex::encode(sk_b.sign(msg.as_bytes()).to_bytes());

        let room = EscrowRoom {
            code: code.clone(),
            escrow_ua: Some("u1escrow".to_string()),
            frost_relay_url: None,
            frost_room_code: None,
            dkg_key_package_hex: None,
            dkg_public_key_package_hex: None,
            dkg_orchard_fvk_hex: None,
            dkg_sk_hex: None,
            dkg_ephemeral_seed_hex: None,
            seat_addresses: vec![None, None],
            seat_addr_bytes: vec![None, None],
            seat_payout_address: vec![Some(a_addr.to_string()), Some(b_addr.to_string())],
            seat_identity_pubkey: vec![Some(pk_a), Some(pk_b)],
            notes: Vec::new(),
            last_scanned_height: 0,
            payout_status: PayoutStatus::None,
            escrow_address,
            group_pubkey,
            server_share,
            player_a_share_hex: String::new(),
            player_b_share_hex: String::new(),
            player_a_deposit: a_conf,
            player_b_deposit: b_conf,
            required_deposit: required,
            rake_bps: 0,
            rake_paid: false,
            game_active: true,
            player_a_address: None,
            player_b_address: None,
            final_stacks: None,
            payout_plan: None,
            pending_nonces: None,
            created_at: now_ms(),
            payout_token: new_payout_token(),
            counted_deposits: std::collections::HashSet::new(),
            player_a_deposit_pending: 0,
            player_b_deposit_pending: 0,
            pending_deposits: std::collections::HashMap::new(),
            settle_pending: Some(PendingSettlement {
                player_a_stack: a_stack,
                player_b_stack: b_stack,
                player_a_address: a_addr.to_string(),
                player_b_address: b_addr.to_string(),
                action_log_hash: log_hash.to_string(),
                player_a_sig: a_sig,
                player_b_sig: b_sig,
                queued_at: now_ms(),
            }),
            evicted_shortfall: false,
            dkg_failed: None,
        };
        (room, code)
    }

    #[test]
    fn pending_settlement_does_not_execute_while_unconfirmed() {
        let required = 100_000;
        // seat B never confirmed — money gate shut.
        let (mut room, code) = room_with_queued_settlement(required, 0, required);
        try_complete_pending_settlement(&mut room, "u1house", None, &code);
        assert!(room.payout_plan.is_none(), "must NOT build a plan while a deposit is unconfirmed");
        assert!(room.settle_pending.is_some(), "queued settlement must remain for later completion");
    }

    #[test]
    fn pending_settlement_executes_once_both_confirm() {
        let required = 100_000;
        // both confirmed now (a block landed for both seats).
        let (mut room, code) = room_with_queued_settlement(required, required, required);
        try_complete_pending_settlement(&mut room, "u1house", None, &code);
        assert!(room.payout_plan.is_some(), "must build the CONFIRMED payout plan once both confirm");
        assert!(room.settle_pending.is_none(), "queued settlement cleared after completion");
        // the plan spends exactly the confirmed pot (no rake here) minus the tx fee.
        let plan = room.payout_plan.clone().unwrap();
        let sum: u64 = plan.outputs.iter().map(|o| o.amount).sum();
        assert_eq!(sum + TX_PAYOUT_FEE_ZAT, 2 * required, "plan balances against confirmed pot");

        // IDEMPOTENCY: a second completion (e.g. scanner re-entry) must not rebuild/duplicate.
        let settled_at_before = room.payout_plan.as_ref().unwrap().settled_at;
        try_complete_pending_settlement(&mut room, "u1house", None, &code);
        assert_eq!(
            room.payout_plan.as_ref().unwrap().settled_at, settled_at_before,
            "second completion is a no-op (same plan, not rebuilt)"
        );
    }

    #[test]
    fn pending_settlement_does_not_execute_on_eviction() {
        let required = 100_000;
        // seat B's deposit was seen in the mempool (dealt the hand) but got EVICTED, never
        // confirmed. Confirmed b=0, and the eviction reconciliation marked the room.
        let (mut room, code) = room_with_queued_settlement(required, 0, required);
        room.evicted_shortfall = true; // set by run_mempool_watch eviction (FIX 2)
        try_complete_pending_settlement(&mut room, "u1house", None, &code);
        assert!(room.payout_plan.is_none(), "evicted_shortfall must block auto-completion");
        assert!(room.settle_pending.is_some(), "queued settlement left for refund/arbitrate recovery");

        // Even if — hypothetically — a later confirmation made both counters satisfy the gate,
        // the evicted_shortfall taint keeps the settled-plan auto-path shut (theft guard).
        room.player_b_deposit = required;
        try_complete_pending_settlement(&mut room, "u1house", None, &code);
        assert!(room.payout_plan.is_none(), "taint persists — settled auto-payout stays blocked");
    }

    #[test]
    fn evicted_shortfall_false_positive_clears_on_confirm() {
        // The eviction race: a deposit MINES (leaving the mempool) and the mempool watcher trips
        // evicted_shortfall in the ~20s before the confirmed scan credits it. This must NOT strand
        // a legitimately-funded game as a refund. run_deposit_poll clears the taint when the note
        // confirms and BOTH seats are satisfied; here we exercise that exact decision.
        let required = 100_000;

        // (a) genuine double-spend — seat B never confirms → clear-decision is FALSE, taint latches.
        assert!(!both_deposits_satisfied(required, 0, required),
            "one seat short → real shortfall, taint must NOT clear (theft-safe)");

        // (b) false positive — both mined/confirmed → clear-decision is TRUE.
        assert!(both_deposits_satisfied(required, required, required),
            "both confirmed → shortfall was a false positive, safe to clear");

        // end-to-end: with the taint cleared (as run_deposit_poll does on confirm), the queued
        // settlement completes to the WINNER instead of falling back to a refund-both wash.
        let (mut room, code) = room_with_queued_settlement(required, required, required);
        room.evicted_shortfall = true; // stale false-positive from the mine-vs-scan race
        if both_deposits_satisfied(room.player_a_deposit, room.player_b_deposit, room.required_deposit) {
            room.evicted_shortfall = false; // FIX #1: clear-on-confirm
        }
        try_complete_pending_settlement(&mut room, "u1house", None, &code);
        assert!(room.payout_plan.is_some(), "winner is paid once the false-positive taint clears");
        assert!(room.settle_pending.is_none(), "queued settlement completed, not left for refund");
    }

    #[test]
    fn finalize_settlement_is_idempotent() {
        let required = 100_000;
        let (mut room, code) = room_with_queued_settlement(required, required, required);
        let pending = room.settle_pending.clone().unwrap();
        let first = finalize_settlement(
            &mut room, "u1house", None, &code,
            pending.player_a_stack, pending.player_b_stack,
            &pending.player_a_address, &pending.player_b_address,
            &pending.action_log_hash, &pending.player_a_sig, &pending.player_b_sig,
        );
        // second call with the same inputs returns the SAME already-recorded plan (no rebuild)
        let second = finalize_settlement(
            &mut room, "u1house", None, &code,
            pending.player_a_stack, pending.player_b_stack,
            &pending.player_a_address, &pending.player_b_address,
            &pending.action_log_hash, &pending.player_a_sig, &pending.player_b_sig,
        );
        assert_eq!(first.settled_at, second.settled_at, "idempotent: same plan returned, not rebuilt");
        assert!(room.settle_pending.is_none());
    }

    // ── BUG 3: deposit dedup ──────────────────────────────────────────────

    /// Models the exact dedup keys the two credit paths use: the HTTP path keys on
    /// `http:<txid>:<seat>`, the scanner keys on `note:<nullifier hex>`. Crediting is gated on
    /// `HashSet::insert` returning true, so re-observation is a no-op.
    #[test]
    fn deposit_counted_exactly_once_across_paths() {
        let mut counted: HashSet<String> = HashSet::new();
        let mut balance: u64 = 0;

        // scanner observes note with nullifier N, value 1000
        let nf_key = format!("note:{}", hex::encode([7u8; 32]));
        if counted.insert(nf_key.clone()) { balance += 1000; }
        assert_eq!(balance, 1000);

        // same poll re-runs after a reorg and re-sees the same nullifier — must not double count
        if counted.insert(nf_key.clone()) { balance += 1000; }
        assert_eq!(balance, 1000, "re-observed nullifier must not credit twice");

        // HTTP self-report for a *different* txid credits once (demo mode)
        let http_key = "http:deadbeef:0".to_string();
        if counted.insert(http_key.clone()) { balance += 500; }
        assert_eq!(balance, 1500);

        // same HTTP report replayed — no double count
        if counted.insert(http_key.clone()) { balance += 500; }
        assert_eq!(balance, 1500, "replayed HTTP report must not credit twice");
    }

    // ── Settle-C1 / C2 / HIGH-1: refund split is the single unsigned money exit ────────────────

    #[test]
    fn refund_pays_each_seat_its_own_confirmed_deposit() {
        // two seats, unequal confirmed deposits. Each gets back its OWN confirmed money,
        // minus a proportional share of the tx fee. Conservation: sum + fee == total confirmed.
        let seats = vec![
            SeatRefund { address: "u1a".into(), confirmed_deposit: 120_000 },
            SeatRefund { address: "u1b".into(), confirmed_deposit: 80_000 },
        ];
        let total: u64 = seats.iter().map(|s| s.confirmed_deposit).sum();
        let (outs, amounts) = compute_refund_outputs("R", &seats);
        assert_eq!(outs.len(), 2);
        // conservation holds exactly
        assert_eq!(amounts.iter().sum::<u64>() + TX_PAYOUT_FEE_ZAT, total);
        assert_eq!(sum_outputs(&outs) + TX_PAYOUT_FEE_ZAT, total);
        // A (bigger deposit) gets back more than B, and neither exceeds its own deposit.
        assert!(amounts[0] > amounts[1]);
        assert!(amounts[0] <= 120_000 && amounts[1] <= 80_000);
        // no rake, no house output
        assert!(outs.iter().all(|o| o.memo.contains("refund")));
    }

    #[test]
    fn refund_is_safe_under_shortfall() {
        // 0-conf shortfall: seat B's buy-in was seen in mempool (dealt) but NEVER confirmed /
        // was evicted → B's CONFIRMED deposit is 0. The vault holds only A's real money. A refund
        // must return A its own deposit and pay B NOTHING — it can never pay B from A's money.
        let required = 100_000u64;
        let seats = vec![
            SeatRefund { address: "u1a".into(), confirmed_deposit: required },
            SeatRefund { address: "u1b".into(), confirmed_deposit: 0 },
        ];
        let (outs, amounts) = compute_refund_outputs("R", &seats);
        assert_eq!(outs.len(), 1, "only the funded seat gets an output");
        assert_eq!(amounts[1], 0, "unconfirmed/evicted seat is paid nothing");
        assert_eq!(amounts[0], required - TX_PAYOUT_FEE_ZAT, "A gets back only its own confirmed deposit");
        // conservation against the CONFIRMED pot (A's deposit), never the phantom 0-conf pot.
        assert_eq!(sum_outputs(&outs) + TX_PAYOUT_FEE_ZAT, required);
    }

    #[test]
    fn refund_generalises_to_n_seats() {
        // forward-looking: a future multiway table has N depositors. Conservation must still hold
        // and each seat gets back at most its own confirmed deposit.
        let seats = vec![
            SeatRefund { address: "u1a".into(), confirmed_deposit: 50_000 },
            SeatRefund { address: "u1b".into(), confirmed_deposit: 50_001 },
            SeatRefund { address: "u1c".into(), confirmed_deposit: 33_333 },
            SeatRefund { address: "u1d".into(), confirmed_deposit: 0 },
        ];
        let total: u64 = seats.iter().map(|s| s.confirmed_deposit).sum();
        let (outs, amounts) = compute_refund_outputs("R", &seats);
        assert_eq!(outs.len(), 3, "the zero-deposit seat emits no output");
        assert_eq!(amounts.iter().sum::<u64>() + TX_PAYOUT_FEE_ZAT, total, "conservation across N seats");
        for (i, s) in seats.iter().enumerate() {
            assert!(amounts[i] <= s.confirmed_deposit, "no seat is paid more than it confirmed");
        }
    }

    #[test]
    fn unsigned_non_refund_split_is_rejected() {
        // Settle-C1: the gate `initiate_payout` applies for the unsigned (no co-signed plan) exit.
        // The escrow computes its OWN refund split; a caller-chosen winner-take-all split must NOT
        // match it and must be rejected.
        let seats = vec![
            SeatRefund { address: "u1a".into(), confirmed_deposit: 100_000 },
            SeatRefund { address: "u1b".into(), confirmed_deposit: 100_000 },
        ];
        let (refund_outputs, _) = compute_refund_outputs("R", &seats);

        // (a) winner-take-all: A grabs the whole confirmed pot, B gets nothing → REJECTED.
        let winner_take_all = vec![("u1a".to_string(), 200_000 - TX_PAYOUT_FEE_ZAT)];
        assert!(!outputs_match_refund(&winner_take_all, &refund_outputs),
            "an unsigned winner-take-all split must not pass as a refund");

        // (b) a redirected refund (right amounts, attacker address) → REJECTED.
        let redirected = vec![
            ("u1attacker".to_string(), refund_outputs[0].amount),
            ("u1b".to_string(), refund_outputs[1].amount),
        ];
        assert!(!outputs_match_refund(&redirected, &refund_outputs),
            "a redirected destination must not pass as a refund");

        // (c) a shifted amount (steal 1 zat from B to A) → REJECTED.
        let shifted = vec![
            ("u1a".to_string(), refund_outputs[0].amount + 1),
            ("u1b".to_string(), refund_outputs[1].amount - 1),
        ];
        assert!(!outputs_match_refund(&shifted, &refund_outputs),
            "a shifted split must not pass as a refund");

        // (d) the EXACT refund split (any order) → accepted.
        let exact_reordered = vec![
            ("u1b".to_string(), refund_outputs[1].amount),
            ("u1a".to_string(), refund_outputs[0].amount),
        ];
        assert!(outputs_match_refund(&exact_reordered, &refund_outputs),
            "the escrow's own refund split must be accepted regardless of output order");
    }

    #[test]
    fn cancel_refund_plan_matches_confirmed_and_conserves() {
        // /cancel builds its refund plan from room_seat_refunds. Prove the plan the handler would
        // record refunds each seat its own confirmed deposit and conserves against the confirmed
        // pot — even under a shortfall (seat B unconfirmed).
        let required = 100_000;
        let (mut room, _code) = room_with_queued_settlement(required, 0, required); // B unconfirmed
        let seats = room_seat_refunds(&room);
        // both seats have a pinned payout address; only A has a confirmed deposit.
        assert_eq!(seats.len(), 2);
        assert_eq!(seats[0].confirmed_deposit, required);
        assert_eq!(seats[1].confirmed_deposit, 0);
        let (outs, amounts) = compute_refund_outputs("QUEUE1", &seats);
        assert_eq!(outs.len(), 1, "only the confirmed seat is refunded under shortfall");
        assert_eq!(amounts[0], required - TX_PAYOUT_FEE_ZAT);
        assert_eq!(amounts[1], 0, "unconfirmed seat refunded nothing — never from A's money");
        assert_eq!(sum_outputs(&outs) + TX_PAYOUT_FEE_ZAT, required, "conserves against confirmed pot");
        // sanity: the refund never exceeds what the vault actually holds (A's confirmed deposit).
        assert!(sum_outputs(&outs) <= room.player_a_deposit + room.player_b_deposit);
        let _ = &mut room;
    }
}
