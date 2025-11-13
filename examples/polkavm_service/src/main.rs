//! PolkaVM Verifier Service (HOST)
//!
//! HTTP/WebSocket service that manages a PolkaVM instance running the Ligerito verifier.
//!
//! Architecture:
//! - HOST: This Rust service (HTTP API with axum)
//! - GUEST: PolkaVM binary (ligerito_verifier.polkavm)
//! - Communication: stdin/stdout + exit codes

use axum::{
    extract::State,
    http::StatusCode,
    response::{IntoResponse, Json},
    routing::{get, post},
    Router,
};
use serde::{Deserialize, Serialize};
use std::{path::PathBuf, sync::Arc};
use tower_http::cors::CorsLayer;

mod polkavm_runner;
use polkavm_runner::PolkaVMRunner;

#[derive(Clone)]
struct AppState {
    runner: Arc<PolkaVMRunner>,
}

#[derive(Debug, Serialize, Deserialize)]
struct VerifyRequest {
    /// Proof size: 12, 16, 20, 24, 28, or 30
    proof_size: u32,
    /// Bincode-serialized proof bytes (base64 or hex encoded)
    #[serde(with = "base64_bytes")]
    proof_bytes: Vec<u8>,
}

#[derive(Debug, Serialize)]
struct VerifyResponse {
    valid: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<String>,
    execution_time_ms: u64,
}

#[derive(Debug, Serialize)]
struct HealthResponse {
    status: String,
    polkavm_loaded: bool,
}

#[derive(Debug, Serialize)]
struct ConfigResponse {
    supported_sizes: Vec<u32>,
    max_proof_bytes: usize,
}

/// Verify a proof using the PolkaVM guest
async fn verify_proof(
    State(state): State<AppState>,
    Json(request): Json<VerifyRequest>,
) -> Result<Json<VerifyResponse>, AppError> {
    let start = std::time::Instant::now();

    // Validate proof size
    if !matches!(request.proof_size, 12 | 16 | 20 | 24 | 28 | 30) {
        return Ok(Json(VerifyResponse {
            valid: false,
            error: Some(format!("Unsupported proof size: {}", request.proof_size)),
            execution_time_ms: start.elapsed().as_millis() as u64,
        }));
    }

    // Prepare input: [config_size: u32][proof_bytes]
    let mut input = Vec::with_capacity(4 + request.proof_bytes.len());
    input.extend_from_slice(&request.proof_size.to_le_bytes());
    input.extend_from_slice(&request.proof_bytes);

    // Run PolkaVM guest
    match state.runner.execute(&input).await {
        Ok(result) => {
            let execution_time_ms = start.elapsed().as_millis() as u64;
            Ok(Json(VerifyResponse {
                valid: result.exit_code == 0,
                error: if result.exit_code == 2 {
                    Some(result.stderr)
                } else if result.exit_code != 0 {
                    Some(format!("Verification failed (exit code: {})", result.exit_code))
                } else {
                    None
                },
                execution_time_ms,
            }))
        }
        Err(e) => Ok(Json(VerifyResponse {
            valid: false,
            error: Some(format!("PolkaVM execution error: {}", e)),
            execution_time_ms: start.elapsed().as_millis() as u64,
        })),
    }
}

async fn health() -> Json<HealthResponse> {
    Json(HealthResponse {
        status: "ok".to_string(),
        polkavm_loaded: true,
    })
}

async fn config() -> Json<ConfigResponse> {
    Json(ConfigResponse {
        supported_sizes: vec![12, 16, 20, 24, 28, 30],
        max_proof_bytes: 100 * 1024 * 1024, // 100 MB
    })
}

#[derive(Debug)]
struct AppError(anyhow::Error);

impl IntoResponse for AppError {
    fn into_response(self) -> axum::response::Response {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({
                "error": self.0.to_string()
            })),
        )
            .into_response()
    }
}

impl<E> From<E> for AppError
where
    E: Into<anyhow::Error>,
{
    fn from(err: E) -> Self {
        Self(err.into())
    }
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // Initialize tracing
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "polkavm_service=debug,tower_http=debug".into()),
        )
        .init();

    // Load PolkaVM binary
    let polkavm_path = std::env::args()
        .nth(1)
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("../polkavm_verifier/ligerito_verifier.polkavm"));

    tracing::info!("Loading PolkaVM binary from: {}", polkavm_path.display());

    let runner = Arc::new(PolkaVMRunner::new(polkavm_path)?);

    let state = AppState { runner };

    // Build router
    let app = Router::new()
        .route("/verify", post(verify_proof))
        .route("/health", get(health))
        .route("/config", get(config))
        .layer(CorsLayer::permissive())
        .with_state(state);

    // Start server
    let listener = tokio::net::TcpListener::bind("0.0.0.0:3000").await?;
    tracing::info!("PolkaVM Verifier Service listening on http://0.0.0.0:3000");
    tracing::info!("Endpoints:");
    tracing::info!("  POST /verify  - Verify a proof");
    tracing::info!("  GET  /health  - Health check");
    tracing::info!("  GET  /config  - Configuration info");

    axum::serve(listener, app).await?;

    Ok(())
}

/// Helper module for base64 encoding/decoding
mod base64_bytes {
    use serde::{Deserialize, Deserializer, Serializer};

    pub fn serialize<S>(bytes: &Vec<u8>, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(&base64::encode(bytes))
    }

    pub fn deserialize<'de, D>(deserializer: D) -> Result<Vec<u8>, D::Error>
    where
        D: Deserializer<'de>,
    {
        let s = String::deserialize(deserializer)?;
        base64::decode(&s).map_err(serde::de::Error::custom)
    }
}

/// Simple base64 encoding (std-only, no dependencies)
mod base64 {
    const ALPHABET: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

    pub fn encode(input: &[u8]) -> String {
        let mut result = String::new();
        let mut i = 0;
        while i < input.len() {
            let b1 = input[i];
            let b2 = if i + 1 < input.len() { input[i + 1] } else { 0 };
            let b3 = if i + 2 < input.len() { input[i + 2] } else { 0 };

            result.push(ALPHABET[(b1 >> 2) as usize] as char);
            result.push(ALPHABET[(((b1 & 0x3) << 4) | (b2 >> 4)) as usize] as char);
            if i + 1 < input.len() {
                result.push(ALPHABET[(((b2 & 0xF) << 2) | (b3 >> 6)) as usize] as char);
            } else {
                result.push('=');
            }
            if i + 2 < input.len() {
                result.push(ALPHABET[(b3 & 0x3F) as usize] as char);
            } else {
                result.push('=');
            }

            i += 3;
        }
        result
    }

    pub fn decode(input: &str) -> Result<Vec<u8>, String> {
        // Simple hex decoding as fallback
        if input.len() % 2 == 0 && input.chars().all(|c| c.is_ascii_hexdigit()) {
            return hex_decode(input);
        }

        // Otherwise treat as base64 (simplified)
        Err("Base64 decoding not fully implemented, use hex encoding".to_string())
    }

    fn hex_decode(s: &str) -> Result<Vec<u8>, String> {
        (0..s.len())
            .step_by(2)
            .map(|i| {
                u8::from_str_radix(&s[i..i + 2], 16)
                    .map_err(|e| format!("Invalid hex: {}", e))
            })
            .collect()
    }
}
