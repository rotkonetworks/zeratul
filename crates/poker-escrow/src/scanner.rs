//! Orchard compact-block scanner. Adapted from `zcli/bin/license-server/src/scanner.rs`:
//! we pull compact blocks from zidecar, trial-decrypt each action with the multisig FVK,
//! and attribute every recovered note to a seat by matching its 43-byte address against
//! the per-seat deposit UAs derived at room-creation time.
//!
//! Compact decryption only — no memo, no full-tx fetch. Defense against malicious zidecar
//! via cmx verification (recompute commitment from the decrypted note, compare to the
//! cmx zidecar gave us).
//!
//! ## Ironwood dual-pool scan (step 3)
//!
//! Post-NU6.3 the escrow must scan BOTH shielded pools:
//!   - the legacy **Orchard** pool — in-flight / pre-activation deposits are still V2
//!     Orchard notes and must remain detectable + spendable;
//!   - the **Ironwood** pool — new deposits (depositors can no longer send cross-address
//!     Orchard notes to the escrow, same seal) AND the escrow's own V6-payout change land
//!     here as V3 Ironwood notes (see tx_build::build_pczt_v6_ironwood).
//!
//! Per the fork verdict (and the proven wasm scanner in
//! `zcli-ironwood/crates/zcash-wasm/src/lib.rs`), Ironwood REUSES Orchard's key tree and
//! note encryption: the SAME `OrchardDomain` trial-decrypts both pools — the note plaintext
//! lead byte selects the version (`note.version()` → V2 / V3) and nullifier derivation
//! follows the note's own version. There is no separate `IronwoodDomain`; the "pool" is a
//! property of WHICH bundle in the tx the action came from, applied by the caller. So the
//! escrow's own Orchard FVK/IVK derives ironwood-pool note detection unchanged.
//!
//! We tag every recovered note with its [`NotePool`] and thread that through `DepositNote`
//! → `tx_build::reconstruct_note`, which restores the exact `NoteVersion` so a scanned
//! Ironwood note reconstructs to the right commitment for spending.
//!
//! WIRE DEPENDENCY (see `ironwood_actions_of`): the ironwood action list is a SEPARATE
//! field on the compact block (proto `CompactTx.ironwoodActions`, zidecar commit 4edd4f2).
//! `zecli::client::CompactBlock` does not surface that field yet — until it does, the
//! ironwood arm scans an empty list, so the dual-scan is correct-but-inert on that pool
//! and lights up the moment zecli exposes ironwood actions. See the report / STOP note.

use std::io::Cursor;

use orchard::keys::{FullViewingKey, PreparedIncomingViewingKey, Scope};
use orchard::note::NoteVersion;
use orchard::note_encryption::OrchardDomain;
use zcash_note_encryption::{
    try_compact_note_decryption, try_note_decryption, EphemeralKeyBytes, ShieldedOutput,
    COMPACT_NOTE_SIZE, ENC_CIPHERTEXT_SIZE,
};
use zecli::client::{CompactBlock, ZidecarClient};

/// Which shielded pool a deposit note lives in. Orchard notes carry
/// [`NoteVersion::V2`]; Ironwood (NU6.3+) notes carry [`NoteVersion::V3`]. The pool is
/// fixed at scan time by WHICH bundle's action decrypted (the ciphertext itself does not
/// encode the pool), and it is what tells the payout builder which spend method
/// (`add_orchard_spend` vs `add_ironwood_spend`) and note version to reconstruct with.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum NotePool {
    /// Legacy Orchard pool — `NoteVersion::V2`.
    Orchard,
    /// Ironwood pool — `NoteVersion::V3`.
    Ironwood,
}

impl NotePool {
    /// The orchard note-plaintext version this pool's notes carry. Used to reconstruct the
    /// exact `Note` (and thus its commitment) at spend time.
    pub fn note_version(self) -> NoteVersion {
        match self {
            NotePool::Orchard => NoteVersion::V2,
            NotePool::Ironwood => NoteVersion::V3,
        }
    }
}

/// serde default for `DepositNote.pool` — notes persisted BEFORE step 3 had no pool field
/// and were, by construction, all legacy Orchard (V2). Defaulting to Orchard keeps those
/// on-disk notes readable and spendable exactly as before.
fn default_pool() -> NotePool {
    NotePool::Orchard
}

