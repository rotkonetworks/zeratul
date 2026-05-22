//! DKG-mode room provisioning: connect to the FROST relay, create a room, then spawn a
//! background task that runs `frost_dkg::run_dkg` and writes the result back to the
//! `EscrowRoom` once peers have joined and DKG completes.

use std::time::Duration;

use zecli::client::ZidecarClient;

use crate::frost_dkg;
use crate::frost_relay::FrostRelayClient;
use crate::scanner;
use crate::{EscrowRoom, Rooms};

/// Poll cadence for the deposit scanner. 20s gives near-immediate UX once a block lands
/// without hammering zidecar between Zcash's ~75s block times.
const DEPOSIT_POLL_INTERVAL: std::time::Duration = std::time::Duration::from_secs(20);

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
    zidecar_url: String,
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
    let bg_network = network;
    let bg_zidecar = zidecar_url;
    tokio::spawn(async move {
        let result = frost_dkg::run_dkg(
            &mut client, DKG_THRESHOLD, DKG_TOTAL,
            1, // we're the only one in the room when we just created it
            true, bg_network, DKG_TIMEOUT,
        ).await;
        write_dkg_result(bg_rooms, &bg_code, result, bg_network, bg_zidecar).await;
    });

    Ok(DkgProvision { frost_room_code })
}

async fn write_dkg_result(
    rooms: Rooms,
    code: &str,
    result: Result<frost_dkg::DkgOutput, frost_dkg::DkgError>,
    network: zcash_address::Network,
    zidecar_url: String,
) {
    let out = match result {
        Ok(o) => o,
        Err(e) => { tracing::error!("dkg failed for {}: {}", code, e); return; }
    };
    let (seat_uas, seat_bytes) = match derive_seat_addresses(&out, network) {
        Ok(v) => v,
        Err(e) => {
            tracing::error!("seat address derive for {} failed: {}", code, e);
            return;
        }
    };

    let fvk_hex = out.orchard_fvk_hex.clone();
    {
        let mut rooms_lock = rooms.lock().await;
        let Some(room) = rooms_lock.get_mut(code) else {
            tracing::warn!("dkg completed for unknown room {}", code);
            return;
        };
        tracing::info!(
            "dkg completed for {}: ua={} seat0={} seat1={}",
            code, &out.orchard_ua, &seat_uas[0], &seat_uas[1],
        );
        room.escrow_ua = Some(out.orchard_ua);
        room.dkg_key_package_hex = Some(out.key_package_hex);
        room.dkg_public_key_package_hex = Some(out.public_key_package_hex);
        room.dkg_orchard_fvk_hex = Some(out.orchard_fvk_hex);
        room.dkg_sk_hex = Some(out.sk_hex);
        room.dkg_ephemeral_seed_hex = Some(out.ephemeral_seed_hex);
        room.seat_addresses = vec![Some(seat_uas[0].clone()), Some(seat_uas[1].clone())];
        room.seat_addr_bytes = vec![Some(seat_bytes[0]), Some(seat_bytes[1])];
    }

    tokio::spawn(run_deposit_poll(
        rooms.clone(),
        code.to_string(),
        zidecar_url,
        fvk_hex,
        vec![Some(seat_bytes[0]), Some(seat_bytes[1])],
    ));
}

