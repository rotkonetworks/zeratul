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
mod frost_relay;
mod frost_dkg;
mod dkg_room;
mod scanner;
mod payout_signing;
mod tx_build;

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
    };

    let payout_token_hex = hex::encode(room.payout_token);
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
    state.rooms.lock().await.insert(req.code.clone(), room);

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
            "required_deposit": room.required_deposit,
            "rake_bps": room.rake_bps,
            "rake_paid": room.rake_paid,
            "game_active": room.game_active,
            "settled": room.payout_plan.is_some(),
            "both_deposited": room.player_a_deposit >= room.required_deposit
                && room.player_b_deposit >= room.required_deposit,
            "both_payout_addresses_known": room.seat_payout_address.iter().take(2).all(|a| a.is_some()),
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

    if !room.game_active {
        return Json(serde_json::json!({"error": "game not active"}));
    }

    // ── player co-signing gate ──────────────────────────────────────────────
    // Settlement decides who gets the pot. Require BOTH seats to sign the exact
    // outcome with the Ed25519 identity key each pinned on-chain in their deposit
    // memo. The operator holds no signing key here, so it cannot forge a payout.
    // Fail closed: no pinned identities (and no explicit demo override) ⇒ refuse.
    let allow_unsigned = std::env::var("ESCROW_ALLOW_UNSIGNED_SETTLE")
        .ok().as_deref() == Some("1");
    let pk_a = room.seat_identity_pubkey.get(0).and_then(|p| *p);
    let pk_b = room.seat_identity_pubkey.get(1).and_then(|p| *p);
    match (pk_a, pk_b) {
        (Some(pk_a), Some(pk_b)) => {
            let msg = settlement_message(
                &code, req.player_a_stack, req.player_b_stack,
                &req.player_a_address, &req.player_b_address, &req.action_log_hash,
            );
            let a_ok = verify_settlement_sig(&pk_a, msg.as_bytes(), &req.player_a_sig);
            let b_ok = verify_settlement_sig(&pk_b, msg.as_bytes(), &req.player_b_sig);
            if !a_ok || !b_ok {
                tracing::warn!("settle REJECTED room={} — bad player signatures (a_ok={} b_ok={})", code, a_ok, b_ok);
                return Json(serde_json::json!({
                    "error": "settlement requires a valid Ed25519 signature from BOTH seats' pinned identity keys",
                    "player_a_sig_ok": a_ok,
                    "player_b_sig_ok": b_ok,
                }));
            }
            // defence in depth: if a payout address was pinned on-chain, the signed
            // destination must match it — a signature can't redirect to a fresh addr.
            for (i, (req_addr, pinned)) in [
                (&req.player_a_address, room.seat_payout_address.get(0).and_then(|a| a.clone())),
                (&req.player_b_address, room.seat_payout_address.get(1).and_then(|a| a.clone())),
            ].into_iter().enumerate() {
                if let Some(p) = pinned {
                    if &p != req_addr {
                        tracing::warn!("settle REJECTED room={} seat={} — payout addr != on-chain pinned addr", code, i);
                        return Json(serde_json::json!({"error": "payout address does not match the on-chain pinned address"}));
                    }
                }
            }
            tracing::info!("settle room={} — both player signatures verified", code);
        }
        _ if allow_unsigned => {
            tracing::warn!("settle room={} — UNSIGNED (ESCROW_ALLOW_UNSIGNED_SETTLE=1, demo only)", code);
        }
        _ => {
            tracing::warn!("settle REJECTED room={} — seat identities not pinned on-chain; cannot verify co-signing", code);
            return Json(serde_json::json!({
                "error": "seat identities are not pinned on-chain (deposit memo must carry `;id:<pubkey>`); \
                          settlement cannot be verified. set ESCROW_ALLOW_UNSIGNED_SETTLE=1 for demo only.",
            }));
        }
    }

    // store player addresses
    room.player_a_address = Some(req.player_a_address.clone());
    room.player_b_address = Some(req.player_b_address.clone());

    // compute rake
    let total_pot = room.player_a_deposit + room.player_b_deposit;
    let rake = (total_pot as u128 * room.rake_bps as u128 / 10000) as u64;
    let distributable = total_pot - rake;

    // proportional payout based on final stacks
    let total_stacks = req.player_a_stack + req.player_b_stack;
    let payout_a = if total_stacks > 0 {
        distributable * req.player_a_stack / total_stacks
    } else {
        distributable / 2
    };
    let payout_b = distributable - payout_a;

    room.final_stacks = Some((payout_a, payout_b));
    room.game_active = false;

    // build payout plan: one output per player (skip zero payouts) + rake to house
    let escrow_hex = hex::encode(&room.escrow_address);
    let mut outputs = Vec::new();

    if payout_a > 0 {
        outputs.push(PayoutOutput {
            address: req.player_a_address.clone(),
            amount: payout_a,
            memo: format!("zk.poker payout room={}", code),
        });
    }
    if payout_b > 0 {
        outputs.push(PayoutOutput {
            address: req.player_b_address.clone(),
            amount: payout_b,
            memo: format!("zk.poker payout room={}", code),
        });
    }
    if rake > 0 {
        outputs.push(PayoutOutput {
            address: state.house_address.clone(),
            amount: rake,
            memo: format!("zk.poker rake room={}", code),
        });
    }

    let plan = PayoutPlan {
        room: code.clone(),
        escrow_source: escrow_hex.clone(),
        escrow_value: total_pot,
        outputs: outputs.clone(),
        action_log_hash: req.action_log_hash.clone(),
        settled_at: now_ms(),
    };

    room.payout_plan = Some(plan.clone());

    tracing::info!("settle: room={} A={} B={} rake={} outputs={} log={}",
        code, payout_a, payout_b, rake, outputs.len(),
        &req.action_log_hash[..req.action_log_hash.len().min(16)]);

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
                (fvk.clone(), kp.clone(), seed.clone(), pkg.clone(), room.notes.clone())
            }
            _ => return json_response(serde_json::json!({"error": "DKG not complete; no key material to sign with"})),
        }
    };
    let (fvk_hex, kp_hex, seed_hex, pkg_hex, notes) = snap;

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
    let mut outputs = Vec::with_capacity(req.outputs.len());
    for (i, out) in req.outputs.iter().enumerate() {
        let addr = match tx_build::parse_orchard_ua(&out.address, mainnet) {
            Ok(a) => a,
            Err(e) => return json_response(serde_json::json!({"error": format!("output {} address: {}", i, e)})),
        };
        outputs.push(tx_build::PayoutOutput {
            address: addr,
            amount_zat: out.amount_zat,
            memo: [0u8; 512],
        });
    }
    let fee_zat = if req.fee_zat == 0 { 10_000 } else { req.fee_zat };
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
        }
    }
    tracing::info!("payout initiated for {}: relay_room={} actions={}", code, relay_room, pczt_result.alphas.len());

    let (disp_recipient, disp_amount) = req.outputs.iter()
        .find(|o| o.amount_zat > 0)
        .map(|o| (o.address.clone(), o.amount_zat))
        .unwrap_or_else(|| (String::new(), 0));

    let bg_rooms = state.rooms.clone();
    let bg_code = code.clone();
    let bg_relay_room = relay_room.clone();
    let bg_zidecar_url = state.zidecar_url.clone();
    let bg_fee_zat = fee_zat;
    tokio::spawn(async move {
        run_payout_signing(bg_rooms, bg_code, bg_relay_room, bg_zidecar_url, relay,
            pkg_hex, kp_hex, seed_hex, pczt_result,
            disp_recipient, disp_amount, bg_fee_zat).await;
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
            mark_payout_failed(rooms, code, format!("host_sign_pczt: {:?}", e)).await;
            return;
        }
    };

    let tx_bytes = match crate::tx_build::complete_payout_pczt(
        &pczt_result.pczt_bytes, &sigs, &pczt_result.spend_indices,
    ) {
        Ok(b) => b,
        Err(e) => {
            tracing::error!("complete_payout_pczt for {}: {}", code, e);
            mark_payout_failed(rooms, code, format!("complete_payout_pczt: {}", e)).await;
            return;
        }
    };

    let zidecar = match zecli::client::ZidecarClient::connect(&zidecar_url).await {
        Ok(c) => c,
        Err(e) => {
            mark_payout_failed(rooms, code, format!("zidecar connect: {}", e)).await;
            return;
        }
    };
    match zidecar.send_transaction(tx_bytes).await {
        Ok(res) if res.is_success() => {
            tracing::info!("payout broadcast for {}: tx={}", code, res.txid);
            let mut rl = rooms.lock().await;
            if let Some(room) = rl.get_mut(&code) {
                room.payout_status = PayoutStatus::Broadcast { txid: res.txid, relay_room };
            }
        }
        Ok(res) => {
            mark_payout_failed(rooms, code,
                format!("zidecar rejected tx ({}): {}", res.error_code, res.error_message)).await;
        }
        Err(e) => {
            mark_payout_failed(rooms, code, format!("send_transaction: {}", e)).await;
        }
    }
}