/// serde (de)serialization for `[u8; 43]` as a hex string — serde's derive only supports
/// fixed arrays up to length 32, so `DepositNote.recipient` needs this shim for persistence.
mod hex_array_43 {
    use serde::{Deserialize, Deserializer, Serializer};
    pub fn serialize<S: Serializer>(bytes: &[u8; 43], s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&hex::encode(bytes))
    }
    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<[u8; 43], D::Error> {
        let s = String::deserialize(d)?;
        let v = hex::decode(&s).map_err(serde::de::Error::custom)?;
        v.as_slice()
            .try_into()
            .map_err(|_| serde::de::Error::custom(format!("expected 43 bytes, got {}", v.len())))
    }
}

/// Memo prefix every poker-escrow deposit must carry; the suffix is the depositor's personal
/// Orchard receiver so we know where to send refunds + payouts later.
pub const PAYOUT_MEMO_PREFIX: &str = "zk.poker/v1/payout:";

/// Orchard NU5 activation on Zcash mainnet.
const ORCHARD_ACTIVATION_MAINNET: u32 = 1_687_104;
/// Compact-block stream batch size.
const BATCH_SIZE: u32 = 1_000;

/// Sentinel `position` for a note observed in the MEMPOOL (0-conf). An unmined note has no
/// commitment-tree leaf index yet, so it must never be used to build a payout witness. The
/// confirmed scanner assigns the real position once the note lands in a block.
pub const NO_POSITION: u64 = u64::MAX;

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct DepositNote {
    pub seat: u8,
    pub value_zat: u64,
    pub txid: Vec<u8>,
    pub block_height: u32,
    /// `Some(u1...)` when the deposit's memo started with `PAYOUT_MEMO_PREFIX` — that's where
    /// refunds / payouts go for this seat. `None` means the depositor forgot the memo and the
    /// game cannot start until a memo-bearing top-up arrives.
    pub payout_address: Option<String>,
    /// `Some(32-byte)` when the memo pinned a `;id:<hex>` Ed25519 identity pubkey. This is the
    /// key escrow requires a settlement signature from for this seat (set on-chain by the
    /// depositor, so the operator cannot forge it). `None` = no identity pinned.
    pub identity_pubkey: Option<[u8; 32]>,
    /// 32-byte note nullifier; used to mark the note spent when we sign a payout tx.
    pub nullifier: [u8; 32],
    /// 32-byte note commitment (`cmx`). zidecar's `GetCommitmentProofs` keys on this.
    pub cmx: [u8; 32],
    /// raw 43-byte recipient address (diversifier + pk_d). orchard `Note::from_parts` needs it.
    /// serde can't derive on `[u8; 43]`, so it's (de)serialized as hex.
    #[serde(with = "hex_array_43")]
    pub recipient: [u8; 43],
    /// `rho` is the action-binding randomness; needed to reconstruct the orchard `Note` at payout.
    pub rho: [u8; 32],
    /// `rseed` is the per-note random seed; together with `rho` it reconstructs the `Note`.
    pub rseed: [u8; 32],
    /// Leaf index of this note's `cmx` in the global Orchard commitment tree. Required by
    /// `zecli::witness::build_witnesses` to construct a merkle path at payout time.
    pub position: u64,
    /// Which shielded pool this note was scanned from. Determines the `NoteVersion` used to
    /// reconstruct the note at payout (`reconstruct_note`) and which builder spend method
    /// can consume it (`add_orchard_spend` for Orchard/V2, `add_ironwood_spend` for
    /// Ironwood/V3). `#[serde(default)]` → Orchard, so pre-step-3 persisted notes stay valid.
    #[serde(default = "default_pool")]
    pub pool: NotePool,
}

pub fn parse_fvk(hex_str: &str) -> Result<FullViewingKey, String> {
    let bytes = hex::decode(hex_str.trim()).map_err(|e| format!("fvk hex: {}", e))?;
    if bytes.len() != 96 { return Err(format!("fvk wrong length: {}", bytes.len())); }
    FullViewingKey::read(&mut Cursor::new(bytes)).map_err(|e| format!("fvk parse: {}", e))
}

struct CompactOutput {
    epk: [u8; 32],
    cmx: [u8; 32],
    ct: [u8; 52],
}

impl ShieldedOutput<OrchardDomain, COMPACT_NOTE_SIZE> for CompactOutput {
    fn ephemeral_key(&self) -> EphemeralKeyBytes { EphemeralKeyBytes(self.epk) }
    fn cmstar_bytes(&self) -> [u8; 32] { self.cmx }
    fn enc_ciphertext(&self) -> &[u8; COMPACT_NOTE_SIZE] { &self.ct }
}

