//! HTTP client for the poker-escrow service.

use serde::{Deserialize, Serialize};
use std::time::Duration;

/// One line of a PCZT payout plan — paired (recipient UA, amount zat).
#[derive(Debug, Clone, Serialize)]
pub struct PayoutOutputReq {
    pub address: String,
    pub amount_zat: u64,
}

#[derive(Debug, Clone, Serialize)]
pub struct InitiatePayoutReq {
    pub outputs: Vec<PayoutOutputReq>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fee_zat: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub anchor_height: Option<u32>,
    /// Per-room capability token minted by the escrow at room creation. The escrow's
    /// `/payout/initiate` is gated on it (fail-closed), so it MUST be forwarded here.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub payout_token: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct InitiatePayoutResp {
    /// The FROST relay room the two seats join to co-sign (2-of-3).
    ///
    /// `Some` on the synchronous payout paths (`/payout/initiate`, incl. the settled case) where
    /// the escrow opens the relay room before responding.
    ///
    /// `None` on `/cancel`: that handler records the refund plan and responds IMMEDIATELY, then
    /// opens the relay room in a background task. The caller must resolve it afterwards by polling
    /// `GET /payout/status` until `Pending { relay_room }` (see `resolve_relay_room`). Optional so
    /// the cancel response (which carries no `relay_room`) still deserializes instead of erroring
    /// out and stranding the refund co-sign.
    #[serde(default)]
    pub relay_room: Option<String>,
    /// The escrow-computed payout/refund plan, for DISPLAY in the signing view. Present when the
    /// escrow is the split authority (settled `/payout/initiate`, `/cancel`): the server holds no
    /// amounts of its own, so it echoes what the escrow computed. Absent (`#[serde(default)]` →
    /// empty) on the legacy `/payout/initiate` path where the server passed the outputs in.
    #[serde(default)]
    pub plan: Vec<PayoutPlanLine>,
}

/// Poll `GET /room/{code}/payout/status` until the escrow has opened the FROST relay room and
/// reports it (`Pending`/`Broadcast`). Used on the `/cancel` path, where the relay room is created
/// in a background task AFTER the immediate response. Fails after ~30s so a stuck refund surfaces
/// as a `PayoutFailed` rather than hanging forever. On `Broadcast` (already paid out) or `Failed`,
/// returns the relay room / an error so the caller can short-circuit.
pub async fn resolve_relay_room(base_url: &str, code: &str) -> Result<String, String> {
    for _ in 0..15 {
        match get_payout_status(base_url, code).await {
            Ok(PayoutStatus::Pending { relay_room }) => return Ok(relay_room),
            Ok(PayoutStatus::Broadcast { relay_room, .. }) => return Ok(relay_room),
            Ok(PayoutStatus::Failed { reason }) => return Err(format!("escrow payout failed: {}", reason)),
            Ok(PayoutStatus::None) => {}      // relay room not opened yet — keep polling
            Err(e) => tracing::warn!("resolve_relay_room {}: status poll error: {}", code, e),
        }
        tokio::time::sleep(Duration::from_secs(2)).await;
    }
    Err("escrow did not open a relay room within 30s".to_string())
}

/// One display line of an escrow-computed plan, mirrored back for the `PayoutSigningRequest`.
#[derive(Debug, Clone, Deserialize)]
pub struct PayoutPlanLine {
    pub seat: u8,
    pub address: String,
    pub amount_zat: u64,
}

/// Mirror of poker-escrow's `PayoutStatus` enum.
#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "phase")]
pub enum PayoutStatus {
    None,
    Pending { relay_room: String },
    Broadcast { txid: String, relay_room: String },
    Failed { reason: String },
}

pub async fn initiate_payout(base_url: &str, code: &str, req: &InitiatePayoutReq)
    -> Result<InitiatePayoutResp, String>
{
    let url = format!("{}/room/{}/payout/initiate", base_url.trim_end_matches('/'), code);
    let resp = reqwest::Client::new()
        .post(&url)
        .json(req)
        .timeout(Duration::from_secs(120))   // build_pczt does halo2 proving — seconds of CPU
        .send().await
        .map_err(|e| format!("escrow POST: {}", e))?;
    let v: serde_json::Value = resp.json().await
        .map_err(|e| format!("escrow response not JSON: {}", e))?;
    if let Some(err) = v.get("error") {
        return Err(format!("escrow error: {}", err));
    }
    serde_json::from_value(v).map_err(|e| format!("InitiatePayoutResp shape: {}", e))
}

