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
    /// per-seat deposit UAs (`u1…`) derived at diversifier_index 1, 2
    seat_addresses: Vec<Option<String>>,
    /// per-seat 43-byte raw addresses for matching decrypted notes back to a seat
    seat_addr_bytes: Vec<Option<[u8; 43]>>,
    /// per-seat refund/payout destination — recovered from the `zk.poker/v1/payout:` memo
    /// on the first valid deposit. `None` means the depositor forgot the memo; deposits still
    /// accrue but the game won't start until we know where to refund/pay.
    seat_payout_address: Vec<Option<String>>,
    /// every scanned incoming note kept around so the payout tx builder can spend it later.
    /// Position (orchard merkle tree leaf index) lands here when the tx builder fetches a
    /// witness from zidecar via `GetCommitmentProofs` keyed on `cmx`.
    notes: Vec<scanner::DepositNote>,
    /// resume point for the deposit scanner — last block whose actions we've trial-decrypted
    last_scanned_height: u32,
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
}

type Rooms = Arc<Mutex<HashMap<String, EscrowRoom>>>;

#[derive(Clone)]
pub struct AppState {
    rooms: Rooms,
    house_address: String,
    zidecar_url: String,
    verify_deposits: bool,
    network: zcash_address::Network,
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
fn derive_escrow_ua(network: zcash_address::Network) -> Result<String, String> {
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
        seat_addresses: vec![None, None],
        seat_addr_bytes: vec![None, None],
        seat_payout_address: vec![None, None],
        notes: Vec::new(),
        last_scanned_height: 0,
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
    };

    state.rooms.lock().await.insert(req.code.clone(), room);
    tracing::info!("escrow created (trusted-dealer): {} -> {}", req.code, &escrow_ua);

    Json(serde_json::json!({
        "escrow_address": escrow_ua,
        "player_a_share": a_share_hex,
        "player_b_share": b_share_hex,
        "public_key_package": pubkey_hex,
        "dkg_mode": false,
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
    );
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
    let mut rooms = state.rooms.lock().await;
    let room = match rooms.get_mut(&code) {
        Some(r) => r,
        None => return Json(serde_json::json!({"error": "room not found"})),
    };

    // when verify_deposits is true, we would check the txid against zidecar here.
    // for now (demo mode), trust the client report.
    if state.verify_deposits {
        tracing::warn!("ESCROW_VERIFY_DEPOSITS=true but chain verification not yet implemented -- accepting report on trust");
        // TODO: call zidecar to confirm txid pays to escrow_address
        // let confirmed = verify_deposit_on_chain(&state.zidecar_url, &room.escrow_address, &req.txid).await;
    }

    match req.seat {
        0 => room.player_a_deposit += req.amount,
        1 => room.player_b_deposit += req.amount,
        _ => return Json(serde_json::json!({"error": "invalid seat"})),
    }

    let both = room.player_a_deposit >= room.required_deposit
        && room.player_b_deposit >= room.required_deposit;

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

            let both = room.player_a_deposit >= room.required_deposit
                && room.player_b_deposit >= room.required_deposit;

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

// ---------------------------------------------------------------------------
// Main
// ---------------------------------------------------------------------------

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env()
            .add_directive("poker_escrow=info".parse().unwrap()))
        .init();

    let house_address = std::env::var("HOUSE_ADDRESS")
        .unwrap_or_else(|_| "ztestsapling1...".to_string());
    let zidecar_url = std::env::var("ZIDECAR_URL")
        .unwrap_or_else(|_| "https://zcash.rotko.net".to_string());
    let verify_deposits = std::env::var("ESCROW_VERIFY_DEPOSITS")
        .map(|v| v == "true" || v == "1")
        .unwrap_or(false);
    let network = orchard_ua::network_from_str(
        &std::env::var("ESCROW_NETWORK").unwrap_or_else(|_| "test".to_string())
    );
    let use_dkg = std::env::var("ESCROW_USE_DKG")
        .map(|v| v == "true" || v == "1")
        .unwrap_or(false);
    let frost_relay_url = std::env::var("ESCROW_RELAY_URL")
        .unwrap_or_else(|_| "ws://127.0.0.1:50053/ws".to_string());

    let state = AppState {
        rooms: Arc::new(Mutex::new(HashMap::new())),
        house_address,
        zidecar_url,
        verify_deposits,
        network,
        use_dkg,
        frost_relay_url,
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
        .route("/room/{code}/sign", axum::routing::post(frost_sign))
        .route("/room/{code}/sign-round2", axum::routing::post(frost_sign_round2))
        .route("/health", axum::routing::get(health))
        .with_state(state);

    let port = std::env::var("ESCROW_PORT").unwrap_or_else(|_| "3034".to_string());
    let addr = format!("0.0.0.0:{}", port);
    tracing::info!("poker-escrow listening on http://{}", addr);

    let listener = tokio::net::TcpListener::bind(addr).await.unwrap();
    axum::serve(listener, app).await.unwrap();
}
