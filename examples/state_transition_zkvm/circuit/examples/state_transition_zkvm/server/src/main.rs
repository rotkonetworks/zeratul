//! State Transition Verifier Server
//!
//! Stateless HTTP server that verifies zero-knowledge proofs and updates NOMT state.

use axum::{
    extract::State,
    http::StatusCode,
    routing::{get, post},
    Json, Router,
};
use serde::{Deserialize, Serialize};
use state_transition_circuit::{StateTransitionProof, verify_and_extract_commitments};
use std::sync::Arc;
use tokio::sync::RwLock;
use tower_http::cors::CorsLayer;
use tracing::{info, error};

mod nomt_storage;
use nomt_storage::NomtStorage;

/// Application state shared across all requests
struct AppState {
    storage: Arc<RwLock<NomtStorage>>,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // Initialize tracing
    tracing_subscriber::fmt()
        .with_env_filter("info")
        .init();

    info!("Starting State Transition Verifier Server");

    // Initialize NOMT storage
    let storage_path = std::env::var("NOMT_DATA_DIR")
        .unwrap_or_else(|_| "./nomt_data".to_string());
    
    info!("Initializing NOMT storage at: {}", storage_path);
    let storage = NomtStorage::new(&storage_path)?;

    let app_state = Arc::new(AppState {
        storage: Arc::new(RwLock::new(storage)),
    });

    // Build router
    let app = Router::new()
        .route("/health", get(health_check))
        .route("/ready", get(ready_check))
        .route("/api/state_root", get(get_state_root))
        .route("/api/submit_transaction", post(submit_transaction))
        .layer(CorsLayer::permissive())
        .with_state(app_state);

    // Start server
    let addr = "0.0.0.0:8080";
    info!("Server listening on {}", addr);
    
    let listener = tokio::net::TcpListener::bind(addr).await?;
    axum::serve(listener, app).await?;

    Ok(())
}

/// Health check endpoint
async fn health_check() -> &'static str {
    "OK"
}

/// Readiness check endpoint
async fn ready_check() -> &'static str {
    "READY"
}

/// Get current state root
#[derive(Serialize)]
struct StateRootResponse {
    root: String,
}

async fn get_state_root(
    State(state): State<Arc<AppState>>,
) -> Result<Json<StateRootResponse>, (StatusCode, String)> {
    let storage = state.storage.read().await;
    let root = storage.get_root()
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    
    Ok(Json(StateRootResponse {
        root: hex::encode(root),
    }))
}

/// Submit a transaction proof
#[derive(Deserialize)]
struct SubmitTransactionRequest {
    proof: StateTransitionProof,
    sender_id: u64,
    receiver_id: u64,
}

#[derive(Serialize)]
struct SubmitTransactionResponse {
    status: String,
    new_state_root: String,
    transaction_id: String,
}

async fn submit_transaction(
    State(state): State<Arc<AppState>>,
    Json(req): Json<SubmitTransactionRequest>,
) -> Result<Json<SubmitTransactionResponse>, (StatusCode, String)> {
    info!("Received transaction: {} -> {}", req.sender_id, req.receiver_id);

    // Verify the proof
    let verified = verify_and_extract_commitments(&req.proof)
        .map_err(|e| {
            error!("Proof verification failed: {}", e);
            (StatusCode::BAD_REQUEST, format!("Invalid proof: {}", e))
        })?;

    info!("Proof verified successfully");

    // Update NOMT storage
    let mut storage = state.storage.write().await;
    
    storage.update_commitments(
        req.sender_id,
        &verified.sender_commitment_new,
        req.receiver_id,
        &verified.receiver_commitment_new,
    ).map_err(|e| {
        error!("Storage update failed: {}", e);
        (StatusCode::INTERNAL_SERVER_ERROR, e.to_string())
    })?;

    let new_root = storage.get_root()
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    // Compute transaction ID
    let tx_id = compute_tx_id(&verified);

    info!("Transaction {} committed with new root: {}", 
          hex::encode(&tx_id), hex::encode(&new_root));

    Ok(Json(SubmitTransactionResponse {
        status: "accepted".to_string(),
        new_state_root: hex::encode(new_root),
        transaction_id: hex::encode(tx_id),
    }))
}

/// Compute transaction ID from verified transition
fn compute_tx_id(verified: &state_transition_circuit::VerifiedTransition) -> [u8; 32] {
    use sha2::{Sha256, Digest};
    let mut hasher = Sha256::new();
    hasher.update(&verified.sender_commitment_old);
    hasher.update(&verified.sender_commitment_new);
    hasher.update(&verified.receiver_commitment_old);
    hasher.update(&verified.receiver_commitment_new);
    hasher.finalize().into()
}