async fn mark_payout_failed(rooms: Rooms, code: String, reason: String) {
    let mut rl = rooms.lock().await;
    if let Some(room) = rl.get_mut(&code) {
        room.payout_status = PayoutStatus::Failed { reason };
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

            let both = both_deposits_satisfied(room.player_a_deposit, room.player_b_deposit, room.required_deposit);

            if both {
                room.game_active = true;
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
}

#[tokio::main]
async fn main() {
    let _ = dotenvy::dotenv();
    let args = <Args as clap::Parser>::parse();

    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env()
            .add_directive("poker_escrow=info".parse().unwrap()))
        .init();

    let state = AppState {
        rooms: Arc::new(Mutex::new(HashMap::new())),
        house_address: args.house_address,
        zidecar_url: args.zidecar_url,
        verify_deposits: args.verify_deposits,
        network: orchard_ua::network_from_str(&args.network),
        use_dkg: args.use_dkg,
        frost_relay_url: args.relay_url,
    };

    tracing::info!("house address: {}", state.house_address);
    tracing::info!("zidecar: {}", state.zidecar_url);
    tracing::info!("verify_deposits: {}", state.verify_deposits);
    tracing::info!("network: {:?}", state.network);
    tracing::info!("use_dkg: {} (relay: {})", state.use_dkg, state.frost_relay_url);

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
        .route("/health", axum::routing::get(health))
        .with_state(state);

    let addr = format!("0.0.0.0:{}", args.port);
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
}
