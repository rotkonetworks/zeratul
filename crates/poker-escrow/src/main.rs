//! poker-escrow: FROST 2-of-3 escrow service for poker games.
//!
//! Manages:
//!   - Key generation (trusted dealer for demo, DKG for production)
//!   - Escrow address derivation
//!   - Deposit tracking (watches zidecar for incoming ZEC)
//!   - Rake fee transaction (from escrow → house wallet)
//!   - Payout transaction (from escrow → winners)
//!
//! Separate from the relay — holds key material, the relay doesn't.
//!
//! API:
//!   POST /room                   — create escrow for a new room
//!   GET  /room/{code}            — escrow address + deposit status
//!   POST /room/{code}/rake       — build + sign rake tx (needs 1 player sig)
//!   POST /room/{code}/payout     — build + sign payout tx
//!   POST /room/{code}/cancel     — cancel game + refund depositors
//!   GET  /health                 — service health

use axum::{Router, Json, extract::{Path, State}, response::IntoResponse};
use serde::{Deserialize, Serialize};
use sha2::{Sha256, Digest};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::Mutex;

use osst::redpallas::zcash as frost;
use pasta_curves::pallas::Scalar as PallasScalar;

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

struct EscrowRoom {
    code: String,
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
    cancelled: bool,
    // player addresses for refunds
    player_a_address: Option<String>,
    player_b_address: Option<String>,
    // payout
    final_stacks: Option<(u64, u64)>,
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

async fn create_room(
    State(state): State<AppState>,
    Json(req): Json<CreateRoomReq>,
) -> impl IntoResponse {
    // do all crypto BEFORE any await (rng is not Send)
    let (room, a_share_hex, b_share_hex, pubkey_hex, escrow_hex) = {
        let mut rng = rand::thread_rng();
        let result = frost::setup_escrow(1, 1, &mut rng);
        let (player_a_share, player_b_share, jury_network, group_pubkey) = match result {
            Ok(r) => r,
            Err(e) => return Json(serde_json::json!({"error": format!("{:?}", e)})),
        };

        let escrow_address = frost::derive_address_bytes(&group_pubkey);
        let escrow_hex = hex::encode(&escrow_address);
        let a_share_hex = hex::encode(&share_to_bytes(&player_a_share));
        let b_share_hex = hex::encode(&share_to_bytes(&player_b_share));
        let pubkey_hex = hex::encode(&point_to_bytes(&group_pubkey));

        let server_share = jury_network.node_shares.into_iter().next()
            .expect("jury_n=1 should produce exactly 1 share");

        let room = EscrowRoom {
            code: req.code.clone(),
            escrow_address,
            group_pubkey,
            server_share,
            player_a_share_hex: a_share_hex.clone(),
            player_b_share_hex: b_share_hex.clone(),
            player_a_deposit: 0,
            player_b_deposit: 0,
            required_deposit: req.required_deposit,
            rake_bps: req.rake_bps,
            rake_paid: false,
            game_active: false,
            cancelled: false,
            player_a_address: None,
            player_b_address: None,
            final_stacks: None,
            pending_nonces: None,
            created_at: now_ms(),
        };
        (room, a_share_hex, b_share_hex, pubkey_hex, escrow_hex)
    };

    state.rooms.lock().await.insert(req.code.clone(), room);

    tracing::info!("escrow created: {} → {}", req.code, &escrow_hex[..16]);

    Json(serde_json::json!({
        "escrow_address": escrow_hex,
        "player_a_share": a_share_hex,
        "player_b_share": b_share_hex,
        "public_key_package": pubkey_hex,
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
            "escrow_address": hex::encode(&room.escrow_address),
            "player_a_deposit": room.player_a_deposit,
            "player_b_deposit": room.player_b_deposit,
            "required_deposit": room.required_deposit,
            "rake_bps": room.rake_bps,
            "rake_paid": room.rake_paid,
            "game_active": room.game_active,
            "cancelled": room.cancelled,
            "both_deposited": room.player_a_deposit >= room.required_deposit
                && room.player_b_deposit >= room.required_deposit,
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
    /// player's return address for refunds
    #[serde(default)]
    return_address: Option<String>,
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

    if room.cancelled {
        return Json(serde_json::json!({"error": "room cancelled"}));
    }

    // store player return address if provided
    if let Some(addr) = &req.return_address {
        match req.seat {
            0 => room.player_a_address = Some(addr.clone()),
            1 => room.player_b_address = Some(addr.clone()),
            _ => {}
        }
    }

    // when verify_deposits is true, we would check the txid against zidecar here.
    // for now (demo mode), trust the client report.
    if state.verify_deposits {
        tracing::warn!("ESCROW_VERIFY_DEPOSITS=true but chain verification not yet implemented — accepting report on trust");
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

/// game ended — record final stacks for payout
#[derive(Deserialize)]
struct SettleReq {
    player_a_stack: u64,
    player_b_stack: u64,
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

    if room.cancelled {
        return Json(serde_json::json!({"error": "room cancelled"}));
    }

    if !room.game_active {
        return Json(serde_json::json!({"error": "game not active"}));
    }

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

    tracing::info!("settle: room={} A={} B={} rake={} log={}",
        code, payout_a, payout_b, rake, &req.action_log_hash[..req.action_log_hash.len().min(16)]);

    Json(serde_json::json!({
        "payout_a": payout_a,
        "payout_b": payout_b,
        "rake": rake,
        "action_log_hash": req.action_log_hash,
        // TODO: return unsigned payout tx for players to co-sign
    }))
}

/// FROST signing — escrow provides its signature share for a transaction.
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

    if room.cancelled && req.tx_type != "refund" {
        return Json(serde_json::json!({"error": "room cancelled — only refund signing allowed"}));
    }

    // validate: only sign rake if deposits confirmed, only sign payout if game ended
    match req.tx_type.as_str() {
        "rake" => {
            if room.player_a_deposit < room.required_deposit || room.player_b_deposit < room.required_deposit {
                return Json(serde_json::json!({"error": "deposits not confirmed"}));
            }
        }
        "payout" => {
            if room.final_stacks.is_none() {
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

/// FROST round 2 — server produces its signature share.
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
        None => return Json(serde_json::json!({"error": "no pending nonces — call /sign first"})),
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

/// Cancel a room and compute refund plan.
/// Either player can cancel before the game starts.
/// If game is active, reject — must use /settle or dispute.
#[derive(Deserialize)]
struct CancelReq {
    /// seat of the player requesting cancel (0 or 1)
    seat: u8,
    /// reason for cancellation (logged, not enforced)
    #[serde(default)]
    reason: Option<String>,
}

async fn cancel_room(
    State(state): State<AppState>,
    Path(code): Path<String>,
    Json(req): Json<CancelReq>,
) -> impl IntoResponse {
    let mut rooms = state.rooms.lock().await;
    let room = match rooms.get_mut(&code) {
        Some(r) => r,
        None => return Json(serde_json::json!({"error": "room not found"})),
    };

    if room.cancelled {
        return Json(serde_json::json!({"error": "room already cancelled"}));
    }

    if room.game_active {
        return Json(serde_json::json!({
            "error": "game is active — use /settle or dispute instead"
        }));
    }

    // mark cancelled
    room.cancelled = true;
    room.game_active = false;

    let reason = req.reason.as_deref().unwrap_or("player request");
    tracing::info!("cancel: room={} seat={} reason={}", code, req.seat, reason);

    // build refund plan — same format as payout outputs
    // no rake on cancellation (game never started)
    let mut refunds = Vec::new();

    if room.player_a_deposit > 0 {
        refunds.push(serde_json::json!({
            "seat": 0,
            "address": room.player_a_address.as_deref().unwrap_or("unknown"),
            "amount": room.player_a_deposit,
        }));
    }

    if room.player_b_deposit > 0 {
        refunds.push(serde_json::json!({
            "seat": 1,
            "address": room.player_b_address.as_deref().unwrap_or("unknown"),
            "amount": room.player_b_deposit,
        }));
    }

    let total_refund: u64 = room.player_a_deposit + room.player_b_deposit;

    Json(serde_json::json!({
        "cancelled": true,
        "reason": reason,
        "cancelled_by": req.seat,
        "escrow_address": hex::encode(&room.escrow_address),
        "total_refund": total_refund,
        "refunds": refunds,
        "tx_type": "refund",
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
            if room.game_active || room.cancelled {
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
            tracing::info!("monitor: room={} both deposits confirmed — game_active=true", code);
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

    let state = AppState {
        rooms: Arc::new(Mutex::new(HashMap::new())),
        house_address,
        zidecar_url,
        verify_deposits,
    };

    tracing::info!("house address: {}", state.house_address);
    tracing::info!("zidecar: {}", state.zidecar_url);
    tracing::info!("verify_deposits: {}", state.verify_deposits);

    // spawn background deposit monitor (checks every 5s)
    tokio::spawn(deposit_monitor(state.clone()));

    let app = Router::new()
        .route("/room", axum::routing::post(create_room))
        .route("/room/{code}", axum::routing::get(get_room))
        .route("/room/{code}/deposit", axum::routing::post(report_deposit))
        .route("/room/{code}/settle", axum::routing::post(settle))
        .route("/room/{code}/cancel", axum::routing::post(cancel_room))
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