/// Body for driving the on-chain payout of a room whose co-signed outcome has ALREADY been
/// recorded via `/settle`. Sent as `POST /room/{code}/payout/initiate` with EMPTY `outputs`:
/// carries NO amounts and NO per-seat winner split. The escrow is the single split authority —
/// with a recorded `payout_plan` it spends that plan and ignores the (empty) `outputs`, so the
/// poker-server only forwards the capability token to authorize the payout.
#[derive(Debug, Clone, Serialize)]
pub struct InitiateSettledPayoutReq {
    /// ALWAYS empty. The escrow's `/payout/initiate` requires an `outputs` field, but when a
    /// co-signed `payout_plan` exists it IGNORES these and spends the recorded plan. We send `[]`
    /// so the escrow deserializes the body and takes the settled branch — the server never asserts
    /// a split of its own on a settled game.
    pub outputs: Vec<PayoutOutputReq>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fee_zat: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub anchor_height: Option<u32>,
    /// Per-room capability token minted by the escrow at room creation. Fail-closed gate.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub payout_token: Option<String>,
}

/// Ask the escrow to build + drive the on-chain payout from the outcome it already verified at
/// `/settle`. The escrow is the single split authority: it spends the co-signed `payout_plan` it
/// recorded at `/settle` and IGNORES any caller-supplied outputs, so we send an EMPTY `outputs`
/// list and no amounts.
///
/// CONTRACT: this drives the SETTLED case through the escrow's existing `POST /payout/initiate`
/// endpoint (the escrow branches on `payout_plan.is_some()` → spends the recorded plan, discarding
/// `req.outputs`). There is no separate `/payout/settled` route on the escrow; routing the settled
/// case here is exactly what makes the winner payout fire. Returns `{relay_room}` synchronously.
pub async fn initiate_settled_payout(base_url: &str, code: &str, req: &InitiateSettledPayoutReq)
    -> Result<InitiatePayoutResp, String>
{
    let url = format!("{}/room/{}/payout/initiate", base_url.trim_end_matches('/'), code);
    let resp = reqwest::Client::new()
        .post(&url)
        .json(req)
        .timeout(Duration::from_secs(120))   // build_pczt does halo2 proving — seconds of CPU
        .send().await
        .map_err(|e| format!("escrow POST: {}", e))?;
    let v: serde_json::Value = resp.json().await
        .map_err(|e| format!("escrow response not JSON: {}", e))?;
    if let Some(err) = v.get("error") {
        return Err(format!("escrow error: {}", err));
    }
    serde_json::from_value(v).map_err(|e| format!("InitiatePayoutResp shape: {}", e))
}

/// Body for `POST /room/{code}/cancel` — self-service abandonment refund. There is NO winner and
/// NO server-chosen split: the escrow refunds EACH depositor their OWN confirmed deposit (minus
/// its share of the tx fee) to its on-chain-pinned payout address. The escrow computes the whole
/// plan; the poker-server supplies only the capability token and the reason for the journal.
///
/// This replaces the old, unsafe `settlement_plan()` → unsigned winner payout on `Leave`: an
/// abandonment can only ever refund-each-own here, or be resolved by a completed co-signed
/// `/settle` (winner-exit), or escalated to the operator's `/arbitrate`.
#[derive(Debug, Clone, Serialize)]
pub struct CancelRefundReq {
    /// Coarse reason recorded in the escrow journal ("leave", "abandon", "timeout").
    pub reason: String,
    /// Per-room capability token; fail-closed like `/payout/initiate`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub payout_token: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fee_zat: Option<u64>,
}