struct FullOutput {
    epk: [u8; 32],
    cmx: [u8; 32],
    enc: [u8; ENC_CIPHERTEXT_SIZE],
}

impl ShieldedOutput<OrchardDomain, ENC_CIPHERTEXT_SIZE> for FullOutput {
    fn ephemeral_key(&self) -> EphemeralKeyBytes { EphemeralKeyBytes(self.epk) }
    fn cmstar_bytes(&self) -> [u8; 32] { self.cmx }
    fn enc_ciphertext(&self) -> &[u8; ENC_CIPHERTEXT_SIZE] { &self.enc }
}

/// Locate the 580-byte enc_ciphertext for an action matching `(cmx, epk)` within a raw V5
/// orchard tx. Each action lays out as cv(32) + nf(32) + rk(32) + cmx(32) + epk(32) + enc(580)
/// + out(80) — so once we find cmx+epk back-to-back, enc follows immediately.
/// (Inlined from `zync-core::sync::extract_enc_ciphertext`; same logic, no extra dep.)
fn extract_enc_ciphertext(
    raw_tx: &[u8],
    cmx: &[u8; 32],
    epk: &[u8; 32],
) -> Option<[u8; ENC_CIPHERTEXT_SIZE]> {
    for i in 0..raw_tx.len().saturating_sub(64 + ENC_CIPHERTEXT_SIZE) {
        if &raw_tx[i..i + 32] == cmx && &raw_tx[i + 32..i + 64] == epk {
            let start = i + 64;
            let end = start + ENC_CIPHERTEXT_SIZE;
            if end <= raw_tx.len() {
                let mut enc = [0u8; ENC_CIPHERTEXT_SIZE];
                enc.copy_from_slice(&raw_tx[start..end]);
                return Some(enc);
            }
        }
    }
    None
}

/// Parse the deposit memo `zk.poker/v1/payout:<u1addr>[;id:<64-hex>]`.
/// Returns the payout address and, when present, the depositor's 32-byte Ed25519
/// identity pubkey. The pubkey is pinned ON-CHAIN by the depositor here — only the
/// party who owns the deposit can set it — so at settlement escrow can require a
/// signature from exactly this key. The operator cannot substitute its own key.
pub(crate) fn parse_payout_memo(memo_bytes: &[u8]) -> Option<(String, Option<[u8; 32]>)> {
    let end = memo_bytes.iter().rposition(|&b| b != 0).map(|i| i + 1).unwrap_or(0);
    let text = std::str::from_utf8(&memo_bytes[..end]).ok()?;
    let suffix = text.strip_prefix(PAYOUT_MEMO_PREFIX)?.trim();
    // split off an optional `;id:<hex>` identity-pin segment
    let (addr_part, id_part) = match suffix.split_once(";id:") {
        Some((a, id)) => (a.trim(), Some(id.trim())),
        None => (suffix, None),
    };
    if !(addr_part.starts_with("u1") || addr_part.starts_with("utest1") || addr_part.starts_with("uregtest1")) {
        return None;
    }
    if addr_part.len() < 20 || addr_part.len() > 256 { return None; }
    let pubkey = id_part.and_then(|h| {
        let bytes = hex::decode(h).ok()?;
        <[u8; 32]>::try_from(bytes.as_slice()).ok()
    });
    Some((addr_part.to_string(), pubkey))
}

