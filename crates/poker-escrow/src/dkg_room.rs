//! DKG-mode room provisioning: connect to the FROST relay, create a room, then spawn a
//! background task that runs `frost_dkg::run_dkg` and writes the result back to the
//! `EscrowRoom` once peers have joined and DKG completes.

use std::time::Duration;

use crate::frost_dkg;
use crate::frost_relay::FrostRelayClient;
use crate::{EscrowRoom, Rooms};

/// Total expected participants in the FROST multisig (escrow + 2 players).
const DKG_TOTAL: u16 = 3;
/// Threshold — any 2 of 3 may sign.
const DKG_THRESHOLD: u16 = 2;
/// Generous per-room budget for "open + wait for peers + run DKG"; deliberately long because
/// the human players might take a while to sit down at the table.
const DKG_TIMEOUT: Duration = Duration::from_secs(600);

/// Result of synchronously provisioning a FROST relay room. The actual DKG runs in the
/// background — once it completes, the `escrow_ua` field on the EscrowRoom gets populated.
pub struct DkgProvision {
    pub frost_room_code: String,
}

/// Open a FROST relay room, kick off the DKG background task. Returns the relay room code
/// immediately so the HTTP response can include the coords clients need to join.
pub async fn provision(
    rooms: Rooms,
    room_code: String,
    relay_url: String,
    network: zcash_address::Network,
) -> Result<DkgProvision, String> {
    let nick = format!("escrow-{}", room_code);
    let mut client = FrostRelayClient::connect(&relay_url, nick).await
        .map_err(|e| format!("relay connect: {:?}", e))?;
    let frost_room_code = client.create_room().await
        .map_err(|e| format!("relay create_room: {:?}", e))?;

    tracing::info!(
        "dkg room provisioned: poker_room={} frost_room={}",
        room_code, frost_room_code
    );

    let bg_rooms = rooms.clone();
    let bg_code = room_code.clone();
    tokio::spawn(async move {
        let result = frost_dkg::run_dkg(
            &mut client, DKG_THRESHOLD, DKG_TOTAL,
            1, // we're the only one in the room when we just created it
            true, network, DKG_TIMEOUT,
        ).await;
        write_dkg_result(bg_rooms, &bg_code, result).await;
    });

    Ok(DkgProvision { frost_room_code })
}

async fn write_dkg_result(
    rooms: Rooms,
    code: &str,
    result: Result<frost_dkg::DkgOutput, frost_dkg::DkgError>,
) {
    let mut rooms = rooms.lock().await;
    let Some(room) = rooms.get_mut(code) else {
        tracing::warn!("dkg completed for unknown room {}", code);
        return;
    };
    match result {
        Ok(out) => {
            tracing::info!("dkg completed for {}: ua={}", code, &out.orchard_ua);
            room.escrow_ua = Some(out.orchard_ua);
            room.dkg_key_package_hex = Some(out.key_package_hex);
            room.dkg_public_key_package_hex = Some(out.public_key_package_hex);
        }
        Err(e) => {
            tracing::error!("dkg failed for {}: {}", code, e);
        }
    }
}

/// Build the EscrowRoom shell for a DKG-mode room. `escrow_ua` is None until the
/// background task fills it in. The legacy osst-derived fields are still populated
/// because the unmigrated /sign endpoints depend on them; that compatibility shim
/// goes away in Phase 2.4.
pub fn empty_room(
    code: String,
    required_deposit: u64,
    rake_bps: u16,
    frost_relay_url: String,
    frost_room_code: String,
    legacy_osst: LegacyOsstShim,
) -> EscrowRoom {
    EscrowRoom {
        code,
        escrow_ua: None,
        frost_relay_url: Some(frost_relay_url),
        frost_room_code: Some(frost_room_code),
        dkg_key_package_hex: None,
        dkg_public_key_package_hex: None,
        escrow_address: legacy_osst.escrow_address,
        group_pubkey: legacy_osst.group_pubkey,
        server_share: legacy_osst.server_share,
        player_a_share_hex: legacy_osst.player_a_share_hex,
        player_b_share_hex: legacy_osst.player_b_share_hex,
        player_a_deposit: 0,
        player_b_deposit: 0,
        required_deposit,
        rake_bps,
        rake_paid: false,
        game_active: false,
        player_a_address: None,
        player_b_address: None,
        final_stacks: None,
        payout_plan: None,
        pending_nonces: None,
        created_at: now_ms(),
    }
}

/// Bundle of osst-derived legacy fields still required by `EscrowRoom`. Goes away with Phase 2.4.
pub struct LegacyOsstShim {
    pub escrow_address: [u8; 32],
    pub group_pubkey: pasta_curves::pallas::Point,
    pub server_share: osst::SecretShare<pasta_curves::pallas::Scalar>,
    pub player_a_share_hex: String,
    pub player_b_share_hex: String,
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}
