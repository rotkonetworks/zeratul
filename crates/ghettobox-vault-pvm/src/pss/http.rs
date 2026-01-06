//! HTTP handlers for PSS reshare coordination
//!
//! endpoints:
//! - GET  /reshare/epoch        - get current reshare epoch
//! - POST /reshare/epoch        - start new reshare epoch
//! - POST /reshare/commitment   - submit dealer commitment
//! - GET  /reshare/subshare/:idx - get subshare for player
//! - GET  /reshare/status       - get reshare status

use super::config::ProviderConfig;
use super::reshare::{CommitmentMsg, DealerState, ReshareEpoch, SubShareMsg};
use axum::{
    extract::{Path, State},
    http::StatusCode,
    Json,
};
use curve25519_dalek::scalar::Scalar;
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tokio::sync::Mutex;

/// reshare state held by the vault
pub struct ReshareAppState {
    /// provider config (index, share, group_pubkey)
    pub config: Option<ProviderConfig>,
    /// current reshare epoch (if active)
    pub epoch: Option<ReshareEpoch>,
    /// dealer state (if we're an old provider)
    pub dealer: Option<DealerState>,
}

impl ReshareAppState {
    pub fn new() -> Self {
        Self {
            config: None,
            epoch: None,
            dealer: None,
        }
    }

    pub fn with_config(config: ProviderConfig) -> Self {
        Self {
            config: Some(config),
            epoch: None,
            dealer: None,
        }
    }
}

impl Default for ReshareAppState {
    fn default() -> Self {
        Self::new()
    }
}

/// request to start a new reshare epoch
#[derive(Debug, Serialize, Deserialize)]
pub struct StartEpochRequest {
    pub epoch: u64,
    pub old_threshold: u32,
    pub new_threshold: u32,
    pub old_provider_count: u32,
    pub new_provider_count: u32,
}

/// response for epoch endpoints
#[derive(Debug, Serialize, Deserialize)]
pub struct EpochResponse {
    pub active: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub epoch: Option<ReshareEpoch>,
}

/// response for status endpoint
#[derive(Debug, Serialize, Deserialize)]
pub struct ReshareStatusResponse {
    pub configured: bool,
    pub provider_index: Option<u32>,
    pub reshare_active: bool,
    pub epoch: Option<u64>,
    pub commitment_count: Option<usize>,
    pub has_quorum: Option<bool>,
}

/// get current reshare epoch
pub async fn get_epoch(
    State(state): State<Arc<Mutex<ReshareAppState>>>,
) -> Json<EpochResponse> {
    let state = state.lock().await;
    Json(EpochResponse {
        active: state.epoch.is_some(),
        epoch: state.epoch.clone(),
    })
}

/// start a new reshare epoch
pub async fn start_epoch(
    State(state): State<Arc<Mutex<ReshareAppState>>>,
    Json(req): Json<StartEpochRequest>,
) -> Result<Json<EpochResponse>, (StatusCode, String)> {
    let mut state = state.lock().await;

    // need config to participate
    let config = state.config.as_ref()
        .ok_or((StatusCode::PRECONDITION_FAILED, "provider not configured".into()))?;

    let group_pubkey = config.group_pubkey_point()
        .ok_or((StatusCode::PRECONDITION_FAILED, "group pubkey not set".into()))?;

    // check if epoch already active
    if let Some(ref epoch) = state.epoch {
        if epoch.epoch == req.epoch {
            return Ok(Json(EpochResponse {
                active: true,
                epoch: Some(epoch.clone()),
            }));
        }
    }

    // create new epoch
    let epoch = ReshareEpoch::new(
        req.epoch,
        req.old_provider_count,
        req.old_threshold,
        req.new_threshold,
        req.new_provider_count,
        &group_pubkey,
    );

    // if we're an old provider (have a share), become a dealer
    if let Some(share_scalar) = config.share_scalar() {
        let dealer = DealerState::new(
            config.index,
            share_scalar,
            req.new_threshold,
            req.new_provider_count,
        );
        state.dealer = Some(dealer);
    }

    state.epoch = Some(epoch.clone());

    Ok(Json(EpochResponse {
        active: true,
        epoch: Some(epoch),
    }))
}