/// Scan from `last_height + 1` to tip and return every note that landed at one of
/// `seat_addr_bytes`. `(seat_addr_bytes[i] == Some(b))` ⇒ recipient is seat `i`.
pub async fn scan(
    client: &ZidecarClient,
    fvk: &FullViewingKey,
    last_height: u32,
    seat_addr_bytes: &[Option<[u8; 43]>],
    network: zcash_protocol::consensus::NetworkType,
) -> Result<(u32, Vec<DepositNote>), String> {
    let (tip, _) = client.get_tip().await.map_err(|e| format!("get_tip: {}", e))?;
    // The Orchard-activation floor skips scanning ancient pre-Orchard blocks. It is a MAINNET
    // height (~1.68M) and is a no-op there (rooms anchor at the current tip, far past it). On
    // testnet/regtest — where NU5/NU6.3 are active from a low height and tips are small — applying
    // the mainnet floor made `start > tip` ALWAYS, so scan() returned immediately having scanned
    // nothing while still reporting `tip` as scanned: deposits were silently never seen.
    let floor = match network {
        zcash_protocol::consensus::NetworkType::Main => ORCHARD_ACTIVATION_MAINNET,
        _ => 1,
    };
    let start = last_height.saturating_add(1).max(floor);
    if start > tip { return Ok((tip, vec![])); }

    let ivk_ext = PreparedIncomingViewingKey::new(&fvk.to_ivk(Scope::External));
    let mut found = Vec::new();
    let mut current = start;

    // position counter = total orchard cmx commitments before our scan window. seed it from
    // zidecar's tree state at the height we already finished scanning; then bump it once per
    // action as we walk through the new blocks in chain order. payout-time merkle paths key
    // off this leaf index.
    let mut position_counter: u64 = if last_height > 0 {
        match client.get_tree_state(last_height).await {
            Ok((hex, _)) => match hex::decode(&hex) {
                Ok(bytes) => zecli::witness::frontier_tree_size(&bytes).unwrap_or(0),
                Err(_) => 0,
            },
            Err(e) => {
                tracing::warn!("scanner: get_tree_state({}) failed, positions start at 0: {}", last_height, e);
                0
            }
        }
    } else { 0 };

    while current <= tip {
        let end = (current + BATCH_SIZE - 1).min(tip);
        let blocks = client.get_compact_blocks(current, end).await
            .map_err(|e| format!("get_compact_blocks {}..{}: {}", current, end, e))?;

        for block in &blocks {
            // ── ORCHARD pool (legacy, V2) ──────────────────────────────────────────────
            // position_counter counts EVERY orchard cmx leaf in chain order, decrypted or
            // not — so it must advance for every action before the (maybe-skipping) decrypt.
            // Ironwood notes live in a SEPARATE commitment tree, so they must NOT bump the
            // orchard position counter (their leaf index comes from the ironwood tree).
            for action in &block.actions {
                let action_position = position_counter;
                position_counter = position_counter.saturating_add(1);
                if let Some(note) = extract_deposit_from_action(
                    client, &ivk_ext, seat_addr_bytes, action, block.height, action_position,
                    NotePool::Orchard,
                ).await {
                    found.push(note);
                }
            }

            // ── IRONWOOD pool (NU6.3+, V3) ─────────────────────────────────────────────
            // Same FVK/IVK + OrchardDomain; the note's own lead byte yields V3. Ironwood
            // leaf positions index the ironwood tree, which zecli does not expose an
            // offset/tree-state for yet — so confirmed ironwood notes are emitted with
            // `NO_POSITION` and cannot yet build a payout witness (see report). They still
            // credit the room deposit (pool-agnostic). Today `ironwood_actions_of` is empty
            // until zecli surfaces `CompactTx.ironwoodActions`; this arm is a no-op until then.
            let iw_actions = ironwood_actions_of(block);
            if !iw_actions.is_empty() {
                tracing::debug!("scan {}: {} orchard, {} ironwood action(s)", block.height, block.actions.len(), iw_actions.len());
            }
            for action in iw_actions {
                if let Some(note) = extract_deposit_from_action(
                    client, &ivk_ext, seat_addr_bytes, action, block.height, NO_POSITION,
                    NotePool::Ironwood,
                ).await {
                    tracing::debug!("scan {}: ironwood note attributed to seat {} ({} zat)", block.height, note.seat, note.value_zat);
                    found.push(note);
                }
            }
        }
        current = end + 1;
    }

    Ok((tip, found))
}

/// The Ironwood compact actions for a block. Ironwood notes are carried in a SEPARATE
/// bundle from Orchard (proto `CompactTx.ironwoodActions` = 9, zidecar commit 4edd4f2);
/// the pool is a property of which bundle an action lives in, not of the ciphertext.
///
/// `zecli::client::CompactBlock` (rev f497a277) carries this as a first-class field —
/// `ironwood_actions`, populated by `convert_actions(block.ironwood_actions)` at both
/// GetCompactBlocks decode sites (bin/zcli/src/client.rs). Post-NU6.3 the Orchard pool is
/// sealed and every new deposit lands here, so an escrow whose ironwood arm iterated an empty
/// slice was BLIND to real deposits while passing every unit test (in-process notes go through
/// the orchard arm). This returns the block's real ironwood actions.
#[inline]
fn ironwood_actions_of(block: &CompactBlock) -> &[zecli::client::CompactAction] {
    &block.ironwood_actions
}