/// Per-room poll loop. Connects to zidecar, initializes scan cursor at current tip,
/// then every `DEPOSIT_POLL_INTERVAL` scans new blocks and adds matched notes' values
/// to `player_{a,b}_deposit`. Exits when the room disappears from `rooms`.
async fn run_deposit_poll(
    rooms: Rooms,
    code: String,
    zidecar_url: String,
    fvk_hex: String,
    seat_addr_bytes: Vec<Option<[u8; 43]>>,
) {
    let client = match ZidecarClient::connect(&zidecar_url).await {
        Ok(c) => c,
        Err(e) => { tracing::error!("deposit poll {}: zidecar connect: {}", code, e); return; }
    };
    let fvk = match scanner::parse_fvk(&fvk_hex) {
        Ok(f) => f,
        Err(e) => { tracing::error!("deposit poll {}: parse fvk: {}", code, e); return; }
    };

    if let Ok((tip, _)) = client.get_tip().await {
        if let Some(room) = rooms.lock().await.get_mut(&code) {
            room.last_scanned_height = tip;
        }
        tracing::info!("deposit poll {}: starting from tip={}", code, tip);
    }

    loop {
        tokio::time::sleep(DEPOSIT_POLL_INTERVAL).await;
        let last = match rooms.lock().await.get(&code) {
            Some(r) => r.last_scanned_height,
            None => { tracing::info!("deposit poll {}: room gone, exiting", code); return; }
        };
        match scanner::scan(&client, &fvk, last, &seat_addr_bytes).await {
            Ok((new_tip, notes)) => {
                let mut rooms_lock = rooms.lock().await;
                let Some(room) = rooms_lock.get_mut(&code) else { return; };
                room.last_scanned_height = new_tip;
                for n in notes {
                    let txid_short = hex::encode(&n.txid[..n.txid.len().min(8)]);
                    match n.seat {
                        0 => room.player_a_deposit = room.player_a_deposit.saturating_add(n.value_zat),
                        1 => room.player_b_deposit = room.player_b_deposit.saturating_add(n.value_zat),
                        _ => {}
                    }
                    if let (Some(addr), Some(slot)) = (
                        n.payout_address.as_ref(),
                        room.seat_payout_address.get_mut(n.seat as usize),
                    ) {
                        if slot.is_none() {
                            *slot = Some(addr.clone());
                            tracing::info!("deposit room={} seat={} payout_address={}", code, n.seat, addr);
                        }
                    } else if n.payout_address.is_none() {
                        tracing::warn!(
                            "deposit room={} seat={} val={} MISSING payout memo — game cannot start until top-up with `zk.poker/v1/payout:<addr>`",
                            code, n.seat, n.value_zat,
                        );
                    }
                    tracing::info!(
                        "deposit room={} seat={} val={} tx={} h={}",
                        code, n.seat, n.value_zat, txid_short, n.block_height,
                    );
                    if !room.notes.iter().any(|existing| existing.nullifier == n.nullifier) {
                        room.notes.push(n);
                    }
                }
            }
            Err(e) => tracing::warn!("deposit poll {}: scan: {}", code, e),
        }
    }
}

/// derive UAs + raw 43-byte addresses for seats 0/1 at diversifier_index 1/2
fn derive_seat_addresses(
    out: &frost_dkg::DkgOutput,
    network: zcash_address::Network,
) -> Result<([String; 2], [[u8; 43]; 2]), String> {
    let sk_bytes = decode_sk_hex(&out.sk_hex)?;
    let mut uas = [String::new(), String::new()];
    let mut bytes = [[0u8; 43]; 2];
    for (seat, idx) in [(0usize, 1u32), (1, 2)] {
        let raw = frost_spend::orchestrate::derive_address_from_sk(
            &out.public_key_package_hex, sk_bytes, idx,
        ).map_err(|e| format!("derive_address_from_sk idx {}: {:?}", idx, e))?;
        bytes[seat] = raw;
        uas[seat] = crate::orchard_ua::encode_unified(raw, network)?;
    }
    Ok((uas, bytes))
}

fn decode_sk_hex(s: &str) -> Result<[u8; 32], String> {
    let v = hex::decode(s.trim()).map_err(|e| format!("sk hex: {}", e))?;
    if v.len() != 32 { return Err(format!("sk wrong length: {}", v.len())); }
    let mut out = [0u8; 32];
    out.copy_from_slice(&v);
    Ok(out)
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
        dkg_orchard_fvk_hex: None,
        dkg_sk_hex: None,
        dkg_ephemeral_seed_hex: None,
        seat_addresses: vec![None, None],
        seat_addr_bytes: vec![None, None],
        seat_payout_address: vec![None, None],
        notes: Vec::new(),
        last_scanned_height: 0,
        payout_status: crate::PayoutStatus::None,
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
