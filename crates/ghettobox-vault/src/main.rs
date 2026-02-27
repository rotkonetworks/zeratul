//! ghettobox-vault - guard your users' key shares
//!
//! run this on your validator to provide key recovery services.
//!
//! usage:
//!   ghettobox-vault --port 4200 --software    # dev/testing
//!   ghettobox-vault --port 4200 --tpm         # hardware tpm
//!   ghettobox-vault --port 4200 --hsm         # future: hsm support
//!
//! data stored in ~/.ghettobox-vault/

use axum::{
    extract::{Path, State},
    http::StatusCode,
    routing::{get, post},
    Json, Router,
};
use clap::{Parser, ValueEnum};
use curve25519_dalek::{
    constants::RISTRETTO_BASEPOINT_POINT,
    ristretto::{CompressedRistretto, RistrettoPoint},
    scalar::Scalar,
};
use ed25519_dalek::{SigningKey, Signer, Signature};
use ghettobox::{Realm, Error as GhettoError, VerifiedOprfResponse, ServerPublicKey};
use ghettobox::oprf::DleqProof;
use metrics::{counter, gauge, histogram};
use metrics_exporter_prometheus::PrometheusBuilder;
use serde::{Deserialize, Serialize};
use sha2::{Sha512, Digest};
use std::sync::Arc;
use std::time::Instant;
use tokio::sync::RwLock;
use tower_http::cors::CorsLayer;
use tracing::{info, warn, error};

/// realm mode selection
#[derive(Debug, Clone, Copy, ValueEnum, PartialEq, Eq)]
pub enum RealmMode {
    /// software-only encryption (dev/testing only, no hardware security)
    Software,
    /// tpm 2.0 hardware sealing (requires /dev/tpm0 or /dev/tpmrm0)
    Tpm,
    /// hsm integration (future)
    Hsm,
}

impl std::fmt::Display for RealmMode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            RealmMode::Software => write!(f, "software"),
            RealmMode::Tpm => write!(f, "tpm"),
            RealmMode::Hsm => write!(f, "hsm"),
        }
    }
}

/// ghettobox-vault - guard your users' key shares
#[derive(Parser)]
#[command(name = "ghettobox-vault")]
#[command(about = "ghettobox vault - guard your users' key shares")]
#[command(version)]
struct Args {
    /// port to listen on
    #[arg(short, long, default_value = "4200")]
    port: u16,

    /// data directory (default: ~/.ghettobox-vault)
    #[arg(short, long)]
    data_dir: Option<String>,

    /// vault index (1-3, for display only)
    #[arg(short, long, default_value = "1")]
    index: u8,

    /// realm mode for sealing shares
    #[arg(short, long, value_enum, default_value = "software")]
    mode: RealmMode,

    /// bind address (default: 0.0.0.0)
    #[arg(short, long, default_value = "0.0.0.0")]
    bind: String,

    /// metrics port (prometheus endpoint, default: api_port + 1000)
    #[arg(long)]
    metrics_port: Option<u16>,
}

/// legacy registration stored in db (unlock_tag based)
#[derive(Clone, Serialize, Deserialize)]
struct Registration {
    /// sealed share data
    sealed_share: Vec<u8>,
    /// unlock key tag for verification
    unlock_tag: [u8; 16],
    /// max allowed guesses
    allowed_guesses: u32,
    /// failed attempts so far
    attempted_guesses: u32,
    /// registration timestamp
    created_at: u64,
    /// realm mode used for sealing
    realm_mode: String,
}

/// OPRF registration stored in db (verified OPRF based)
#[derive(Clone, Serialize, Deserialize)]
struct OprfRegistration {
    /// encrypted seed (client encrypts with OPRF output)
    encrypted_seed: Vec<u8>,
    /// max allowed guesses
    allowed_guesses: u32,
    /// failed attempts so far
    attempted_guesses: u32,
    /// registration timestamp
    created_at: u64,
}