/// Scan the current MEMPOOL (0-conf) for deposits to our seats via zidecar's
/// `GetMempoolStream`. Detection, seat attribution, cmx verification and memo parsing are
/// byte-for-byte identical to the confirmed `scan` — the ONLY differences are that these notes
/// carry `block_height = 0` and `position = NO_POSITION`. A mempool note is provisional: it has
/// no commitment-tree leaf yet, so callers MUST treat it as a UX/"seen it" signal only and must
/// NEVER add it to the spendable note set or build a payout witness from it. When the same tx is
/// later mined, the confirmed scanner emits the fully-positioned note and the caller's
/// nullifier dedup makes the transition idempotent.
pub async fn scan_mempool(
    client: &ZidecarClient,
    fvk: &FullViewingKey,
    seat_addr_bytes: &[Option<[u8; 43]>],
) -> Result<Vec<DepositNote>, String> {
    let ivk_ext = PreparedIncomingViewingKey::new(&fvk.to_ivk(Scope::External));
    // one synthetic height-0 CompactBlock per unconfirmed shielded tx
    let blocks = client
        .get_mempool_stream()
        .await
        .map_err(|e| format!("get_mempool_stream: {}", e))?;

    let mut found = Vec::new();
    for block in &blocks {
        // Orchard mempool actions (legacy, V2).
        for action in &block.actions {
            if let Some(note) = extract_deposit_from_action(
                client, &ivk_ext, seat_addr_bytes, action, 0, NO_POSITION, NotePool::Orchard,
            )
            .await
            {
                found.push(note);
            }
        }
        // Ironwood mempool actions (NU6.3+, V3) — same key material; empty until zecli
        // surfaces the ironwood field (see `ironwood_actions_of`).
        for action in ironwood_actions_of(block) {
            if let Some(note) = extract_deposit_from_action(
                client, &ivk_ext, seat_addr_bytes, action, 0, NO_POSITION, NotePool::Ironwood,
            )
            .await
            {
                found.push(note);
            }
        }
    }
    Ok(found)
}

/// Trial-decrypt one compact action, attribute it to a seat, and (on a hit) recover the memo.
/// Shared by the confirmed block scan and the mempool scan across BOTH pools; `block_height`
/// / `position` differ per caller (mempool passes `0` / `NO_POSITION`; ironwood passes
/// `NO_POSITION` until its tree offset is available), and `pool` fixes which shielded pool
/// the action came from (the ciphertext does not encode it). The same `OrchardDomain`
/// decrypts either pool — we then assert the recovered note's own version matches the pool
/// we scanned it from (V2 ⇔ Orchard, V3 ⇔ Ironwood); a mismatch means the action was in the
/// wrong bundle and is skipped defensively.
async fn extract_deposit_from_action(
    client: &ZidecarClient,
    ivk_ext: &PreparedIncomingViewingKey,
    seat_addr_bytes: &[Option<[u8; 43]>],
    action: &zecli::client::CompactAction,
    block_height: u32,
    position: u64,
    pool: NotePool,
) -> Option<DepositNote> {
    // Pure, network-free part: trial-decrypt, verify version+cmx, attribute a seat, and build
    // the DepositNote with memo fields still empty. Same for both pools; unit-tested directly.
    let (mut note_out, domain) =
        decrypt_action_to_note(ivk_ext, seat_addr_bytes, action, block_height, position, pool)?;

    // re-decrypt the full ciphertext to recover the 512-byte memo. extra round-trip per matched
    // action only; the vast majority of actions have no hits, so cost stays bounded. Works for
    // mempool txs too — zidecar's GetTransaction serves unconfirmed txids.
    let parsed_memo = match client.get_transaction(&action.txid).await {
        Ok(raw_tx) => extract_enc_ciphertext(&raw_tx, &action.cmx, &action.ephemeral_key).and_then(|enc| {
            let full = FullOutput { epk: action.ephemeral_key, cmx: action.cmx, enc };
            try_note_decryption(&domain, ivk_ext, &full).and_then(|(_, _, memo)| parse_payout_memo(&memo))
        }),
        Err(e) => {
            tracing::warn!("scanner: get_transaction failed, leaving memo unparsed: {}", e);
            None
        }
    };
    note_out.payout_address = parsed_memo.as_ref().map(|(a, _)| a.clone());
    note_out.identity_pubkey = parsed_memo.and_then(|(_, pk)| pk);
    Some(note_out)
}

