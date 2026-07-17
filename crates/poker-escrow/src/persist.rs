//! Durable persistence of per-room FROST key material + payout-relevant state.
//!
//! MONEY-CRITICAL. `poker-escrow` normally keeps every room's key material only in the
//! in-memory `Rooms` map. If the process restarts while a room holds deposited funds, the
//! key material evaporates and the house can NEVER co-sign that vault's payout → funds are
//! stranded on-chain forever. This module mirrors each room's secret + payout state to disk
//! so a restart can still co-sign.
//!
//! Layout: one JSON file per room at `{state_dir}/{code}.json`, written atomically (temp +
//! rename) with mode 0600. On startup `load_all` rehydrates every room. A room's file is
//! removed only once its payout has been successfully `Broadcast` (never while funds could
//! still be in the vault).
//!
//! Secrets on disk: this is plaintext JSON protected only by filesystem permissions (0600).
//! The directory MUST live on an encrypted / access-controlled volume. Encryption-at-rest is
//! a recommended follow-up (see module TODO); it was deferred to avoid adding an AEAD dep to
//! the money path in this first version.
//!
//! What we DON'T persist: `pending_nonces` (ephemeral FROST signing nonces). They are consumed
//! within a single signing round and are meaningless across a restart; they rehydrate as `None`.
//!
//! Never log actual secret values from here.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use pasta_curves::pallas::Scalar as PallasScalar;

use crate::{EscrowRoom, PayoutPlan, PayoutStatus, PendingSettlement};

// ---------------------------------------------------------------------------
// Serializable mirror of scanner::DepositNote
// ---------------------------------------------------------------------------
// `scanner::DepositNote` is `#[derive(Serialize, Deserialize)]` (added alongside this module),
// so we persist it directly rather than mirroring it here.

// ---------------------------------------------------------------------------
// PersistedRoom
// ---------------------------------------------------------------------------

/// On-disk form of an `EscrowRoom`. Captures everything needed to reconstruct a *payable*
/// room after a restart. Tricky curve types are stored as fixed-width hex/byte forms using
/// the same encodings the running service already uses on its wire paths:
///   - `server_share`: 36-byte hex (4-byte LE index ++ 32-byte scalar `to_repr`), matching
///     `main::share_to_bytes`.
///   - `group_pubkey`: 32-byte compressed point hex (`GroupEncoding::to_bytes`).
///   - `notes`: `scanner::DepositNote` serialized via serde (all plain byte/primitive fields).
#[derive(Serialize, Deserialize)]
pub struct PersistedRoom {
    pub code: String,

    // ── DKG-mode secret-bearing material (prod path: ESCROW_USE_DKG=true) ──
    pub escrow_ua: Option<String>,
    pub frost_relay_url: Option<String>,
    pub frost_room_code: Option<String>,
    pub dkg_key_package_hex: Option<String>,
    pub dkg_public_key_package_hex: Option<String>,
    pub dkg_orchard_fvk_hex: Option<String>,
    pub dkg_sk_hex: Option<String>,
    pub dkg_ephemeral_seed_hex: Option<String>,

    // ── per-seat metadata needed to process a payout ──
    pub seat_addresses: Vec<Option<String>>,
    /// per-seat 43-byte raw address, hex-encoded (serde can't derive on `[u8; 43]`).
    pub seat_addr_bytes_hex: Vec<Option<String>>,
    pub seat_payout_address: Vec<Option<String>>,
    pub seat_identity_pubkey: Vec<Option<[u8; 32]>>,

    // ── spendable notes + scan cursor ──
    pub notes: Vec<crate::scanner::DepositNote>,
    pub last_scanned_height: u32,

    // ── payout flow state ──
    pub payout_status: PayoutStatus,
    pub payout_plan: Option<PayoutPlan>,
    pub final_stacks: Option<(u64, u64)>,
    pub player_a_address: Option<String>,
    pub player_b_address: Option<String>,

    // ── deposit tracking ──
    pub player_a_deposit: u64,
    pub player_b_deposit: u64,
    pub required_deposit: u64,
    pub counted_deposits: std::collections::HashSet<String>,
    pub rake_bps: u16,
    pub rake_paid: bool,
    pub game_active: bool,

    // ── legacy / trusted-dealer (osst) material, needed by the unmigrated /sign endpoints ──
    /// 36-byte hex: 4-byte LE index ++ 32-byte scalar repr (see `main::share_to_bytes`).
    pub server_share_hex: String,
    /// 32-byte compressed pallas point hex.
    pub group_pubkey_hex: String,
    pub player_a_share_hex: String,
    pub player_b_share_hex: String,
    pub escrow_address: [u8; 32],

