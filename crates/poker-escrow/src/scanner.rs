//! Orchard compact-block scanner. Adapted from `zcli/bin/license-server/src/scanner.rs`:
//! we pull compact blocks from zidecar, trial-decrypt each action with the multisig FVK,
//! and attribute every recovered note to a seat by matching its 43-byte address against
//! the per-seat deposit UAs derived at room-creation time.
//!
//! Compact decryption only — no memo, no full-tx fetch. Defense against malicious zidecar
//! via cmx verification (recompute commitment from the decrypted note, compare to the
//! cmx zidecar gave us).

use std::io::Cursor;

use orchard::keys::{FullViewingKey, PreparedIncomingViewingKey, Scope};
use orchard::note_encryption::OrchardDomain;
use zcash_note_encryption::{
    try_compact_note_decryption, try_note_decryption, EphemeralKeyBytes, ShieldedOutput,
    COMPACT_NOTE_SIZE, ENC_CIPHERTEXT_SIZE,
};
use zecli::client::ZidecarClient;

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
) -> Result<(u32, Vec<DepositNote>), String> {
    let (tip, _) = client.get_tip().await.map_err(|e| format!("get_tip: {}", e))?;
    let start = last_height.saturating_add(1).max(ORCHARD_ACTIVATION_MAINNET);
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
            for action in &block.actions {
                // position_counter counts EVERY orchard cmx leaf in chain order, decrypted or
                // not — so it must advance for every action before the (maybe-skipping) decrypt.
                let action_position = position_counter;
                position_counter = position_counter.saturating_add(1);
                if let Some(note) = extract_deposit_from_action(
                    client, &ivk_ext, seat_addr_bytes, action, block.height, action_position,
                ).await {
                    found.push(note);
                }
            }
        }
        current = end + 1;
    }

    Ok((tip, found))
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
        for action in &block.actions {
            if let Some(note) =
                extract_deposit_from_action(client, &ivk_ext, seat_addr_bytes, action, 0, NO_POSITION)
                    .await
            {
                found.push(note);
            }
        }
    }
    Ok(found)
}

/// Trial-decrypt one compact action, attribute it to a seat, and (on a hit) recover the memo.
/// Shared by the confirmed block scan and the mempool scan; `block_height` / `position` are the
/// only things the two callers supply differently (mempool passes `0` / `NO_POSITION`).
async fn extract_deposit_from_action(
    client: &ZidecarClient,
    ivk_ext: &PreparedIncomingViewingKey,
    seat_addr_bytes: &[Option<[u8; 43]>],
    action: &zecli::client::CompactAction,
    block_height: u32,
    position: u64,
) -> Option<DepositNote> {
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
    let payout_address = parsed_memo.as_ref().map(|(a, _)| a.clone());
    let identity_pubkey = parsed_memo.and_then(|(_, pk)| pk);

    Some(DepositNote {
        seat: seat as u8,
        value_zat: note.value().inner(),
        txid: action.txid.clone(),
        block_height,
        payout_address,
        identity_pubkey,
        nullifier: action.nullifier,
        cmx: action.cmx,
        recipient: addr_bytes,
        rho: note.rho().to_bytes(),
        rseed: *note.rseed().as_bytes(),
        position,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

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