/// The network-free core of a compact-action scan, shared by both pools and directly
/// unit-tested: trial-decrypt with the escrow IVK, verify the note's version matches the
/// `pool` we scanned it from, verify the cmx (anti-malicious-zidecar), attribute a seat, and
/// build a `DepositNote` whose memo fields (`payout_address` / `identity_pubkey`) are left
/// `None` for the caller to fill from a full-tx re-decryption. Returns the note plus the
/// `OrchardDomain` so the caller can reuse it for that memo decryption. `None` if the action
/// is undecryptable, mis-versioned, cmx-mismatched, or lands on an unattributed diversifier.
fn decrypt_action_to_note(
    ivk_ext: &PreparedIncomingViewingKey,
    seat_addr_bytes: &[Option<[u8; 43]>],
    action: &zecli::client::CompactAction,
    block_height: u32,
    position: u64,
    pool: NotePool,
) -> Option<(DepositNote, OrchardDomain)> {
    if action.ciphertext.len() < 52 {
        return None;
    }
    let mut ct = [0u8; 52];
    ct.copy_from_slice(&action.ciphertext[..52]);

    let nf = orchard::note::Nullifier::from_bytes(&action.nullifier).into_option()?;
    let cmx_obj = orchard::note::ExtractedNoteCommitment::from_bytes(&action.cmx).into_option()?;
    let compact = orchard::note_encryption::CompactAction::from_parts(
        nf,
        cmx_obj,
        EphemeralKeyBytes(action.ephemeral_key),
        ct,
    );
    let domain = OrchardDomain::for_compact_action(&compact);
    let output = CompactOutput { epk: action.ephemeral_key, cmx: action.cmx, ct };

    let (note, addr) = try_compact_note_decryption(&domain, ivk_ext, &output)?;

    // Pool/version consistency: an Orchard-bundle action must decrypt to a V2 note and an
    // Ironwood-bundle action to a V3 note. If the recovered version disagrees with the pool
    // we scanned it from, the action was mis-bundled (or a malicious zidecar mixed pools) —
    // skip it rather than persist a note whose stored pool would spend it via the wrong
    // builder method later.
    if note.version() != pool.note_version() {
        tracing::warn!(
            "scanner: note version {:?} does not match scanned pool {:?}, skipping action",
            note.version(),
            pool,
        );
        return None;
    }

    // cmx verification — recompute and compare; protects against a malicious zidecar
    let recomputed = orchard::note::ExtractedNoteCommitment::from(note.commitment());
    if recomputed.to_bytes() != action.cmx {
        tracing::warn!("scanner: cmx mismatch, skipping action");
        return None;
    }

    let addr_bytes = addr.to_raw_address_bytes();
    let seat = match seat_addr_bytes.iter().position(|b| b.as_ref() == Some(&addr_bytes)) {
        Some(s) => s,
        None => {
            tracing::debug!("scanner: deposit to unattributed diversifier — skipping");
            return None;
        }
    };

    let deposit = DepositNote {
        seat: seat as u8,
        value_zat: note.value().inner(),
        txid: action.txid.clone(),
        block_height,
        payout_address: None,
        identity_pubkey: None,
        nullifier: action.nullifier,
        cmx: action.cmx,
        recipient: addr_bytes,
        rho: note.rho().to_bytes(),
        rseed: *note.rseed().as_bytes(),
        position,
        pool,
    };
    Some((deposit, domain))
}

#[cfg(test)]
mod tests {
    use super::*;
    use orchard::keys::{OutgoingViewingKey, SpendingKey};
    use orchard::note::{NoteVersion, Rho};
    use orchard::note_encryption::OrchardNoteEncryption;
    use orchard::value::NoteValue;
    use orchard::Note;
    // `epk_bytes` is a method on the `Domain` trait — bring it in scope so we can call
    // `OrchardDomain::epk_bytes(..)` to derive the ephemeral-key bytes for the mock action.
    use zcash_note_encryption::Domain;