    // ── capability + bookkeeping ──
    pub payout_token: [u8; 32],
    pub created_at: u64,

    // ── FIX 1/2/3: queued-settlement + terminal-failure state (default for old files) ──
    /// A co-signed settlement queued while a deposit was unconfirmed (FIX 1). Completed by the
    /// confirmed scanner once both deposits land; abandoned if a deposit is evicted.
    #[serde(default)]
    pub settle_pending: Option<PendingSettlement>,
    /// A pending buy-in was evicted while its seat's confirmed deposit was still short (FIX 2).
    /// Blocks the queued settled-plan path from auto-paying a full pot that isn't fully confirmed.
    #[serde(default)]
    pub evicted_shortfall: bool,
    /// DKG errored — room is terminally unable to co-sign (FIX 3).
    #[serde(default)]
    pub dkg_failed: Option<String>,
}

// ---------------------------------------------------------------------------
// curve <-> bytes helpers (inverses of main::share_to_bytes / point_to_bytes)
// ---------------------------------------------------------------------------

/// Serialize an osst `SecretShare<PallasScalar>` to 36-byte hex — identical layout to
/// `main::share_to_bytes` (4-byte LE index ++ 32-byte scalar `to_repr`).
fn share_to_hex(share: &osst::SecretShare<PallasScalar>) -> String {
    use pasta_curves::group::ff::PrimeField;
    let mut buf = [0u8; 36];
    buf[0..4].copy_from_slice(&share.index.to_le_bytes());
    let scalar_bytes = share.scalar().to_repr();
    buf[4..36].copy_from_slice(scalar_bytes.as_ref());
    hex::encode(buf)
}

/// Inverse of `share_to_hex` — reconstruct the `SecretShare`.
fn share_from_hex(s: &str) -> Result<osst::SecretShare<PallasScalar>, String> {
    use pasta_curves::group::ff::PrimeField;
    let bytes = hex::decode(s).map_err(|e| format!("share hex: {}", e))?;
    if bytes.len() != 36 {
        return Err(format!("share wrong length: {} (want 36)", bytes.len()));
    }
    let index = u32::from_le_bytes(bytes[0..4].try_into().unwrap());
    let mut repr = <PallasScalar as PrimeField>::Repr::default();
    AsMut::<[u8]>::as_mut(&mut repr).copy_from_slice(&bytes[4..36]);
    let scalar = Option::<PallasScalar>::from(PallasScalar::from_repr(repr))
        .ok_or_else(|| "share scalar not canonical".to_string())?;
    if index == 0 {
        return Err("share index must be 1-indexed".to_string());
    }
    Ok(osst::SecretShare::new(index, scalar))
}

/// Serialize a pallas point to 32-byte compressed hex — identical to `main::point_to_bytes`.
fn point_to_hex(point: &pasta_curves::pallas::Point) -> String {
    use pasta_curves::group::GroupEncoding;
    hex::encode(point.to_bytes())
}

/// Inverse of `point_to_hex`.
fn point_from_hex(s: &str) -> Result<pasta_curves::pallas::Point, String> {
    use pasta_curves::group::GroupEncoding;
    let bytes = hex::decode(s).map_err(|e| format!("point hex: {}", e))?;
    if bytes.len() != 32 {
        return Err(format!("point wrong length: {} (want 32)", bytes.len()));
    }
    let mut arr = [0u8; 32];
    arr.copy_from_slice(&bytes);
    Option::<pasta_curves::pallas::Point>::from(pasta_curves::pallas::Point::from_bytes(&arr))
        .ok_or_else(|| "point not on curve".to_string())
}

// ---------------------------------------------------------------------------
// EscrowRoom <-> PersistedRoom
// ---------------------------------------------------------------------------