/// app state shared across handlers
struct AppState {
    /// embedded database
    db: sled::Db,
    /// OPRF registrations (separate tree)
    oprf_db: sled::Tree,
    /// realm for sealing (software, tpm, or hsm)
    realm: Box<dyn Realm>,
    /// node signing key (ed25519)
    signing_key: SigningKey,
    /// OPRF share (ristretto255 scalar)
    oprf_share: Scalar,
    /// OPRF public key (G * share)
    oprf_pubkey: RistrettoPoint,
    /// node index (0-based for OPRF)
    index: u8,
    /// realm mode
    mode: RealmMode,
    /// tpm info (only set in tpm mode)
    tpm_info: Option<TpmInfoResponse>,
}

// === request/response types ===

#[derive(Deserialize)]
struct RegisterRequest {
    user_id: [u8; 32],
    unlock_tag: [u8; 16],
    encrypted_share: Vec<u8>,
    allowed_guesses: u32,
}

#[derive(Serialize)]
struct RegisterResponse {
    ok: bool,
    node_index: u8,
    signature: String,
}

#[derive(Deserialize)]
struct RecoverRequest {
    user_id: [u8; 32],
    unlock_tag: [u8; 16],
}

#[derive(Serialize)]
struct RecoverResponse {
    ok: bool,
    share: Option<ShareData>,
    guesses_remaining: u32,
    error: Option<String>,
}

#[derive(Serialize)]
struct ShareData {
    index: u8,
    data: String,
}

#[derive(Serialize)]
struct StatusResponse {
    registered: bool,
    guesses_remaining: u32,
    locked: bool,
}

#[derive(Serialize)]
struct NodeInfoResponse {
    version: String,
    index: u8,
    pubkey: String,
    registrations: u64,
    mode: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    tpm_info: Option<TpmInfoResponse>,
}

#[derive(Clone, Serialize)]
struct TpmInfoResponse {
    manufacturer: String,
    firmware_version: String,
    tpm_type: String,
    is_virtual: bool,
}

// === OPRF request/response types ===

#[derive(Deserialize)]
struct OprfRegisterRequest {
    /// user identifier (hex encoded)
    user_id: String,
    /// blinded element (hex encoded compressed point)
    blinded: String,
    /// encrypted seed (hex encoded)
    encrypted_seed: String,
    /// allowed guesses before lockout
    allowed_guesses: u32,
}

#[derive(Serialize)]
struct OprfRegisterResponse {
    ok: bool,
    response: Option<VerifiedOprfResponse>,
    error: Option<String>,
}

#[derive(Deserialize)]
struct OprfRecoverRequest {
    /// user identifier (hex encoded)
    user_id: String,
    /// blinded element (hex encoded compressed point)
    blinded: String,
}

#[derive(Serialize)]
struct OprfRecoverResponse {
    ok: bool,
    response: Option<VerifiedOprfResponse>,
    encrypted_seed: Option<String>,
    guesses_remaining: u32,
    error: Option<String>,
}

#[derive(Serialize)]
struct OprfHealthResponse {
    ok: bool,
    index: u8,
    version: String,
    oprf_pubkey: String,
}

// === handlers ===

