//! DKG-mode room provisioning: connect to the FROST relay, create a room, then spawn a
//! background task that runs `frost_dkg::run_dkg` and writes the result back to the
//! `EscrowRoom` once peers have joined and DKG completes.

use std::time::Duration;

use zecli::client::ZidecarClient;

use crate::frost_dkg;
use crate::frost_relay::FrostRelayClient;
use crate::scanner;
use crate::{EscrowRoom, PendingDeposit, Rooms};

/// Poll cadence for the deposit scanner. 20s gives near-immediate UX once a block lands
/// without hammering zidecar between Zcash's ~75s block times.
const DEPOSIT_POLL_INTERVAL: std::time::Duration = std::time::Duration::from_secs(20);

/// Poll cadence for the mempool (0-conf) watcher. Faster than the confirmed poll so the deal gate
/// opens promptly once both deposits are broadcast, without waiting for a block.
const MEMPOOL_POLL_INTERVAL: std::time::Duration = std::time::Duration::from_secs(3);

/// How long a pending (mempool-seen) note is retained after it stops appearing in the mempool
/// before we evict it and reverse its pending credit. The grace window tolerates a single missed
/// poll (e.g. a transient zidecar hiccup) so a still-valid tx isn't flapped out prematurely.
const EVICTION_GRACE_MS: u64 = 30_000;

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
    network: zcash_protocol::consensus::NetworkType,
    zidecar_url: String,
    house_address: String,
    store: Option<crate::persist::Store>,
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
    let bg_house = house_address;
    let bg_store = store;
    tokio::spawn(async move {
        let result = frost_dkg::run_dkg(
            &mut client, DKG_THRESHOLD, DKG_TOTAL,
            1, // we're the only one in the room when we just created it
            true, bg_network, DKG_TIMEOUT,
        ).await;
        write_dkg_result(bg_rooms, &bg_code, result, bg_network, bg_zidecar, bg_house, bg_store).await;
    });

    Ok(DkgProvision { frost_room_code })
}