impl PersistedRoom {
    /// Snapshot an in-memory room for durable storage. Never touches `pending_nonces`.
    pub fn from_room(r: &EscrowRoom) -> Self {
        PersistedRoom {
            code: r.code.clone(),
            escrow_ua: r.escrow_ua.clone(),
            frost_relay_url: r.frost_relay_url.clone(),
            frost_room_code: r.frost_room_code.clone(),
            dkg_key_package_hex: r.dkg_key_package_hex.clone(),
            dkg_public_key_package_hex: r.dkg_public_key_package_hex.clone(),
            dkg_orchard_fvk_hex: r.dkg_orchard_fvk_hex.clone(),
            dkg_sk_hex: r.dkg_sk_hex.clone(),
            dkg_ephemeral_seed_hex: r.dkg_ephemeral_seed_hex.clone(),
            seat_addresses: r.seat_addresses.clone(),
            seat_addr_bytes_hex: r.seat_addr_bytes.iter().map(|o| o.map(hex::encode)).collect(),
            seat_payout_address: r.seat_payout_address.clone(),
            seat_identity_pubkey: r.seat_identity_pubkey.clone(),
            notes: r.notes.clone(),
            last_scanned_height: r.last_scanned_height,
            payout_status: r.payout_status.clone(),
            payout_plan: r.payout_plan.clone(),
            final_stacks: r.final_stacks,
            player_a_address: r.player_a_address.clone(),
            player_b_address: r.player_b_address.clone(),
            player_a_deposit: r.player_a_deposit,
            player_b_deposit: r.player_b_deposit,
            required_deposit: r.required_deposit,
            counted_deposits: r.counted_deposits.clone(),
            rake_bps: r.rake_bps,
            rake_paid: r.rake_paid,
            game_active: r.game_active,
            server_share_hex: share_to_hex(&r.server_share),
            group_pubkey_hex: point_to_hex(&r.group_pubkey),
            player_a_share_hex: r.player_a_share_hex.clone(),
            player_b_share_hex: r.player_b_share_hex.clone(),
            escrow_address: r.escrow_address,
            payout_token: r.payout_token,
            created_at: r.created_at,
            settle_pending: r.settle_pending.clone(),
            evicted_shortfall: r.evicted_shortfall,
            dkg_failed: r.dkg_failed.clone(),
        }
    }

    /// Rebuild an in-memory room from its on-disk form. `pending_nonces` is always `None`
    /// (ephemeral, never persisted).
    pub fn into_room(self) -> Result<EscrowRoom, String> {
        let server_share = share_from_hex(&self.server_share_hex)?;
        let group_pubkey = point_from_hex(&self.group_pubkey_hex)?;
        let seat_addr_bytes: Vec<Option<[u8; 43]>> = self
            .seat_addr_bytes_hex
            .iter()
            .map(|o| match o {
                None => Ok(None),
                Some(h) => {
                    let b = hex::decode(h).map_err(|e| format!("seat_addr hex: {}", e))?;
                    let arr: [u8; 43] = b
                        .as_slice()
                        .try_into()
                        .map_err(|_| format!("seat_addr wrong length: {}", b.len()))?;
                    Ok(Some(arr))
                }
            })
            .collect::<Result<_, String>>()?;
        Ok(EscrowRoom {
            code: self.code,
            escrow_ua: self.escrow_ua,
            frost_relay_url: self.frost_relay_url,
            frost_room_code: self.frost_room_code,
            dkg_key_package_hex: self.dkg_key_package_hex,
            dkg_public_key_package_hex: self.dkg_public_key_package_hex,
            dkg_orchard_fvk_hex: self.dkg_orchard_fvk_hex,
            dkg_sk_hex: self.dkg_sk_hex,
            dkg_ephemeral_seed_hex: self.dkg_ephemeral_seed_hex,
            seat_addresses: self.seat_addresses,
            seat_addr_bytes,
            seat_payout_address: self.seat_payout_address,
            seat_identity_pubkey: self.seat_identity_pubkey,
            notes: self.notes,
            last_scanned_height: self.last_scanned_height,
            payout_status: self.payout_status,
            escrow_address: self.escrow_address,
            group_pubkey,
            server_share,
            player_a_share_hex: self.player_a_share_hex,
            player_b_share_hex: self.player_b_share_hex,
            player_a_deposit: self.player_a_deposit,
            player_b_deposit: self.player_b_deposit,
            required_deposit: self.required_deposit,
            rake_bps: self.rake_bps,
            rake_paid: self.rake_paid,
            game_active: self.game_active,
            player_a_address: self.player_a_address,
            player_b_address: self.player_b_address,
            final_stacks: self.final_stacks,
            payout_plan: self.payout_plan,
            pending_nonces: None,
            created_at: self.created_at,
            payout_token: self.payout_token,
            counted_deposits: self.counted_deposits,
            // Pending (mempool-seen) deposits are EPHEMERAL and deliberately NOT persisted: a note
            // evicted from the mempool during downtime would otherwise rehydrate as a phantom
            // pending credit. They rebuild from the live mempool within one poll after restart.
            player_a_deposit_pending: 0,
            player_b_deposit_pending: 0,
            pending_deposits: std::collections::HashMap::new(),
            // FIX 1/2/3: queued co-signed settlement + terminal-failure flags survive a restart so
            // the confirmed scanner can still complete a queued settlement and the guards persist.
            settle_pending: self.settle_pending,
            evicted_shortfall: self.evicted_shortfall,
            dkg_failed: self.dkg_failed,
        })
    }
}