async fn register(
    State(state): State<Arc<RwLock<AppState>>>,
    Json(req): Json<RegisterRequest>,
) -> Result<Json<RegisterResponse>, (StatusCode, String)> {
    let start = Instant::now();
    counter!("vault_requests_total", "endpoint" => "register").increment(1);

    let state = state.write().await;
    let user_key = hex::encode(req.user_id);

    if state.db.contains_key(&user_key).unwrap_or(false) {
        counter!("vault_errors_total", "endpoint" => "register", "error" => "conflict").increment(1);
        return Err((StatusCode::CONFLICT, "already registered".into()));
    }

    let sealed = state.realm.seal(&req.encrypted_share)
        .map_err(|e: GhettoError| {
            counter!("vault_errors_total", "endpoint" => "register", "error" => "seal_failed").increment(1);
            (StatusCode::INTERNAL_SERVER_ERROR, e.to_string())
        })?;

    let reg = Registration {
        sealed_share: sealed,
        unlock_tag: req.unlock_tag,
        allowed_guesses: req.allowed_guesses.min(10),
        attempted_guesses: 0,
        created_at: std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs(),
        realm_mode: state.mode.to_string(),
    };

    let reg_bytes = serde_json::to_vec(&reg)
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    state.db.insert(&user_key, reg_bytes)
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    let sig_data = [req.user_id.as_slice(), &req.unlock_tag].concat();
    let signature: Signature = state.signing_key.sign(&sig_data);

    // update metrics
    counter!("vault_registrations_total").increment(1);
    gauge!("vault_registrations_current").set(state.db.len() as f64);
    histogram!("vault_request_duration_seconds", "endpoint" => "register").record(start.elapsed().as_secs_f64());

    info!("registered user {}", &user_key[..16]);

    Ok(Json(RegisterResponse {
        ok: true,
        node_index: state.index,
        signature: hex::encode(signature.to_bytes()),
    }))
}

async fn recover(
    State(state): State<Arc<RwLock<AppState>>>,
    Json(req): Json<RecoverRequest>,
) -> Result<Json<RecoverResponse>, (StatusCode, String)> {
    let start = Instant::now();
    counter!("vault_requests_total", "endpoint" => "recover").increment(1);

    let state = state.write().await;
    let user_key = hex::encode(req.user_id);

    let reg_bytes = state.db.get(&user_key)
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
        .ok_or_else(|| {
            counter!("vault_errors_total", "endpoint" => "recover", "error" => "not_found").increment(1);
            (StatusCode::NOT_FOUND, "not registered".into())
        })?;

    let mut reg: Registration = serde_json::from_slice(&reg_bytes)
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    if reg.attempted_guesses >= reg.allowed_guesses {
        state.db.remove(&user_key).ok();
        counter!("vault_lockouts_total").increment(1);
        gauge!("vault_registrations_current").set(state.db.len() as f64);
        warn!("user {} locked out, deleting", &user_key[..16]);
        return Ok(Json(RecoverResponse {
            ok: false,
            share: None,
            guesses_remaining: 0,
            error: Some("no guesses remaining, registration deleted".into()),
        }));
    }

    if req.unlock_tag != reg.unlock_tag {
        reg.attempted_guesses += 1;
        let remaining = reg.allowed_guesses.saturating_sub(reg.attempted_guesses);

        let reg_bytes = serde_json::to_vec(&reg).unwrap();
        state.db.insert(&user_key, reg_bytes).ok();

        counter!("vault_failed_attempts_total").increment(1);
        warn!("user {} wrong pin, {} remaining", &user_key[..16], remaining);

        return Ok(Json(RecoverResponse {
            ok: false,
            share: None,
            guesses_remaining: remaining,
            error: Some(format!("invalid pin, {} guesses remaining", remaining)),
        }));
    }

    let share_data = state.realm.unseal(&reg.sealed_share)
        .map_err(|e: GhettoError| {
            counter!("vault_errors_total", "endpoint" => "recover", "error" => "unseal_failed").increment(1);
            (StatusCode::INTERNAL_SERVER_ERROR, e.to_string())
        })?;

    counter!("vault_recoveries_total").increment(1);
    histogram!("vault_request_duration_seconds", "endpoint" => "recover").record(start.elapsed().as_secs_f64());

    info!("user {} recovered successfully", &user_key[..16]);

    Ok(Json(RecoverResponse {
        ok: true,
        share: Some(ShareData {
            index: state.index,
            data: hex::encode(&share_data),
        }),
        guesses_remaining: reg.allowed_guesses - reg.attempted_guesses,
        error: None,
    }))
}