    /// Build a compact `zecli::client::CompactAction` that pays `value` to `recipient`, with a
    /// note of the given plaintext `version` (V2 = Orchard, V3 = Ironwood). This is the
    /// scanner's inverse of a real deposit: a note is created + encrypted exactly as a wallet
    /// would, then reduced to the compact (52-byte) ciphertext that a compact block carries.
    /// Mirrors orchard's own `note_encryption::testing::fake_compact_action`, but lets us pin
    /// the note version so we can exercise the ironwood (V3) path. Returns the action and the
    /// note it encodes (so a test can cross-check value/cmx).
    fn mk_compact_action(
        recipient: orchard::Address,
        value: u64,
        version: NoteVersion,
    ) -> (zecli::client::CompactAction, Note) {
        // The receiving action reveals `nf_old` (the nullifier of the note being SPENT to
        // create this output). The output note's `rho` is derived from it — and the scanner
        // rebuilds the trial-decryption domain the same way, via `for_compact_action` →
        // `Rho::from_nf_old(nf_old)`. `Rho::from_bytes(&nf.to_bytes())` is exactly that same
        // value (both are `Rho(nf.inner())`), so the note's rho MUST come from `nf_old` or
        // decryption fails. Search for a byte pattern that is a valid nullifier + rho.
        let (nf_bytes, rho) = (0u8..=255)
            .find_map(|b| {
                let nf = orchard::note::Nullifier::from_bytes(&[b; 32]).into_option()?;
                let rho = Rho::from_bytes(&nf.to_bytes()).into_option()?;
                Some((nf.to_bytes(), rho))
            })
            .expect("test nullifier/rho");
        // Search for a valid rseed under that rho.
        let rseed = (0u8..=255)
            .find_map(|b| Option::from(orchard::note::RandomSeed::from_bytes([b; 32], &rho)))
            .expect("test rseed");
        let note: Note = Option::from(Note::from_parts(
            recipient,
            NoteValue::from_raw(value),
            rho,
            rseed,
            version,
        ))
        .expect("note from_parts");

        // No OVK → recipient-only; memo empty (compact scan ignores memo anyway).
        let encryptor = OrchardNoteEncryption::new(None::<OutgoingViewingKey>, note, [0u8; 512]);
        let epk = OrchardDomain::epk_bytes(encryptor.epk()).0;
        let enc = encryptor.encrypt_note_plaintext();
        let cmx = orchard::note::ExtractedNoteCommitment::from(note.commitment()).to_bytes();

        let action = zecli::client::CompactAction {
            cmx,
            ephemeral_key: epk,
            ciphertext: enc[..52].to_vec(),
            nullifier: nf_bytes,
            txid: vec![0xab; 32],
        };
        (action, note)
    }

    /// An escrow-owned FVK + its seat-0 external address (the diversifier the room attributes
    /// deposits to). Deterministic so the test is reproducible.
    fn escrow_fvk_and_seat_addr() -> (FullViewingKey, [u8; 43]) {
        let sk = SpendingKey::from_bytes([3u8; 32]).unwrap();
        let fvk = FullViewingKey::from(&sk);
        let addr = fvk.address_at(0u32, Scope::External).to_raw_address_bytes();
        (fvk, addr)
    }

    /// STEP 3 core: a mocked IRONWOOD (V3) compact action addressed to the escrow is
    /// trial-decrypted — through the SAME `OrchardDomain` + escrow IVK as the Orchard path —
    /// into a `DepositNote` tagged `NotePool::Ironwood`, with the value/seat/cmx recovered.
    /// This is the ironwood-deposit detection the escrow needs post-NU6.3.
    #[test]
    fn ironwood_v3_action_decrypts_to_deposit_note_with_ironwood_pool() {
        let (fvk, seat0) = escrow_fvk_and_seat_addr();
        let ivk_ext = PreparedIncomingViewingKey::new(&fvk.to_ivk(Scope::External));
        let seat_addr_bytes = [Some(seat0), None];

        let value = 750_000u64;
        let recipient = fvk.address_at(0u32, Scope::External);
        let (action, note) = mk_compact_action(recipient, value, NoteVersion::V3);

        // Scan it AS an ironwood-pool action (the caller supplies the pool, per how the real
        // dual-scan feeds ironwood-bundle actions to this same helper).
        let (deposit, _domain) =
            decrypt_action_to_note(&ivk_ext, &seat_addr_bytes, &action, 100, NO_POSITION, NotePool::Ironwood)
                .expect("ironwood V3 action must decrypt to a DepositNote");

        assert_eq!(deposit.pool, NotePool::Ironwood, "must be tagged Ironwood pool");
        assert_eq!(deposit.pool.note_version(), NoteVersion::V3);
        assert_eq!(deposit.seat, 0, "recipient is seat 0");
        assert_eq!(deposit.value_zat, value);
        assert_eq!(deposit.value_zat, note.value().inner());
        assert_eq!(deposit.cmx, action.cmx, "cmx round-trips");
        assert_eq!(deposit.recipient, seat0);
    }