async fn write_dkg_result(
    rooms: Rooms,
    code: &str,
    result: Result<frost_dkg::DkgOutput, frost_dkg::DkgError>,
    network: zcash_protocol::consensus::NetworkType,
    zidecar_url: String,
    house_address: String,
    store: Option<crate::persist::Store>,
) {
    let out = match result {
        Ok(o) => o,
        // FIX 3: DKG errored (e.g. "frost capability denied"). Previously this just logged and
        // returned, leaving a permanent zombie room stuck "setting up forever" with no signal.
        // Journal a `dkg_failed` event and mark the room so `get_room` can surface it as
        // terminally failed instead of pending.
        Err(e) => {
            let reason = e.to_string();
            tracing::error!("dkg failed for {}: {}", code, reason);
            mark_dkg_failed(&rooms, code, reason, store.as_ref()).await;
            return;
        }
    };
    let (seat_uas, seat_bytes) = match derive_seat_addresses(&out, network) {
        Ok(v) => v,
        Err(e) => {
            // seat-address derivation is part of DKG completion — a failure here is equally
            // terminal for the room, so surface it the same way.
            tracing::error!("seat address derive for {} failed: {}", code, e);
            mark_dkg_failed(&rooms, code, format!("seat address derive: {}", e), store.as_ref()).await;
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
        crate::journal::record(code, "dkg_completed", serde_json::json!({
            "escrow_ua": &out.orchard_ua,
            "seat0_address": &seat_uas[0],
            "seat1_address": &seat_uas[1],
        }));
        room.escrow_ua = Some(out.orchard_ua);
        room.dkg_key_package_hex = Some(out.key_package_hex);
        room.dkg_public_key_package_hex = Some(out.public_key_package_hex);
        room.dkg_orchard_fvk_hex = Some(out.orchard_fvk_hex);
        room.dkg_sk_hex = Some(out.sk_hex);
        room.dkg_ephemeral_seed_hex = Some(out.ephemeral_seed_hex);
        room.seat_addresses = vec![Some(seat_uas[0].clone()), Some(seat_uas[1].clone())];
        room.seat_addr_bytes = vec![Some(seat_bytes[0]), Some(seat_bytes[1])];
        // CRITICAL: DKG key material just landed — persist immediately so a restart can
        // still co-sign this vault's payout.
        if let Some(s) = store.as_ref() {
            s.save_room(room);
        }
    }

    tokio::spawn(run_deposit_poll(
        rooms.clone(),
        code.to_string(),
        zidecar_url.clone(),
        fvk_hex.clone(),
        vec![Some(seat_bytes[0]), Some(seat_bytes[1])],
        house_address,
        store.clone(),
        false, // fresh room: anchor the scan cursor at the current tip
    ));
    // 0-conf watcher: lets the deal gate open on mempool-seen deposits (never touches money paths).
    tokio::spawn(run_mempool_watch(
        rooms.clone(),
        code.to_string(),
        zidecar_url,
        fvk_hex,
        vec![Some(seat_bytes[0]), Some(seat_bytes[1])],
        store,
    ));
}

/// FIX 3: mark a room's DKG as terminally failed. Journals a `dkg_failed` event, sets
/// `room.dkg_failed = Some(reason)` (surfaced by `get_room`), fires a dispute alert, and persists
/// so the terminal state survives a restart. The room is left in the map (not removed) so operators
/// and clients can see WHY it failed rather than it silently vanishing or appearing to hang.
async fn mark_dkg_failed(
    rooms: &Rooms,
    code: &str,
    reason: String,
    store: Option<&crate::persist::Store>,
) {
    crate::journal::record(code, "dkg_failed", serde_json::json!({ "reason": reason }));
    crate::notify::dispute_alert(
        "🚨 zk.poker DKG failed",
        &format!("room {} DKG failed: {} — room is terminally unusable, no funds should have been deposited", code, reason),
        "rotating_light",
        code,
    );
    let mut rooms_lock = rooms.lock().await;
    if let Some(room) = rooms_lock.get_mut(code) {
        room.dkg_failed = Some(reason);
        if let Some(s) = store {
            s.save_room(room);
        }
    } else {
        tracing::warn!("dkg failed for unknown room {}", code);
    }
}

/// Resume the deposit scanner for a room restored from disk after a restart. Unlike a fresh
/// room, a resumed room keeps its persisted `last_scanned_height` so deposits that landed
/// while the service was down are still detected — NOT re-anchored to the current tip.
///
/// Caller must ensure the room is DKG-complete (`dkg_orchard_fvk_hex` set, `seat_addr_bytes`
/// populated) and still needs deposits (payout not yet settled).
pub fn resume_deposit_poll(
    rooms: Rooms,
    code: String,
    zidecar_url: String,
    fvk_hex: String,
    seat_addr_bytes: Vec<Option<[u8; 43]>>,
    house_address: String,
    store: Option<crate::persist::Store>,
) {
    tokio::spawn(run_deposit_poll(
        rooms.clone(),
        code.clone(),
        zidecar_url.clone(),
        fvk_hex.clone(),
        seat_addr_bytes.clone(),
        house_address,
        store.clone(),
        true,
    ));
    // 0-conf watcher rebuilds the ephemeral pending ledger from the live mempool after restart.
    tokio::spawn(run_mempool_watch(rooms, code, zidecar_url, fvk_hex, seat_addr_bytes, store));
}

/// Per-room poll loop. Connects to zidecar, initializes scan cursor at current tip (fresh
/// rooms only), then every `DEPOSIT_POLL_INTERVAL` scans new blocks and adds matched notes'
/// values to `player_{a,b}_deposit`. Exits when the room disappears from `rooms`.
///
/// `resume`: when true this is a room restored from disk — keep the persisted
/// `last_scanned_height` rather than re-anchoring to the tip, so downtime deposits aren't
/// skipped. The one exception is a persisted cursor of 0 (the process crashed in the narrow
/// window between DKG completion and the first tip-init, before any deposit was possible —
/// the escrow UA had only just been derived): re-anchor to tip to avoid a full genesis rescan.
#[allow(clippy::too_many_arguments)]
async fn run_deposit_poll(
    rooms: Rooms,
    code: String,
    zidecar_url: String,
    fvk_hex: String,
    seat_addr_bytes: Vec<Option<[u8; 43]>>,
    house_address: String,
    store: Option<crate::persist::Store>,
    resume: bool,
) {
    let client = match ZidecarClient::connect(&zidecar_url).await {
        Ok(c) => c,
        Err(e) => { tracing::error!("deposit poll {}: zidecar connect: {}", code, e); return; }
    };
    let fvk = match scanner::parse_fvk(&fvk_hex) {
        Ok(f) => f,
        Err(e) => { tracing::error!("deposit poll {}: parse fvk: {}", code, e); return; }
    };

    // A resumed room with a real persisted cursor keeps it (don't skip downtime deposits).
    // A fresh room — or a resumed room that never established a baseline (cursor 0) — anchors
    // at the current tip.
    let anchor_at_tip = !resume
        || rooms.lock().await.get(&code).map(|r| r.last_scanned_height == 0).unwrap_or(true);
    if anchor_at_tip {
        if let Ok((tip, _)) = client.get_tip().await {
            if let Some(room) = rooms.lock().await.get_mut(&code) {
                room.last_scanned_height = tip;
            }
            tracing::info!("deposit poll {}: starting from tip={}", code, tip);
        }
    } else {
        let from = rooms.lock().await.get(&code).map(|r| r.last_scanned_height).unwrap_or(0);
        tracing::info!("deposit poll {}: resuming from persisted height={}", code, from);
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
                    // BUG 3 — dedup by nullifier so a deposit credits the counters exactly once,
                    // even if the same note is observed on a subsequent poll (e.g. after a reorg
                    // rescans an overlapping height range) or was already counted. The nullifier
                    // is globally unique per note, so it's the canonical dedup key.
                    // dedup key is the bare nullifier hex; the mempool watcher keys its
                    // `pending_deposits` / this room's `counted_deposits` on the SAME string, so a
                    // note observed first in the mempool and then in a block maps to one key.
                    let null_key = hex::encode(n.nullifier);
                    let dedup_key = format!("note:{}", null_key);
                    let newly_counted = room.counted_deposits.insert(dedup_key);
                    if newly_counted {
                        match n.seat {
                            0 => room.player_a_deposit = room.player_a_deposit.saturating_add(n.value_zat),
                            1 => room.player_b_deposit = room.player_b_deposit.saturating_add(n.value_zat),
                            _ => {}
                        }
                        // PROMOTION: this note just confirmed. If the mempool watcher was already
                        // counting it as pending, remove it and subtract its value from the seat's
                        // PENDING counter — so confirmed += value and pending -= value keeps the
                        // deal-gate total (confirmed + pending) stable across the transition.
                        if let Some(pd) = room.pending_deposits.remove(&null_key) {
                            match pd.seat {
                                0 => room.player_a_deposit_pending = room.player_a_deposit_pending.saturating_sub(pd.value_zat),
                                1 => room.player_b_deposit_pending = room.player_b_deposit_pending.saturating_sub(pd.value_zat),
                                _ => {}
                            }
                        }
                        crate::journal::record(&code, "deposit_detected", serde_json::json!({
                            "seat": n.seat,
                            "value_zat": n.value_zat,
                            "txid": hex::encode(&n.txid),
                            "block_height": n.block_height,
                        }));
                    } else {
                        tracing::debug!("deposit room={} seat={} nullifier already counted — skipping credit", code, n.seat);
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
                    // pin the depositor's settlement identity key from the on-chain memo.
                    // first pin wins (a later top-up can't rebind the seat to a different key).
                    if let (Some(pk), Some(slot)) = (
                        n.identity_pubkey,
                        room.seat_identity_pubkey.get_mut(n.seat as usize),
                    ) {
                        if slot.is_none() {
                            *slot = Some(pk);
                            tracing::info!("deposit room={} seat={} identity pinned={}", code, n.seat, hex::encode(pk));
                        }
                    }
                    tracing::info!(
                        "deposit room={} seat={} val={} tx={} h={}",
                        code, n.seat, n.value_zat, txid_short, n.block_height,
                    );
                    if !room.notes.iter().any(|existing| existing.nullifier == n.nullifier) {
                        room.notes.push(n);
                    }
                }
                // persist after crediting deposits / pinning payout+identity / adding notes /
                // advancing the scan cursor, so a restart resumes with the same spendable notes.
                if let Some(s) = store.as_ref() {
                    s.save_room(room);
                }
                // FIX 1: a co-signed settlement may have been QUEUED while a deposit was
                // unconfirmed. Now that we just credited confirmed notes, try to complete it — it
                // executes (builds the CONFIRMED payout plan) iff both deposits are confirmed and
                // no eviction-shortfall taints the room. Fail-closed inside the helper.
                crate::try_complete_pending_settlement(room, &house_address, store.as_ref(), &code);
            }
            Err(e) => tracing::warn!("deposit poll {}: scan: {}", code, e),
        }
    }
}