async fn status(
    State(state): State<Arc<RwLock<AppState>>>,
    Path(user_id): Path<String>,
) -> Result<Json<StatusResponse>, (StatusCode, String)> {
    let state = state.read().await;
    let user_key = user_id;

    let reg_bytes = state.db.get(&user_key)
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    match reg_bytes {
        None => Ok(Json(StatusResponse {
            registered: false,
            guesses_remaining: 0,
            locked: false,
        })),
        Some(bytes) => {
            let reg: Registration = serde_json::from_slice(&bytes)
                .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

            Ok(Json(StatusResponse {
                registered: true,
                guesses_remaining: reg.allowed_guesses.saturating_sub(reg.attempted_guesses),
                locked: reg.attempted_guesses >= reg.allowed_guesses,
            }))
        }
    }
}

async fn node_info(
    State(state): State<Arc<RwLock<AppState>>>,
) -> Json<NodeInfoResponse> {
    let state = state.read().await;
    let count = state.db.len() as u64;

    Json(NodeInfoResponse {
        version: env!("CARGO_PKG_VERSION").into(),
        index: state.index,
        pubkey: hex::encode(state.signing_key.verifying_key().to_bytes()),
        registrations: count,
        mode: state.mode.to_string(),
        tpm_info: state.tpm_info.clone(),
    })
}

async fn health() -> &'static str {
    "ok"
}

// === OPRF handlers ===

async fn oprf_health(
    State(state): State<Arc<RwLock<AppState>>>,
) -> Json<OprfHealthResponse> {
    let state = state.read().await;
    Json(OprfHealthResponse {
        ok: true,
        index: state.index,
        version: env!("CARGO_PKG_VERSION").into(),
        oprf_pubkey: hex::encode(state.oprf_pubkey.compress().as_bytes()),
    })
}

async fn oprf_register(
    State(state): State<Arc<RwLock<AppState>>>,
    Json(req): Json<OprfRegisterRequest>,
) -> Result<Json<OprfRegisterResponse>, (StatusCode, String)> {
    let start = Instant::now();
    counter!("vault_requests_total", "endpoint" => "oprf_register").increment(1);

    let state = state.write().await;

    // check if already registered
    if state.oprf_db.contains_key(&req.user_id).unwrap_or(false) {
        counter!("vault_errors_total", "endpoint" => "oprf_register", "error" => "conflict").increment(1);
        return Ok(Json(OprfRegisterResponse {
            ok: false,
            response: None,
            error: Some("already registered".into()),
        }));
    }

    // decode blinded point
    let blinded_bytes = hex::decode(&req.blinded)
        .map_err(|e| (StatusCode::BAD_REQUEST, format!("invalid blinded hex: {}", e)))?;

    if blinded_bytes.len() != 32 {
        return Err((StatusCode::BAD_REQUEST, "blinded must be 32 bytes".into()));
    }

    let blinded = CompressedRistretto::from_slice(&blinded_bytes)
        .map_err(|_| (StatusCode::BAD_REQUEST, "invalid compressed point".into()))?
        .decompress()
        .ok_or_else(|| (StatusCode::BAD_REQUEST, "point decompression failed".into()))?;

    // evaluate OPRF: response = blinded * share
    let response_point = blinded * state.oprf_share;

    // create DLEQ proof
    let proof = DleqProof::create(
        &state.oprf_share,
        &blinded,
        &response_point,
        &state.oprf_pubkey,
    );

    // decode encrypted seed
    let encrypted_seed = hex::decode(&req.encrypted_seed)
        .map_err(|e| (StatusCode::BAD_REQUEST, format!("invalid encrypted_seed hex: {}", e)))?;

    // store OPRF registration
    let reg = OprfRegistration {
        encrypted_seed,
        allowed_guesses: req.allowed_guesses.min(10),
        attempted_guesses: 0,
        created_at: std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs(),
    };

    let reg_bytes = serde_json::to_vec(&reg)
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    state.oprf_db.insert(&req.user_id, reg_bytes)
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    // create verified response
    let oprf_response = VerifiedOprfResponse {
        server_index: state.index,
        point: response_point.compress().as_bytes().to_vec().try_into().unwrap(),
        proof,
    };

    counter!("vault_oprf_registrations_total").increment(1);
    histogram!("vault_request_duration_seconds", "endpoint" => "oprf_register")
        .record(start.elapsed().as_secs_f64());

    info!("oprf registered user {}", &req.user_id[..16.min(req.user_id.len())]);

    Ok(Json(OprfRegisterResponse {
        ok: true,
        response: Some(oprf_response),
        error: None,
    }))
}