    /// Legacy Orchard (V2) scan still works: the exact same helper, with a V2 note scanned as
    /// `NotePool::Orchard`, yields a `DepositNote` tagged Orchard. Guards that dual-scan did
    /// NOT break the pre-activation deposit path.
    #[test]
    fn orchard_v2_action_still_decrypts_to_deposit_note() {
        let (fvk, seat0) = escrow_fvk_and_seat_addr();
        let ivk_ext = PreparedIncomingViewingKey::new(&fvk.to_ivk(Scope::External));
        let seat_addr_bytes = [Some(seat0), None];

        let value = 1_234_567u64;
        let recipient = fvk.address_at(0u32, Scope::External);
        let (action, _note) = mk_compact_action(recipient, value, NoteVersion::V2);

        let (deposit, _domain) =
            decrypt_action_to_note(&ivk_ext, &seat_addr_bytes, &action, 100, 5, NotePool::Orchard)
                .expect("legacy Orchard V2 action must still decrypt");

        assert_eq!(deposit.pool, NotePool::Orchard);
        assert_eq!(deposit.pool.note_version(), NoteVersion::V2);
        assert_eq!(deposit.seat, 0);
        assert_eq!(deposit.value_zat, value);
        assert_eq!(deposit.position, 5, "orchard note keeps its tree position");
    }

    /// Pool/version guard: a V3 (ironwood) note fed to the ORCHARD arm — i.e. a mis-bundled or
    /// malicious-zidecar-mixed action — is rejected, not silently mis-tagged as an Orchard V2
    /// deposit (which would later be spent via the wrong builder method). Symmetric for V2 in
    /// the ironwood arm.
    #[test]
    fn pool_version_mismatch_is_rejected() {
        let (fvk, seat0) = escrow_fvk_and_seat_addr();
        let ivk_ext = PreparedIncomingViewingKey::new(&fvk.to_ivk(Scope::External));
        let seat_addr_bytes = [Some(seat0), None];
        let recipient = fvk.address_at(0u32, Scope::External);

        // V3 note, but scanned as Orchard → reject.
        let (v3_action, _) = mk_compact_action(recipient, 500_000, NoteVersion::V3);
        assert!(
            decrypt_action_to_note(&ivk_ext, &seat_addr_bytes, &v3_action, 1, 0, NotePool::Orchard)
                .is_none(),
            "V3 note in the Orchard arm must be rejected"
        );

        // V2 note, but scanned as Ironwood → reject.
        let (v2_action, _) = mk_compact_action(recipient, 500_000, NoteVersion::V2);
        assert!(
            decrypt_action_to_note(&ivk_ext, &seat_addr_bytes, &v2_action, 1, 0, NotePool::Ironwood)
                .is_none(),
            "V2 note in the Ironwood arm must be rejected"
        );
    }

    /// connect to the live zidecar, scan the last ~3 blocks for the multisig the
    /// in-process DKG test produced. doesn't assert deposits — just exercises the
    /// wire path. run with:
    ///   cargo test --release -p poker-escrow -- --ignored scanner_live
    #[tokio::test]
    #[ignore]
    async fn scanner_live() {
        let url = std::env::var("ZIDECAR_URL").unwrap_or_else(|_| "https://zcash.rotko.net".into());
        let client = ZidecarClient::connect(&url).await.expect("zidecar connect");
        let (tip, _) = client.get_tip().await.expect("get_tip");
        // a random fvk; we expect zero hits but the path should not error
        let fvk_bytes = [0u8; 96];
        let fvk = FullViewingKey::read(&mut Cursor::new(fvk_bytes));
        if fvk.is_err() {
            eprintln!("skipping — zero FVK is invalid (expected)");
            return;
        }
        let _ = scan(&client, &fvk.unwrap(), tip.saturating_sub(3), &[None, None]).await;
    }

    /// Verify the deployed zidecar actually SERVES `GetMempoolStream` (it landed 2026-03-15;
    /// an older prod binary would return Unimplemented). Read-only. run with:
    ///   cargo test --release -p poker-escrow -- --ignored mempool_endpoint_live --nocapture
    #[tokio::test]
    #[ignore]
    async fn mempool_endpoint_live() {
        let url = std::env::var("ZIDECAR_URL").unwrap_or_else(|_| "https://zcash.rotko.net".into());
        let client = ZidecarClient::connect(&url).await.expect("zidecar connect");
        match client.get_mempool_stream().await {
            Ok(blocks) => {
                let actions: usize = blocks.iter().map(|b| b.actions.len()).sum();
                eprintln!(
                    "GetMempoolStream OK — {} unconfirmed shielded tx(s), {} orchard action(s)",
                    blocks.len(),
                    actions
                );
            }
            Err(e) => panic!("GetMempoolStream NOT served by prod zidecar (needs redeploy?): {}", e),
        }
    }
}