/// POST /room/{code}/cancel — trigger the escrow's refund-each-own-confirmed plan. Returns the
/// `{relay_room}` for co-signing, identical in shape to `/payout/initiate`, so the poker-server's
/// existing status-poll + signing-request broadcast path drives the refund unchanged.
///
/// NOTE (parallel-contract assumption): coded against the escrow engineer's intended self-service
/// `/cancel` refund-each-own endpoint. If the final endpoint differs (name/verb), only this URL
/// changes; the poker-server still computes nothing.
pub async fn cancel_refund(base_url: &str, code: &str, req: &CancelRefundReq)
    -> Result<InitiatePayoutResp, String>
{
    let url = format!("{}/room/{}/cancel", base_url.trim_end_matches('/'), code);
    let resp = reqwest::Client::new()
        .post(&url)
        .json(req)
        .timeout(Duration::from_secs(120))   // escrow may build_pczt (halo2 proving) for the refund
        .send().await
        .map_err(|e| format!("escrow POST: {}", e))?;
    let v: serde_json::Value = resp.json().await
        .map_err(|e| format!("escrow response not JSON: {}", e))?;
    if let Some(err) = v.get("error") {
        return Err(format!("escrow error: {}", err));
    }
    serde_json::from_value(v).map_err(|e| format!("InitiatePayoutResp shape: {}", e))
}

pub async fn get_payout_status(base_url: &str, code: &str) -> Result<PayoutStatus, String> {
    let url = format!("{}/room/{}/payout/status", base_url.trim_end_matches('/'), code);
    let resp = reqwest::Client::new()
        .get(&url)
        .timeout(Duration::from_secs(5))
        .send().await
        .map_err(|e| format!("escrow GET: {}", e))?;
    let v: serde_json::Value = resp.json().await
        .map_err(|e| format!("escrow response not JSON: {}", e))?;
    serde_json::from_value(v).map_err(|e| format!("PayoutStatus shape: {}", e))
}

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
    /// Per-room capability token; must be echoed back on `/payout/initiate`.
    #[serde(default)]
    pub payout_token: Option<String>,
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
    /// MEMPOOL-seen (0-conf) per-seat totals. UX signal only — never spendable. Absent on older
    /// escrow builds, so `#[serde(default)]` → 0.
    #[serde(default)]
    pub player_a_deposit_pending: u64,
    #[serde(default)]
    pub player_b_deposit_pending: u64,
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

/// Body for `POST /room/{code}/settle` — the co-signed final outcome. Field names/order
/// mirror poker-escrow's `SettleReq`; both `player_*_sig` are hex Ed25519 signatures over
/// the escrow `settlement_message(..)`, one per seat's on-chain-pinned identity key.
#[derive(Debug, Clone, Serialize)]
pub struct SettleReq {
    pub player_a_stack: u64,
    pub player_b_stack: u64,
    pub player_a_address: String,
    pub player_b_address: String,
    pub action_log_hash: String,
    pub player_a_sig: String,
    pub player_b_sig: String,
}

/// Outcome of a co-signed `/settle` submission the escrow ACCEPTED (both sigs valid).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SettleOutcome {
    /// Both deposits are confirmed on-chain — the payout plan is recorded; drive it now.
    Finalized,
    /// Co-sign accepted and QUEUED because a deposit is not yet confirmed. This is NOT a failure:
    /// the escrow's confirmed-deposit scanner builds the payout plan automatically once both
    /// deposits confirm. The caller must not fail the room and must not drive the payout yet.
    QueuedPendingConfirmation,
}

