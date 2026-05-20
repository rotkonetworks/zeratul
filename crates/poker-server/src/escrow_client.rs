//! HTTP client for the poker-escrow service.

use serde::Deserialize;
use std::time::Duration;

/// shares + public_key_package are unused server-side today; will be forwarded
/// to clients for payout co-signing in Phase 5.
#[allow(dead_code)]
#[derive(Debug, Deserialize)]
pub struct EscrowSetup {
    /// Orchard UA — `None` when escrow is in DKG mode and the UA is still being derived.
    #[serde(default)]
    pub escrow_address: Option<String>,
    pub player_a_share: String,
    pub player_b_share: String,
    #[serde(default)]
    pub public_key_package: String,
    /// `Some` when escrow is in DKG mode; clients must join this relay room to participate.
    #[serde(default)]
    pub frost_relay_url: Option<String>,
    #[serde(default)]
    pub frost_room_code: Option<String>,
    #[serde(default)]
    pub dkg_mode: bool,
}

/// POST /room — ask poker-escrow to generate a fresh FROST escrow for a room.
pub async fn create_escrow(
    base_url: &str,
    code: &str,
    required_deposit: u64,
    rake_bps: u16,
) -> Result<EscrowSetup, String> {
    let url = format!("{}/room", base_url.trim_end_matches('/'));
    let body = serde_json::json!({
        "code": code,
        "required_deposit": required_deposit,
        "rake_bps": rake_bps,
    });
    let resp = reqwest::Client::new()
        .post(&url)
        .json(&body)
        .timeout(Duration::from_secs(5))
        .send()
        .await
        .map_err(|e| format!("escrow POST failed: {}", e))?;
    let v: serde_json::Value = resp
        .json()
        .await
        .map_err(|e| format!("escrow response not JSON: {}", e))?;
    if let Some(err) = v.get("error") {
        return Err(format!("escrow service error: {}", err));
    }
    serde_json::from_value(v).map_err(|e| format!("escrow response shape mismatch: {}", e))
}