// ---------------------------------------------------------------------------
// Disk IO
// ---------------------------------------------------------------------------

/// Directory whose files each hold one room's persisted state. Empty string = disabled.
#[derive(Clone)]
pub struct Store {
    dir: PathBuf,
}

impl Store {
    /// Open (creating if needed) a state directory. Returns `None` (disabled) if `dir` is
    /// empty. A failure to create the directory is logged loudly and returns `None` — this
    /// is money-critical, so the operator MUST notice, but we fail-soft rather than crash so
    /// the service still serves (albeit without restart-durability, as before this feature).
    pub fn open(dir: &str) -> Option<Store> {
        let dir = dir.trim();
        if dir.is_empty() {
            tracing::error!(
                "persist: ESCROW_STATE_DIR is empty — room key material will NOT survive a \
                 restart. Set a persistent state dir before handling real money."
            );
            return None;
        }
        let path = PathBuf::from(dir);
        if let Err(e) = std::fs::create_dir_all(&path) {
            tracing::error!(
                "persist: create_dir_all({}) failed: {} — key material will NOT survive a restart",
                path.display(),
                e
            );
            return None;
        }
        // best-effort tighten dir perms to 0700
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            if let Err(e) = std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o700)) {
                tracing::warn!("persist: chmod 0700 {} failed: {}", path.display(), e);
            }
        }
        tracing::info!("persist: state dir {}", path.display());
        Some(Store { dir: path })
    }

    fn room_path(&self, code: &str) -> PathBuf {
        self.dir.join(format!("{}.json", sanitize(code)))
    }

    /// Atomically write a room's state. Fail-soft: an IO error is logged loudly (this is
    /// money-critical) but never panics, so a disk hiccup can't crash the money path.
    pub fn save_room(&self, room: &EscrowRoom) {
        if let Err(e) = self.save_room_inner(room) {
            tracing::error!(
                "persist: FAILED to save room {} — key material may be lost on restart: {}",
                room.code,
                e
            );
        }
    }

    fn save_room_inner(&self, room: &EscrowRoom) -> Result<(), String> {
        let persisted = PersistedRoom::from_room(room);
        let json = serde_json::to_vec_pretty(&persisted).map_err(|e| format!("serialize: {}", e))?;
        let final_path = self.room_path(&room.code);
        let tmp_path = self.dir.join(format!("{}.json.tmp", sanitize(&room.code)));

        write_private(&tmp_path, &json).map_err(|e| format!("write tmp: {}", e))?;
        std::fs::rename(&tmp_path, &final_path).map_err(|e| {
            // clean up the temp file so a rename failure doesn't leave litter
            let _ = std::fs::remove_file(&tmp_path);
            format!("rename: {}", e)
        })?;
        Ok(())
    }

    /// Load every `*.json` room file in the dir. A file that fails to parse or convert is
    /// skipped and logged — one corrupt file must not block recovery of the rest.
    pub fn load_all(&self) -> Vec<EscrowRoom> {
        let mut out = Vec::new();
        let entries = match std::fs::read_dir(&self.dir) {
            Ok(e) => e,
            Err(e) => {
                tracing::error!("persist: read_dir({}) failed: {}", self.dir.display(), e);
                return out;
            }
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().and_then(|s| s.to_str()) != Some("json") {
                continue; // skip .tmp and anything else
            }
            match self.load_one(&path) {
                Ok(room) => {
                    tracing::info!("persist: restored room {} from {}", room.code, path.display());
                    out.push(room);
                }
                Err(e) => {
                    tracing::error!(
                        "persist: SKIPPING unreadable room file {} — funds for that room may be \
                         unrecoverable, manual review required: {}",
                        path.display(),
                        e
                    );
                }
            }
        }
        out
    }

    fn load_one(&self, path: &Path) -> Result<EscrowRoom, String> {
        let bytes = std::fs::read(path).map_err(|e| format!("read: {}", e))?;
        let persisted: PersistedRoom =
            serde_json::from_slice(&bytes).map_err(|e| format!("deserialize: {}", e))?;
        persisted.into_room()
    }

    /// Remove a room's persisted file. Conservative: only removes when the payout is
    /// `Broadcast` (funds have left the vault). Any other status is a no-op — we never delete
    /// while funds could still be in the vault.
    pub fn remove_room(&self, room: &EscrowRoom) {
        if !matches!(room.payout_status, PayoutStatus::Broadcast { .. }) {
            tracing::warn!(
                "persist: refusing to remove room {} — payout_status is not Broadcast",
                room.code
            );
            return;
        }
        let path = self.room_path(&room.code);
        match std::fs::remove_file(&path) {
            Ok(()) => tracing::info!("persist: removed settled room {} ({})", room.code, path.display()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => tracing::warn!("persist: remove {} failed: {}", path.display(), e),
        }
    }
}