async fn oprf_recover(
    State(state): State<Arc<RwLock<AppState>>>,
    Json(req): Json<OprfRecoverRequest>,
) -> Result<Json<OprfRecoverResponse>, (StatusCode, String)> {
    let start = Instant::now();
    counter!("vault_requests_total", "endpoint" => "oprf_recover").increment(1);

    let mut state = state.write().await;

    // lookup registration
    let reg_bytes = match state.oprf_db.get(&req.user_id) {
        Ok(Some(bytes)) => bytes,
        Ok(None) => {
            counter!("vault_errors_total", "endpoint" => "oprf_recover", "error" => "not_found").increment(1);
            return Ok(Json(OprfRecoverResponse {
                ok: false,
                response: None,
                encrypted_seed: None,
                guesses_remaining: 0,
                error: Some("not registered".into()),
            }));
        }
        Err(e) => return Err((StatusCode::INTERNAL_SERVER_ERROR, e.to_string())),
    };

    let mut reg: OprfRegistration = serde_json::from_slice(&reg_bytes)
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    // check rate limit
    if reg.attempted_guesses >= reg.allowed_guesses {
        // delete registration after too many attempts
        state.oprf_db.remove(&req.user_id).ok();
        counter!("vault_oprf_lockouts_total").increment(1);
        warn!("oprf user {} locked out, deleting", &req.user_id[..16.min(req.user_id.len())]);

        return Ok(Json(OprfRecoverResponse {
            ok: false,
            response: None,
            encrypted_seed: None,
            guesses_remaining: 0,
            error: Some("account locked - too many attempts".into()),
        }));
    }

    // decode blinded point
    let blinded_bytes = hex::decode(&req.blinded)
        .map_err(|e| (StatusCode::BAD_REQUEST, format!("invalid blinded hex: {}", e)))?;

    if blinded_bytes.len() != 32 {
        return Err((StatusCode::BAD_REQUEST, "blinded must be 32 bytes".into()));
    }

    let blinded = CompressedRistretto::from_slice(&blinded_bytes)
        .map_err(|_| (StatusCode::BAD_REQUEST, "invalid compressed point".into()))?
        .decompress()
        .ok_or_else(|| (StatusCode::BAD_REQUEST, "point decompression failed".into()))?;

    // evaluate OPRF: response = blinded * share
    let response_point = blinded * state.oprf_share;

    // create DLEQ proof
    let proof = DleqProof::create(
        &state.oprf_share,
        &blinded,
        &response_point,
        &state.oprf_pubkey,
    );

    // increment attempt counter (will be reset on successful client-side decryption)
    // note: in OPRF, we can't verify correctness server-side, so we always count
    reg.attempted_guesses += 1;
    let remaining = reg.allowed_guesses.saturating_sub(reg.attempted_guesses);

    let reg_bytes = serde_json::to_vec(&reg)
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    state.oprf_db.insert(&req.user_id, reg_bytes)
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    // create verified response
    let oprf_response = VerifiedOprfResponse {
        server_index: state.index,
        point: response_point.compress().as_bytes().to_vec().try_into().unwrap(),
        proof,
    };

    counter!("vault_oprf_recoveries_total").increment(1);
    histogram!("vault_request_duration_seconds", "endpoint" => "oprf_recover")
        .record(start.elapsed().as_secs_f64());

    info!(
        "oprf recover for user {}, {} attempts remaining",
        &req.user_id[..16.min(req.user_id.len())],
        remaining
    );

    Ok(Json(OprfRecoverResponse {
        ok: true,
        response: Some(oprf_response),
        encrypted_seed: Some(hex::encode(&reg.encrypted_seed)),
        guesses_remaining: remaining,
        error: None,
    }))
}