/// Per-room MEMPOOL (0-conf) watcher. Polls zidecar's mempool stream every
/// `MEMPOOL_POLL_INTERVAL` and maintains the room's EPHEMERAL pending ledger
/// (`player_{a,b}_deposit_pending` + `pending_deposits`). This ledger feeds the DEAL gate only —
/// it lets the game start before a block confirms — and NEVER affects settlement / rake / payout,
/// which read the confirmed counters. A mempool note is NEVER pushed to `room.notes` (it has no
/// commitment-tree position, so it isn't spendable).
///
/// Each tick:
///   - scan the mempool; build this tick's set of nullifier-hex keys.
///   - CREDIT: for each note whose key is neither already confirmed (`counted_deposits`) nor
///     already pending (`pending_deposits`), insert a `PendingDeposit` and add its value to the
///     seat's pending counter.
///   - EVICT: for each existing pending entry whose key is NOT in this tick's mempool AND NOT in
///     `counted_deposits` AND older than `EVICTION_GRACE_MS`, remove it and subtract its value.
///
/// Exits when the room disappears from `rooms` (mirrors `run_deposit_poll`). Does NOT persist —
/// the pending ledger is ephemeral by design.
async fn run_mempool_watch(
    rooms: Rooms,
    code: String,
    zidecar_url: String,
    fvk_hex: String,
    seat_addr_bytes: Vec<Option<[u8; 43]>>,
    _store: Option<crate::persist::Store>,
) {
    let client = match ZidecarClient::connect(&zidecar_url).await {
        Ok(c) => c,
        Err(e) => { tracing::error!("mempool watch {}: zidecar connect: {}", code, e); return; }
    };
    let fvk = match scanner::parse_fvk(&fvk_hex) {
        Ok(f) => f,
        Err(e) => { tracing::error!("mempool watch {}: parse fvk: {}", code, e); return; }
    };

    loop {
        tokio::time::sleep(MEMPOOL_POLL_INTERVAL).await;
        // exit if the room is gone (mirror run_deposit_poll)
        if rooms.lock().await.get(&code).is_none() {
            tracing::info!("mempool watch {}: room gone, exiting", code);
            return;
        }

        let notes = match scanner::scan_mempool(&client, &fvk, &seat_addr_bytes).await {
            Ok(n) => n,
            Err(e) => { tracing::warn!("mempool watch {}: scan_mempool: {}", code, e); continue; }
        };

        // nullifier-hex keys seen THIS tick — used to decide evictions.
        let seen_this_tick: std::collections::HashSet<String> =
            notes.iter().map(|n| hex::encode(n.nullifier)).collect();

        let mut rooms_lock = rooms.lock().await;
        let Some(room) = rooms_lock.get_mut(&code) else { return; };

        // ── CREDIT new mempool notes ──────────────────────────────────────────
        for n in &notes {
            let key = hex::encode(n.nullifier);
            // already confirmed (dedup key is `note:<hex>`): the confirmed path owns it — skip.
            if room.counted_deposits.contains(&format!("note:{}", key)) {
                continue;
            }
            // already pending: skip (don't double-credit on a repeat poll).
            if room.pending_deposits.contains_key(&key) {
                continue;
            }
            room.pending_deposits.insert(key, PendingDeposit {
                seat: n.seat,
                value_zat: n.value_zat,
                first_seen_ms: now_ms(),
            });
            match n.seat {
                0 => room.player_a_deposit_pending = room.player_a_deposit_pending.saturating_add(n.value_zat),
                1 => room.player_b_deposit_pending = room.player_b_deposit_pending.saturating_add(n.value_zat),
                _ => {}
            }
            // journal the 0-conf sighting — never log secret values (memo/key material).
            crate::journal::record(&code, "deposit_pending", serde_json::json!({
                "seat": n.seat,
                "value_zat": n.value_zat,
                "txid": hex::encode(&n.txid),
            }));
            tracing::info!("mempool watch {}: seat={} val={} pending (0-conf)", code, n.seat, n.value_zat);
        }

        // ── EVICT pending notes that left the mempool without confirming ──────
        let now = now_ms();
        let to_evict: Vec<String> = room
            .pending_deposits
            .iter()
            .filter(|(key, pd)| {
                !seen_this_tick.contains(*key)
                    && !room.counted_deposits.contains(&format!("note:{}", key))
                    && now.saturating_sub(pd.first_seen_ms) > EVICTION_GRACE_MS
            })
            .map(|(key, _)| key.clone())
            .collect();
        let mut shortfall_marked = false;
        for key in to_evict {
            if let Some(pd) = room.pending_deposits.remove(&key) {
                match pd.seat {
                    0 => room.player_a_deposit_pending = room.player_a_deposit_pending.saturating_sub(pd.value_zat),
                    1 => room.player_b_deposit_pending = room.player_b_deposit_pending.saturating_sub(pd.value_zat),
                    _ => {}
                }
                // FIX 2: reconcile a live/queued hand against this eviction. If the evicted note's
                // seat is still short on CONFIRMED value, the buy-in it represented never landed —
                // the pot is NOT fully backed by both seats' own confirmed money. Mark the room so
                // the queued/attempted SETTLED-plan path cannot auto-pay a full pot (which would be
                // theft from the honest seat). The initiate_payout guard already blocks the
                // UNSETTLED path; this closes the SETTLED-plan path's completion hook.
                let seat_confirmed = match pd.seat {
                    0 => room.player_a_deposit,
                    1 => room.player_b_deposit,
                    _ => room.required_deposit, // unknown seat: treat as covered (no spurious taint)
                };
                if seat_confirmed < room.required_deposit && !room.evicted_shortfall {
                    room.evicted_shortfall = true;
                    shortfall_marked = true;
                    crate::journal::record(&code, "evicted_shortfall", serde_json::json!({
                        "seat": pd.seat,
                        "value_zat": pd.value_zat,
                        "seat_confirmed": seat_confirmed,
                        "required_deposit": room.required_deposit,
                        "had_queued_settlement": room.settle_pending.is_some(),
                    }));
                    tracing::warn!(
                        "mempool watch {}: seat={} pending val={} EVICTED while confirmed={} < required={} — \
                         marking evicted_shortfall; settled payout auto-completion blocked, use refund/arbitrate",
                        code, pd.seat, pd.value_zat, seat_confirmed, room.required_deposit,
                    );
                    // Operator push: a mempool-seen buy-in vanished without confirming while its seat is
                    // short — the classic 0-conf-double-spend signature. Auto-payout is already blocked;
                    // the operator should know so they can refund/arbitrate rather than wait.
                    crate::notify::dispute_alert(
                        "🚨 zk.poker 0-conf shortfall",
                        &format!(
                            "room {} seat {}: mempool buy-in ({} zat) EVICTED, confirmed {} < required {} \
                             — possible 0-conf double-spend. Settled auto-payout blocked; refund/arbitrate.",
                            code, pd.seat, pd.value_zat, seat_confirmed, room.required_deposit,
                        ),
                        "shortfall",
                        &code,
                    );
                }
                crate::journal::record(&code, "deposit_evicted", serde_json::json!({
                    "seat": pd.seat,
                    "value_zat": pd.value_zat,
                }));
                tracing::info!("mempool watch {}: seat={} val={} evicted (left mempool unconfirmed)", code, pd.seat, pd.value_zat);
            }
        }
        // persist the eviction-shortfall taint (normally the mempool ledger is ephemeral, but this
        // flag is a durable money-safety guard that must survive a restart).
        if shortfall_marked {
            if let Some(s) = _store.as_ref() {
                s.save_room(room);
            }
        }
    }
}

/// derive UAs + raw 43-byte addresses for seats 0/1 at diversifier_index 1/2
fn derive_seat_addresses(
    out: &frost_dkg::DkgOutput,
    network: zcash_protocol::consensus::NetworkType,
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
    payout_token: [u8; 32],
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
        seat_identity_pubkey: vec![None, None],
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
        payout_token,
        counted_deposits: std::collections::HashSet::new(),
        player_a_deposit_pending: 0,
        player_b_deposit_pending: 0,
        pending_deposits: std::collections::HashMap::new(),
        settle_pending: None,
        evicted_shortfall: false,
        dkg_failed: None,
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