/// Write `bytes` to `path`, creating with mode 0600 (owner read/write only).
fn write_private(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    use std::io::Write;
    let mut opts = std::fs::OpenOptions::new();
    opts.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        opts.mode(0o600);
    }
    let mut f = opts.open(path)?;
    f.write_all(bytes)?;
    f.sync_all()?;
    Ok(())
}

/// Keep room codes from escaping the state dir via `/` or `..`. Room codes are short
/// alnum-ish tokens; anything outside `[A-Za-z0-9._-]` becomes `_`.
fn sanitize(code: &str) -> String {
    code.chars()
        .map(|c| if c.is_ascii_alphanumeric() || c == '.' || c == '_' || c == '-' { c } else { '_' })
        .collect()
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scanner::DepositNote;

    /// Build a representative DKG-mode room with sample secret material populated, plus the
    /// legacy osst fields (via a real trusted-dealer keygen so the curve types are valid).
    fn sample_room() -> EscrowRoom {
        // real osst material so server_share / group_pubkey are genuine curve elements
        let mut rng = rand::thread_rng();
        let (_a, _b, jury, group_pubkey) =
            osst::redpallas::zcash::setup_escrow(1, 1, &mut rng).expect("setup_escrow");
        let escrow_address = osst::redpallas::zcash::derive_address_bytes(&group_pubkey);
        let server_share = jury.node_shares.into_iter().next().unwrap();

        EscrowRoom {
            code: "TESTROOM".to_string(),
            escrow_ua: Some("u1testescrowua".to_string()),
            frost_relay_url: Some("ws://relay.example/ws".to_string()),
            frost_room_code: Some("FROSTCODE".to_string()),
            dkg_key_package_hex: Some("aa".repeat(64)),
            dkg_public_key_package_hex: Some("bb".repeat(48)),
            dkg_orchard_fvk_hex: Some("cc".repeat(96)),
            dkg_sk_hex: Some("dd".repeat(32)),
            dkg_ephemeral_seed_hex: Some("ee".repeat(32)),
            seat_addresses: vec![Some("u1seat0".to_string()), Some("u1seat1".to_string())],
            seat_addr_bytes: vec![Some([0x11u8; 43]), Some([0x22u8; 43])],
            seat_payout_address: vec![Some("u1payout0".to_string()), None],
            seat_identity_pubkey: vec![Some([0x33u8; 32]), None],
            notes: vec![DepositNote {
                seat: 1,
                value_zat: 123_456,
                txid: vec![0xde, 0xad, 0xbe, 0xef],
                block_height: 42,
                payout_address: Some("u1payoutnote".to_string()),
                identity_pubkey: Some([0x44u8; 32]),
                nullifier: [0x55u8; 32],
                cmx: [0x66u8; 32],
                recipient: [0x77u8; 43],
                rho: [0x88u8; 32],
                rseed: [0x99u8; 32],
                position: 7,
            }],
            last_scanned_height: 100,
            payout_status: PayoutStatus::Pending { relay_room: "RELAY1".to_string() },
            escrow_address,
            group_pubkey,
            server_share,
            player_a_share_hex: "0a".repeat(36),
            player_b_share_hex: "0b".repeat(36),
            player_a_deposit: 210_000,
            player_b_deposit: 205_000,
            required_deposit: 200_000,
            rake_bps: 150,
            rake_paid: false,
            game_active: true,
            player_a_address: Some("u1a".to_string()),
            player_b_address: Some("u1b".to_string()),
            final_stacks: Some((300_000, 110_000)),
            payout_plan: None,
            pending_nonces: None,
            created_at: 1_700_000_000_000,
            payout_token: [0xABu8; 32],
            counted_deposits: {
                let mut s = std::collections::HashSet::new();
                s.insert("note:aabb".to_string());
                s.insert("http:txid:0".to_string());
                s
            },
            player_a_deposit_pending: 0,
            player_b_deposit_pending: 0,
            pending_deposits: std::collections::HashMap::new(),
            settle_pending: None,
            evicted_shortfall: false,
            dkg_failed: None,
        }
    }

    #[test]
    fn roundtrip_preserves_secret_bearing_fields() {
        let room = sample_room();

        // snapshot the secret-bearing + payout-relevant fields BEFORE conversion (room is moved)
        let orig_kp = room.dkg_key_package_hex.clone();
        let orig_seed = room.dkg_ephemeral_seed_hex.clone();
        let orig_sk = room.dkg_sk_hex.clone();
        let orig_fvk = room.dkg_orchard_fvk_hex.clone();
        let orig_ua = room.escrow_ua.clone();
        let orig_token = room.payout_token;
        let orig_a_dep = room.player_a_deposit;
        let orig_b_dep = room.player_b_deposit;
        let orig_seat_bytes = room.seat_addr_bytes.clone();
        let orig_seat_id = room.seat_identity_pubkey.clone();
        let orig_seat_payout = room.seat_payout_address.clone();
        let orig_notes_null = room.notes[0].nullifier;
        let orig_notes_val = room.notes[0].value_zat;
        let orig_counted = room.counted_deposits.clone();
        let orig_share_hex = share_to_hex(&room.server_share);
        let orig_point_hex = point_to_hex(&room.group_pubkey);
        let orig_escrow_addr = room.escrow_address;

        // EscrowRoom -> PersistedRoom -> JSON -> PersistedRoom -> EscrowRoom
        let persisted = PersistedRoom::from_room(&room);
        let json = serde_json::to_vec(&persisted).expect("serialize");
        let decoded: PersistedRoom = serde_json::from_slice(&json).expect("deserialize");
        let restored = decoded.into_room().expect("into_room");

        // secret-bearing DKG fields byte-for-byte
        assert_eq!(restored.dkg_key_package_hex, orig_kp);
        assert_eq!(restored.dkg_ephemeral_seed_hex, orig_seed);
        assert_eq!(restored.dkg_sk_hex, orig_sk);
        assert_eq!(restored.dkg_orchard_fvk_hex, orig_fvk);
        assert_eq!(restored.escrow_ua, orig_ua);

        // capability token + deposits
        assert_eq!(restored.payout_token, orig_token);
        assert_eq!(restored.player_a_deposit, orig_a_dep);
        assert_eq!(restored.player_b_deposit, orig_b_dep);
        assert_eq!(restored.counted_deposits, orig_counted);

        // seat data
        assert_eq!(restored.seat_addr_bytes, orig_seat_bytes);
        assert_eq!(restored.seat_identity_pubkey, orig_seat_id);
        assert_eq!(restored.seat_payout_address, orig_seat_payout);

        // notes survive
        assert_eq!(restored.notes.len(), 1);
        assert_eq!(restored.notes[0].nullifier, orig_notes_null);
        assert_eq!(restored.notes[0].value_zat, orig_notes_val);

        // pending_nonces must NOT survive (ephemeral)
        assert!(restored.pending_nonces.is_none());

        // legacy osst curve types survive byte-for-byte through their hex encodings
        assert_eq!(share_to_hex(&restored.server_share), orig_share_hex);
        assert_eq!(point_to_hex(&restored.group_pubkey), orig_point_hex);
        assert_eq!(restored.escrow_address, orig_escrow_addr);
    }

    #[test]
    fn save_load_remove_via_store() {
        let tmp = std::env::temp_dir().join(format!("escrow-persist-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        let store = Store::open(tmp.to_str().unwrap()).expect("open store");

        let room = sample_room();
        store.save_room(&room);

        let loaded = store.load_all();
        assert_eq!(loaded.len(), 1);
        assert_eq!(loaded[0].code, "TESTROOM");
        assert_eq!(loaded[0].dkg_key_package_hex, room.dkg_key_package_hex);

        // remove is a no-op while not Broadcast
        store.remove_room(&room);
        assert_eq!(store.load_all().len(), 1, "must not delete a non-Broadcast room");

        // now mark Broadcast and remove
        let mut paid = sample_room();
        paid.payout_status = PayoutStatus::Broadcast {
            txid: "deadbeef".to_string(),
            relay_room: "RELAY1".to_string(),
        };
        store.remove_room(&paid);
        assert_eq!(store.load_all().len(), 0, "Broadcast room file should be gone");

        let _ = std::fs::remove_dir_all(&tmp);
    }

    // ─────────────────────────────────────────────────────────────────────────
    // Tier-1: restored DKG material is USABLE FOR SIGNING after a disk restart.
    //
    // The two tests above only prove the DKG hex fields survive byte-for-byte.
    // That does NOT prove the restored bytes still deserialize into valid FROST
    // types and still produce a VALID SpendAuth signature under the group key —
    // the actual money-critical property. The prod payout path
    // (`main::run_payout_signing` → `payout_signing::host_sign_pczt`) feeds the
    // room's `dkg_key_package_hex` / `dkg_ephemeral_seed_hex` /
    // `dkg_public_key_package_hex` into `frost_spend::orchestrate::{sign_round1,
    // spend_sign_round2_signed, spend_aggregate}` over a relay. Here we exercise
    // those exact functions offline (the relay is only a transport) using the
    // *restored* host material, and prove the co-signature verifies.
    // ─────────────────────────────────────────────────────────────────────────

    /// Mint a real 2-of-3 FROST key set OFFLINE via the in-process DKG (no relay),
    /// mirroring `payout_signing::tests::local_2of3_dkg`. Returns the shared
    /// public-key-package hex plus each party's (key_package_hex, ephemeral_seed_hex).
    fn offline_2of3_dkg() -> (String, [(String, String); 3]) {
        use frost_spend::orchestrate::{dkg_part1, dkg_part2, dkg_part3};
        let r1_a = dkg_part1(3, 2).unwrap();
        let r1_b = dkg_part1(3, 2).unwrap();
        let r1_c = dkg_part1(3, 2).unwrap();
        let bc_for_a = vec![r1_b.broadcast_hex.clone(), r1_c.broadcast_hex.clone()];
        let bc_for_b = vec![r1_a.broadcast_hex.clone(), r1_c.broadcast_hex.clone()];
        let bc_for_c = vec![r1_a.broadcast_hex.clone(), r1_b.broadcast_hex.clone()];
        let r2_a = dkg_part2(&r1_a.secret_hex, &bc_for_a).unwrap();
        let r2_b = dkg_part2(&r1_b.secret_hex, &bc_for_b).unwrap();
        let r2_c = dkg_part2(&r1_c.secret_hex, &bc_for_c).unwrap();
        let all_r2: Vec<String> = r2_a
            .peer_packages
            .iter()
            .chain(r2_b.peer_packages.iter())
            .chain(r2_c.peer_packages.iter())
            .cloned()
            .collect();
        let r3_a = dkg_part3(&r2_a.secret_hex, &bc_for_a, &all_r2).unwrap();
        let r3_b = dkg_part3(&r2_b.secret_hex, &bc_for_b, &all_r2).unwrap();
        let r3_c = dkg_part3(&r2_c.secret_hex, &bc_for_c, &all_r2).unwrap();
        (
            r3_a.public_key_package_hex.clone(),
            [
                (r3_a.key_package_hex, r3_a.ephemeral_seed_hex),
                (r3_b.key_package_hex, r3_b.ephemeral_seed_hex),
                (r3_c.key_package_hex, r3_c.ephemeral_seed_hex),
            ],
        )
    }

    /// Sample 32 bytes that decode to a canonical Pallas scalar (valid sighash/alpha).
    fn valid_scalar_bytes() -> [u8; 32] {
        use pasta_curves::group::ff::PrimeField;
        use rand::RngCore;
        let mut rng = rand::thread_rng();
        loop {
            let mut bytes = [0u8; 32];
            rng.fill_bytes(&mut bytes);
            if bool::from(pasta_curves::pallas::Scalar::from_repr(bytes).is_some()) {
                return bytes;
            }
        }
    }

    /// Drive a full offline 2-of-3 FROST SpendAuth co-sign between two parties using
    /// the exact `frost_spend::orchestrate` calls the prod signing path uses. Returns
    /// the two independently-aggregated 64-byte signatures (hex). A returned pair means
    /// `spend_aggregate` — which internally verifies the aggregate against the group's
    /// (randomized) verifying key and returns `Err` on any failure — succeeded for both
    /// parties, i.e. the produced signature cryptographically verifies under the group key.
    fn offline_cosign(
        pkg_hex: &str,
        host_kp: &str,
        host_seed_hex: &str,
        peer_kp: &str,
        peer_seed_hex: &str,
        sighash: &[u8; 32],
        alpha: &[u8; 32],
    ) -> (String, String) {
        use frost_spend::orchestrate as fs;

        let host_seed = decode_seed_32(host_seed_hex);
        let peer_seed = decode_seed_32(peer_seed_hex);

        // round 1: each party commits
        let (host_nonces, host_commit) = fs::sign_round1(&host_seed, host_kp).expect("host round1");
        let (peer_nonces, peer_commit) = fs::sign_round1(&peer_seed, peer_kp).expect("peer round1");
        // both parties see the same commitment set (order-independent: aggregate maps by identity)
        let all_commits = vec![host_commit.clone(), peer_commit.clone()];

        // round 2: each party signs its share
        let host_share = fs::spend_sign_round2_signed(
            &host_seed, host_kp, &host_nonces, sighash, alpha, &all_commits,
        )
        .expect("host round2");
        let peer_share = fs::spend_sign_round2_signed(
            &peer_seed, peer_kp, &peer_nonces, sighash, alpha, &all_commits,
        )
        .expect("peer round2");
        let all_shares = vec![host_share, peer_share];

        // each party aggregates independently — success == the signature verifies
        // under the group verifying key (frost-rerandomized `aggregate` verifies).
        let host_sig = fs::spend_aggregate(pkg_hex, sighash, alpha, &all_commits, &all_shares)
            .expect("host aggregate (restored material must produce a VALID signature)");
        let peer_sig = fs::spend_aggregate(pkg_hex, sighash, alpha, &all_commits, &all_shares)
            .expect("peer aggregate");
        (host_sig, peer_sig)
    }

    fn decode_seed_32(h: &str) -> [u8; 32] {
        let v = hex::decode(h).expect("seed hex");
        let mut out = [0u8; 32];
        out.copy_from_slice(&v);
        out
    }

    /// TIER 1: end-to-end proof that DKG key material stored by `Store::save_room` and
    /// reloaded by `Store::load_all` still SIGNS. We mint a real 2-of-3 FROST key set
    /// offline, put the HOST's share + group public key package into an `EscrowRoom`,
    /// round-trip that room through the on-disk `Store` (save → load), then co-sign a
    /// sample sighash/alpha using the RESTORED host material + one original player share.
    /// A valid aggregated signature (verified inside `spend_aggregate`) proves the
    /// restored bytes are cryptographically live, not merely byte-equal.
    #[test]
    fn restored_dkg_material_still_signs() {
        // 1. real offline 2-of-3 key set
        let (pkg_hex, shares) = offline_2of3_dkg();
        let (host_kp, host_seed) = shares[0].clone();
        let (peer_kp, peer_seed) = shares[1].clone();

        // sanity: sign works with the ORIGINAL (pre-persistence) host material, so any
        // post-restore failure is attributable to persistence, not the key set itself.
        let sighash = valid_scalar_bytes();
        let alpha = valid_scalar_bytes();
        let (pre_a, pre_b) = offline_cosign(
            &pkg_hex, &host_kp, &host_seed, &peer_kp, &peer_seed, &sighash, &alpha,
        );
        assert_eq!(pre_a, pre_b, "pre-persistence: both parties agree on the sig");
        assert_eq!(pre_a.len(), 128, "SpendAuth sig is 64 bytes");

        // 2. embed the HOST's DKG material into an EscrowRoom and persist it to disk.
        let mut room = sample_room();
        room.dkg_key_package_hex = Some(host_kp.clone());
        room.dkg_public_key_package_hex = Some(pkg_hex.clone());
        room.dkg_ephemeral_seed_hex = Some(host_seed.clone());

        let tmp = std::env::temp_dir()
            .join(format!("escrow-persist-sign-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        let store = Store::open(tmp.to_str().unwrap()).expect("open store");
        store.save_room(&room);

        // 3. reload from disk exactly as a restart would (Store::load_all).
        let loaded = store.load_all();
        assert_eq!(loaded.len(), 1, "one room round-trips through disk");
        let restored = &loaded[0];
        let r_kp = restored
            .dkg_key_package_hex
            .clone()
            .expect("restored key package hex present");
        let r_pkg = restored
            .dkg_public_key_package_hex
            .clone()
            .expect("restored public key package hex present");
        let r_seed = restored
            .dkg_ephemeral_seed_hex
            .clone()
            .expect("restored ephemeral seed hex present");

        // 4. co-sign a FRESH message using the RESTORED host material + the (original,
        //    never-persisted) player share. This is the money-critical property: after a
        //    restart the house can still co-sign a payout.
        let sighash2 = valid_scalar_bytes();
        let alpha2 = valid_scalar_bytes();
        let (post_a, post_b) = offline_cosign(
            &r_pkg, &r_kp, &r_seed, &peer_kp, &peer_seed, &sighash2, &alpha2,
        );

        // aggregate() already verified the signature under the group key; assert the two
        // independent aggregations converge and the signature is well-formed.
        assert_eq!(
            post_a, post_b,
            "post-restore: host (restored) + player converge on one valid aggregated sig"
        );
        assert_eq!(post_a.len(), 128, "restored material yields a 64-byte SpendAuth sig");

        // The restored group public key package must be byte-identical to the original,
        // so the signature verifies under the SAME group verifying key funds are locked to.
        assert_eq!(r_pkg, pkg_hex, "restored group public key package unchanged");

        let _ = std::fs::remove_dir_all(&tmp);
    }
}
