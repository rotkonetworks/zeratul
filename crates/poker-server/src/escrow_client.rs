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

/// Subset of `GET /room/{code}` we care about for deposit gating.
#[derive(Debug, Clone, Deserialize)]
pub struct EscrowState {
    #[serde(default)]
    pub escrow_address: Option<String>,
    #[serde(default)]
    pub seat_addresses: Vec<Option<String>>,
    /// per-seat personal payout addresses recovered from the `zk.poker/v1/payout:` memo
    #[serde(default)]
    pub seat_payout_addresses: Vec<Option<String>>,
    #[serde(default)]
    pub player_a_deposit: u64,
    #[serde(default)]
    pub player_b_deposit: u64,
    #[serde(default)]
    pub required_deposit: u64,
    #[serde(default)]
    pub both_deposited: bool,
    #[serde(default)]
    pub both_payout_addresses_known: bool,
}

/// GET /room/{code} — current escrow + deposit state for the poker-server poll loop.
pub async fn get_room_state(base_url: &str, code: &str) -> Result<EscrowState, String> {
    let url = format!("{}/room/{}", base_url.trim_end_matches('/'), code);
    let resp = reqwest::Client::new()
        .get(&url)
        .timeout(Duration::from_secs(5))
        .send()
        .await
        .map_err(|e| format!("escrow GET failed: {}", e))?;
    let v: serde_json::Value = resp
        .json()
        .await
        .map_err(|e| format!("escrow response not JSON: {}", e))?;
    if let Some(err) = v.get("error") {
        return Err(format!("escrow service error: {}", err));
    }
    serde_json::from_value(v).map_err(|e| format!("escrow state shape mismatch: {}", e))
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