/// POST /room/{code}/settle — submit both seats' signatures over the agreed outcome. The
/// escrow verifies each sig against the seat's on-chain-pinned identity key, checks the
/// signed payout addresses match the pinned ones, and records the payout plan. Returns
/// `Finalized` when confirmed deposits let it settle immediately, or `QueuedPendingConfirmation`
/// when the co-sign is accepted but a deposit is still confirming (auto-completed by the scanner).
/// Only a real rejection (bad sig, address mismatch, transport error) is an `Err`.
pub async fn settle(base_url: &str, code: &str, req: &SettleReq) -> Result<SettleOutcome, String> {
    let url = format!("{}/room/{}/settle", base_url.trim_end_matches('/'), code);
    let resp = reqwest::Client::new()
        .post(&url)
        .json(req)
        .timeout(Duration::from_secs(10))
        .send().await
        .map_err(|e| format!("escrow settle POST: {}", e))?;
    let v: serde_json::Value = resp.json().await
        .map_err(|e| format!("escrow settle response not JSON: {}", e))?;
    if let Some(err) = v.get("error") {
        return Err(format!("escrow settle rejected: {}", err));
    }
    if v.get("settled").and_then(|s| s.as_bool()) == Some(true) {
        return Ok(SettleOutcome::Finalized);
    }
    // `settled:false` with this flag means "accepted, awaiting deposit confirmation" — a queued
    // success, not an error. (Previously the caller treated any non-true `settled` as a hard
    // failure, turning a temporary 0-conf timing gap into a terminal PayoutFailed → refund.)
    if v.get("settle_pending_confirmation").and_then(|s| s.as_bool()) == Some(true) {
        return Ok(SettleOutcome::QueuedPendingConfirmation);
    }
    Err(format!("escrow settle did not confirm: {}", v))
}

/// POST /room/{code}/fault — forward a client-detected escrow fault to the escrow's
/// durable journal. Fire-and-forget from the caller's perspective; the escrow always
/// 200s and just records it. `seat` is the reporting seat (0/1), `phase` is coarse
/// ("dkg"/"deposit"/"settle"/"payout"/"connect"), `detail` is free-text.
pub async fn report_fault(base_url: &str, code: &str, seat: u8, phase: &str, detail: &str)
    -> Result<(), String>
{
    let url = format!("{}/room/{}/fault", base_url.trim_end_matches('/'), code);
    let body = serde_json::json!({ "seat": seat, "phase": phase, "detail": detail });
    reqwest::Client::new()
        .post(&url)
        .json(&body)
        .timeout(Duration::from_secs(5))
        .send().await
        .map_err(|e| format!("escrow fault POST: {}", e))?;
    Ok(())
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

#[cfg(test)]
mod tests {
    use super::*;

    // Contract lock #1: a settled payout carries EMPTY outputs and no amounts. The escrow spends
    // the co-signed plan it recorded at /settle; if the server ever asserted a split here it would
    // reintroduce the second, differently-rounded winner split this crate deliberately removed.
    #[test]
    fn settled_payout_req_carries_empty_outputs() {
        let req = InitiateSettledPayoutReq {
            outputs: Vec::new(),
            fee_zat: Some(10_000),
            anchor_height: None,
            payout_token: Some("aa".repeat(32)),
        };
        let v = serde_json::to_value(&req).unwrap();
        // `outputs` MUST serialize (the escrow's /payout/initiate requires the field) and be empty.
        assert_eq!(v.get("outputs").and_then(|o| o.as_array()).map(|a| a.len()), Some(0),
            "settled payout must send an empty outputs array — never a server-chosen split");
    }

    // Contract lock #2: the /cancel response has NO relay_room (the escrow opens it in a background
    // task and surfaces it via /payout/status). The response MUST still deserialize — a required
    // relay_room field would error and strand the refund co-sign (the bug this fix closes).
    #[test]
    fn cancel_response_without_relay_room_deserializes() {
        let body = serde_json::json!({
            "ok": true,
            "refund_plan": {"room": "R", "outputs": []},
            "message": "cancelled — refunding each seat its own confirmed deposit",
        });
        let resp: InitiatePayoutResp = serde_json::from_value(body)
            .expect("cancel response (no relay_room) must deserialize, not error");
        assert!(resp.relay_room.is_none(), "cancel response carries no relay_room — resolve via /payout/status");
        assert!(resp.plan.is_empty());
    }

    // The synchronous /payout/initiate response DOES carry relay_room inline; it must parse as Some.
    #[test]
    fn initiate_response_with_relay_room_deserializes() {
        let body = serde_json::json!({ "relay_room": "abc123" });
        let resp: InitiatePayoutResp = serde_json::from_value(body).unwrap();
        assert_eq!(resp.relay_room.as_deref(), Some("abc123"));
    }
}