/// submit a dealer commitment
pub async fn submit_commitment(
    State(state): State<Arc<Mutex<ReshareAppState>>>,
    Json(commitment): Json<CommitmentMsg>,
) -> Result<Json<CommitmentSubmitResponse>, (StatusCode, String)> {
    let mut state = state.lock().await;

    let epoch = state.epoch.as_mut()
        .ok_or((StatusCode::PRECONDITION_FAILED, "no active reshare epoch".into()))?;

    let accepted = epoch.submit_commitment(commitment)
        .map_err(|e| (StatusCode::BAD_REQUEST, e.to_string()))?;

    Ok(Json(CommitmentSubmitResponse {
        accepted,
        commitment_count: epoch.commitment_count(),
        has_quorum: epoch.has_quorum(),
    }))
}

#[derive(Debug, Serialize, Deserialize)]
pub struct CommitmentSubmitResponse {
    pub accepted: bool,
    pub commitment_count: usize,
    pub has_quorum: bool,
}

/// get our commitment (if we're a dealer)
pub async fn get_commitment(
    State(state): State<Arc<Mutex<ReshareAppState>>>,
) -> Result<Json<CommitmentMsg>, (StatusCode, String)> {
    let state = state.lock().await;

    let dealer = state.dealer.as_ref()
        .ok_or((StatusCode::NOT_FOUND, "not a dealer in this epoch".into()))?;

    Ok(Json(dealer.commitment()))
}

/// get subshare for a specific player
pub async fn get_subshare(
    State(state): State<Arc<Mutex<ReshareAppState>>>,
    Path(player_index): Path<u32>,
) -> Result<Json<SubShareMsg>, (StatusCode, String)> {
    let state = state.lock().await;

    let dealer = state.dealer.as_ref()
        .ok_or((StatusCode::NOT_FOUND, "not a dealer in this epoch".into()))?;

    let subshare = dealer.generate_subshare(player_index)
        .ok_or((StatusCode::BAD_REQUEST, "invalid player index".into()))?;

    Ok(Json(subshare))
}

/// get reshare status
pub async fn reshare_status(
    State(state): State<Arc<Mutex<ReshareAppState>>>,
) -> Json<ReshareStatusResponse> {
    let state = state.lock().await;

    let configured = state.config.is_some();
    let provider_index = state.config.as_ref().map(|c| c.index);

    if let Some(ref epoch) = state.epoch {
        Json(ReshareStatusResponse {
            configured,
            provider_index,
            reshare_active: true,
            epoch: Some(epoch.epoch),
            commitment_count: Some(epoch.commitment_count()),
            has_quorum: Some(epoch.has_quorum()),
        })
    } else {
        Json(ReshareStatusResponse {
            configured,
            provider_index,
            reshare_active: false,
            epoch: None,
            commitment_count: None,
            has_quorum: None,
        })
    }
}

/// verify group key reconstruction after quorum reached
pub async fn verify_group_key(
    State(state): State<Arc<Mutex<ReshareAppState>>>,
) -> Result<Json<VerifyResponse>, (StatusCode, String)> {
    let state = state.lock().await;

    let epoch = state.epoch.as_ref()
        .ok_or((StatusCode::PRECONDITION_FAILED, "no active reshare epoch".into()))?;

    if !epoch.has_quorum() {
        return Err((StatusCode::PRECONDITION_FAILED, "quorum not reached".into()));
    }

    let valid = epoch.verify_group_key()
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    Ok(Json(VerifyResponse { valid }))
}

#[derive(Debug, Serialize, Deserialize)]
pub struct VerifyResponse {
    pub valid: bool,
}

#[cfg(test)]
mod tests {
    use super::*;
    use curve25519_dalek::constants::RISTRETTO_BASEPOINT_POINT;

    #[test]
    fn test_reshare_state_default() {
        let state = ReshareAppState::new();
        assert!(state.config.is_none());
        assert!(state.epoch.is_none());
        assert!(state.dealer.is_none());
    }

    #[test]
    fn test_reshare_state_with_config() {
        let config = ProviderConfig {
            index: 1,
            port: 4200,
            peers: vec![],
            signing_key: "deadbeef".into(),
            group_pubkey: Some(hex::encode(RISTRETTO_BASEPOINT_POINT.compress().as_bytes())),
            share: Some(hex::encode(Scalar::from(42u64).as_bytes())),
        };

        let state = ReshareAppState::with_config(config);
        assert!(state.config.is_some());
        assert_eq!(state.config.as_ref().unwrap().index, 1);
    }
}
