//! Minimal HTTP server for Ligerito proof verification
//!
//! This example demonstrates a simple HTTP API for verifying Ligerito proofs.
//! It's designed to be deployable in constrained environments like PolkaVM.
//!
//! # Usage
//!
//! ```bash
//! cargo run --manifest-path examples/http_verifier_server/Cargo.toml
//! ```
//!
//! # API Endpoints
//!
//! - POST /verify - Verify a proof (accepts bincode-encoded proof)
//! - GET /health - Health check endpoint
//! - GET /config - Get supported proof sizes

use axum::{
    body::Bytes,
    extract::State,
    http::StatusCode,
    response::{IntoResponse, Json},
    routing::{get, post},
    Router,
};
use ligerito_binary_fields::{BinaryElem32, BinaryElem128, BinaryFieldElement};
use ligerito::{
    hardcoded_config_12_verifier, hardcoded_config_16_verifier,
    hardcoded_config_20_verifier, hardcoded_config_24_verifier,
    verify, FinalizedLigeritoProof,
};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tower_http::cors::CorsLayer;
use tracing::{info, warn};

#[derive(Clone)]
struct AppState {
    // Could add rate limiting, metrics, etc. here
}

#[derive(Debug, Serialize, Deserialize)]
struct VerifyRequest {
    /// Proof size (log2 of polynomial size)
    proof_size: u32,
    /// Bincode-encoded proof as byte array
    proof_bytes: Vec<u8>,
}

#[derive(Debug, Serialize)]
struct VerifyResponse {
    valid: bool,
    proof_size: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<String>,
}

#[derive(Debug, Serialize)]
struct HealthResponse {
    status: &'static str,
    version: &'static str,
}

#[derive(Debug, Serialize)]
struct ConfigResponse {
    supported_sizes: Vec<u32>,
    verifier_only: bool,
}

/// Verify a Ligerito proof
async fn verify_proof(
    State(_state): State<Arc<AppState>>,
    body: Bytes,
) -> impl IntoResponse {
    // Parse the request
    let request: VerifyRequest = match serde_json::from_slice(&body) {
        Ok(req) => req,
        Err(e) => {
            warn!("Failed to parse request: {}", e);
            return (
                StatusCode::BAD_REQUEST,
                Json(VerifyResponse {
                    valid: false,
                    proof_size: 0,
                    error: Some(format!("Invalid request: {}", e)),
                }),
            );
        }
    };

    info!("Verifying proof for size 2^{}", request.proof_size);

    // Verify based on proof size
    let result = match request.proof_size {
        12 => verify_proof_size::<BinaryElem32, BinaryElem128>(
            &request.proof_bytes,
            hardcoded_config_12_verifier(),
        ),
        16 => verify_proof_size::<BinaryElem32, BinaryElem128>(
            &request.proof_bytes,
            hardcoded_config_16_verifier(),
        ),
        20 => verify_proof_size::<BinaryElem32, BinaryElem128>(
            &request.proof_bytes,
            hardcoded_config_20_verifier(),
        ),
        24 => verify_proof_size::<BinaryElem32, BinaryElem128>(
            &request.proof_bytes,
            hardcoded_config_24_verifier(),
        ),
        size => {
            warn!("Unsupported proof size: 2^{}", size);
            return (
                StatusCode::BAD_REQUEST,
                Json(VerifyResponse {
                    valid: false,
                    proof_size: size,
                    error: Some(format!("Unsupported proof size: 2^{}", size)),
                }),
            );
        }
    };

    match result {
        Ok(valid) => {
            info!("Verification result: {}", valid);
            (
                StatusCode::OK,
                Json(VerifyResponse {
                    valid,
                    proof_size: request.proof_size,
                    error: None,
                }),
            )
        }
        Err(e) => {
            warn!("Verification error: {}", e);
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(VerifyResponse {
                    valid: false,
                    proof_size: request.proof_size,
                    error: Some(format!("Verification error: {}", e)),
                }),
            )
        }
    }
}

fn verify_proof_size<T, U>(
    proof_bytes: &[u8],
    config: ligerito::VerifierConfig,
) -> Result<bool, Box<dyn std::error::Error>>
where
    T: BinaryFieldElement + serde::de::DeserializeOwned,
    U: BinaryFieldElement + From<T> + serde::de::DeserializeOwned,
{
    // Deserialize the proof
    let proof: FinalizedLigeritoProof<T, U> = bincode::deserialize(proof_bytes)?;

    // Verify
    let valid = verify(&config, &proof)?;

    Ok(valid)
}

/// Health check endpoint
async fn health() -> impl IntoResponse {
    Json(HealthResponse {
        status: "ok",
        version: env!("CARGO_PKG_VERSION"),
    })
}

/// Get supported configuration
async fn config() -> impl IntoResponse {
    Json(ConfigResponse {
        supported_sizes: vec![12, 16, 20, 24],
        verifier_only: true,
    })
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // Initialize tracing
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "http_verifier_server=info,tower_http=debug".into()),
        )
        .init();

    let state = Arc::new(AppState {});

    // Build the router
    let app = Router::new()
        .route("/verify", post(verify_proof))
        .route("/health", get(health))
        .route("/config", get(config))
        .layer(CorsLayer::permissive())
        .layer(tower_http::trace::TraceLayer::new_for_http())
        .with_state(state);

    // Run the server
    let addr = "0.0.0.0:3000";
    info!("Starting HTTP verifier server on {}", addr);

    let listener = tokio::net::TcpListener::bind(addr).await?;
    axum::serve(listener, app).await?;

    Ok(())
}