/// reset attempt counter after successful recovery (client calls this)
async fn oprf_confirm(
    State(state): State<Arc<RwLock<AppState>>>,
    Path(user_id): Path<String>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    let state = state.write().await;

    let reg_bytes = state.oprf_db.get(&user_id)
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
        .ok_or_else(|| (StatusCode::NOT_FOUND, "not registered".into()))?;

    let mut reg: OprfRegistration = serde_json::from_slice(&reg_bytes)
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    // reset attempts on successful recovery confirmation
    reg.attempted_guesses = 0;

    let reg_bytes = serde_json::to_vec(&reg)
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    state.oprf_db.insert(&user_id, reg_bytes)
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    info!("oprf confirmed for user {}", &user_id[..16.min(user_id.len())]);

    Ok(Json(serde_json::json!({ "ok": true })))
}

/// create realm based on mode, returns (realm, optional tpm info)
fn create_realm(mode: RealmMode, _data_dir: &str) -> (Box<dyn Realm>, Option<TpmInfoResponse>) {
    match mode {
        RealmMode::Software => {
            #[cfg(feature = "software")]
            {
                info!("using software realm (dev mode - no hardware security)");
                (Box::new(ghettobox::SoftwareRealm::new()), None)
            }
            #[cfg(not(feature = "software"))]
            {
                error!("software realm not compiled in, use --features software");
                std::process::exit(1);
            }
        }
        RealmMode::Tpm => {
            #[cfg(feature = "tpm")]
            {
                info!("using tpm realm (hardware security enabled)");
                match ghettobox::TpmRealm::new(_data_dir) {
                    Ok(realm) => {
                        let info = realm.tpm_info();
                        let tpm_resp = TpmInfoResponse {
                            manufacturer: info.manufacturer.clone(),
                            firmware_version: info.firmware_version.clone(),
                            tpm_type: info.tpm_type.to_string(),
                            is_virtual: info.is_virtual,
                        };

                        info!("  tpm manufacturer: {}", info.manufacturer);
                        info!("  tpm firmware: {}", info.firmware_version);
                        info!("  tpm type: {}", info.tpm_type);

                        if info.is_virtual {
                            warn!("⚠️  virtual tpm detected - lower security than hardware tpm");
                        }

                        (Box::new(realm), Some(tpm_resp))
                    }
                    Err(e) => {
                        error!("failed to initialize tpm: {}", e);
                        error!("hints:");
                        error!("  - check /dev/tpm0 or /dev/tpmrm0 exists");
                        error!("  - check permissions (user needs tss group or root)");
                        error!("  - for containers, ensure device passthrough");
                        std::process::exit(1);
                    }
                }
            }
            #[cfg(not(feature = "tpm"))]
            {
                error!("tpm realm not compiled in, use --features tpm");
                std::process::exit(1);
            }
        }
        RealmMode::Hsm => {
            error!("hsm realm not yet implemented");
            error!("use --mode software or --mode tpm");
            std::process::exit(1);
        }
    }
}

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::from_default_env()
                .add_directive("ghettobox_vault=info".parse().unwrap())
        )
        .init();

    let args = Args::parse();

    // setup prometheus metrics exporter
    let metrics_port = args.metrics_port.unwrap_or(args.port + 1000);
    let metrics_addr: std::net::SocketAddr = format!("{}:{}", args.bind, metrics_port)
        .parse()
        .expect("invalid metrics address");

    PrometheusBuilder::new()
        .with_http_listener(metrics_addr)
        .install()
        .expect("failed to install prometheus metrics exporter");

    let data_dir = args.data_dir.unwrap_or_else(|| {
        let home = std::env::var("HOME").unwrap_or_else(|_| ".".into());
        format!("{}/.ghettobox-vault", home)
    });
    std::fs::create_dir_all(&data_dir).expect("failed to create data dir");

    let db_path = format!("{}/db", data_dir);
    let db = sled::open(&db_path).expect("failed to open database");

    // separate tree for OPRF registrations
    let oprf_db = db.open_tree("oprf").expect("failed to open oprf tree");

    let key_path = format!("{}/node.key", data_dir);
    let signing_key = if std::path::Path::new(&key_path).exists() {
        let key_bytes = std::fs::read(&key_path).expect("failed to read key");
        let key_arr: [u8; 32] = key_bytes.try_into().expect("invalid key length");
        SigningKey::from_bytes(&key_arr)
    } else {
        let key = SigningKey::generate(&mut rand::thread_rng());
        std::fs::write(&key_path, key.to_bytes()).expect("failed to write key");
        key
    };

    // load or generate OPRF share
    let oprf_key_path = format!("{}/oprf.key", data_dir);
    let oprf_share = if std::path::Path::new(&oprf_key_path).exists() {
        let key_bytes = std::fs::read(&oprf_key_path).expect("failed to read oprf key");
        let key_arr: [u8; 32] = key_bytes.try_into().expect("invalid oprf key length");
        Scalar::from_bytes_mod_order(key_arr)
    } else {
        // generate random scalar for OPRF share
        let mut scalar_bytes = [0u8; 64];
        rand::RngCore::fill_bytes(&mut rand::thread_rng(), &mut scalar_bytes);
        let share = Scalar::from_bytes_mod_order_wide(&scalar_bytes);
        // store the canonical 32-byte form
        std::fs::write(&oprf_key_path, share.as_bytes()).expect("failed to write oprf key");
        share
    };

    // compute OPRF public key: G * share
    let oprf_pubkey = RISTRETTO_BASEPOINT_POINT * oprf_share;

    let (realm, tpm_info) = create_realm(args.mode, &data_dir);

    let pubkey = hex::encode(signing_key.verifying_key().to_bytes());
    let oprf_pubkey_hex = hex::encode(oprf_pubkey.compress().as_bytes());

    info!("ghettobox-vault v{}", env!("CARGO_PKG_VERSION"));
    info!("  index: {}", args.index);
    info!("  mode: {}", args.mode);
    info!("  ed25519 pubkey: {}", pubkey);
    info!("  oprf pubkey: {}", oprf_pubkey_hex);
    info!("  data: {}", data_dir);
    info!("  bind: {}:{}", args.bind, args.port);
    info!("  metrics: {}:{}", args.bind, metrics_port);

    if args.mode == RealmMode::Software {
        warn!("⚠️  software mode has no hardware security - use only for testing");
    }

    // set initial gauge values
    gauge!("vault_registrations_current").set(db.len() as f64);

    let state = Arc::new(RwLock::new(AppState {
        db,
        oprf_db,
        realm,
        signing_key,
        oprf_share,
        oprf_pubkey,
        index: args.index,
        mode: args.mode,
        tpm_info,
    }));

    let app = Router::new()
        // info endpoints
        .route("/", get(node_info))
        .route("/health", get(health))
        // legacy protocol
        .route("/register", post(register))
        .route("/recover", post(recover))
        .route("/status/{user_id}", get(status))
        // OPRF protocol (verified with DLEQ proofs)
        .route("/oprf/health", get(oprf_health))
        .route("/oprf/register", post(oprf_register))
        .route("/oprf/recover", post(oprf_recover))
        .route("/oprf/confirm/{user_id}", post(oprf_confirm))
        .layer(CorsLayer::permissive())
        .with_state(state);

    let addr = format!("{}:{}", args.bind, args.port);
    let listener = tokio::net::TcpListener::bind(&addr).await.unwrap();
    info!("listening on {}", addr);

    axum::serve(listener, app).await.unwrap();
}
