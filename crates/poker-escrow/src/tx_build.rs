//! Build a FROST-payable Orchard payout PCZT using zcli's native helpers.
//!
//! The pub fns + helpers here are dead-code until Phase 5.2 wires them into a real payout
//! handler. Marking the module rather than each fn so the noise stays in one place.
#![allow(dead_code)]

//!
//! Phase 5.1c-β. Single-source-of-truth: relies on `zecli::pczt::build_pczt_and_qr` to do
//! the heavy lifting (Builder → build_for_pczt → finalize_io → halo2 proving → expose
//! sighash + alphas) and `zecli::pczt::complete_pczt_tx` to inject FROST sigs + serialize.
//!
//! Flow for the caller:
//!   1. `build_payout_pczt(...)`        → `PcztState` (sighash, alphas, pczt_bundle)
//!   2. run FROST signing per action    → `Vec<[u8; 64]>` SpendAuth sigs
//!   3. `zecli::pczt::complete_pczt_tx` → v5 tx bytes ready for `send_transaction`

use orchard::note::{RandomSeed, Rho};
use orchard::tree::{Anchor, MerkleHashOrchard, MerklePath};
use orchard::value::NoteValue;
use orchard::{Address, Note};
use zecli::client::ZidecarClient;
use zecli::pczt::PcztState;
use zecli::wallet::WalletNote;

use crate::scanner::DepositNote;

/// One output line in a payout: where to send + how much + memo.
#[derive(Debug, Clone)]
pub struct PayoutOutput {
    pub address: Address,
    pub amount_zat: u64,
    pub memo: [u8; 512],
}

#[derive(Debug, Clone)]
pub struct PayoutPlan {
    pub outputs: Vec<PayoutOutput>,
    pub fee_zat: u64,
}

#[derive(Debug, thiserror::Error)]
pub enum TxBuildError {
    #[error("witness: {0}")]
    Witness(String),
    #[error("note reconstruct: {0}")]
    NoteReconstruct(String),
    #[error("balance: have {have} zat, need {need}")]
    Balance { have: u64, need: u64 },
    #[error("zecli::pczt: {0}")]
    Pczt(String),
}

/// Build the unsigned PCZT for a payout. Returns a `PcztState` whose `sighash` + `alphas`
/// feed the FROST signing dance; the caller then passes the state + signatures into
/// `zecli::pczt::complete_pczt_tx` to get broadcast-ready v5 tx bytes.
pub async fn build_payout_pczt(
    client: &ZidecarClient,
    fvk_bytes: &[u8; 96],
    notes: &[DepositNote],
    plan: &PayoutPlan,
    anchor_height: u32,
    mainnet: bool,
) -> Result<PcztState, TxBuildError> {
    if notes.is_empty() {
        return Err(TxBuildError::NoteReconstruct("no input notes".into()));
    }

    // balance check first (cheaper than building anything)
    let total_in: u64 = notes.iter().map(|n| n.value_zat).sum();
    let total_out: u64 = plan.outputs.iter().map(|o| o.amount_zat).sum();
    let need = total_out.saturating_add(plan.fee_zat);
    if total_in < need {
        return Err(TxBuildError::Balance { have: total_in, need });
    }
    let change = total_in - need;

    // sync_height must predate every input note so the replay (sync_height..=anchor] visits the block each note was committed in
    let wallet_notes: Vec<WalletNote> = notes.iter().map(deposit_to_wallet_note).collect();
    let min_note_height = notes.iter().map(|n| n.block_height).min().unwrap_or(anchor_height);
    let sync_height = min_note_height.saturating_sub(1).max(1);
    let (anchor, paths) = zecli::witness::build_witnesses(
        client, &wallet_notes, anchor_height, mainnet,
        /* json */ false,
        /* cached_frontier */ None,
        sync_height,
    ).await.map_err(|e| TxBuildError::Witness(e.to_string()))?;
    if paths.len() != notes.len() {
        return Err(TxBuildError::Witness(format!(
            "{} paths but {} notes", paths.len(), notes.len()
        )));
    }

    // reconstruct orchard::Note from stored bytes + pair with its merkle path
    let mut spends: Vec<(Note, MerklePath)> = Vec::with_capacity(notes.len());
    for (i, (n, path)) in notes.iter().zip(paths.into_iter()).enumerate() {
        let note = reconstruct_note(n)
            .map_err(|e| TxBuildError::NoteReconstruct(format!("note {}: {}", i, e)))?;
        // sanity: the path we got must lead to an anchor; trust zecli that part
        let _ = MerkleHashOrchard::from_bytes(&[0u8; 32]); // type assertion
        spends.push((note, path));
    }

    let z_outputs: Vec<(Address, u64, [u8; 512])> = plan.outputs.iter()
        .map(|o| (o.address, o.amount_zat, o.memo))
        .collect();

    let (_qr_bytes, state) = zecli::pczt::build_pczt_and_qr(
        fvk_bytes,
        &spends,
        &z_outputs,
        /* t_outputs */ &[],
        change,
        anchor,
        anchor_height,
        mainnet,
    ).map_err(|e| TxBuildError::Pczt(format!("{}", e)))?;

    Ok(state)
}

fn deposit_to_wallet_note(d: &DepositNote) -> WalletNote {
    WalletNote {
        value: d.value_zat,
        nullifier: d.nullifier,
        cmx: d.cmx,
        block_height: d.block_height,
        is_change: false,
        recipient: d.recipient.to_vec(),
        rho: d.rho,
        rseed: d.rseed,
        position: d.position,
        txid: d.txid.clone(),
        memo: None,
    }
}

fn reconstruct_note(d: &DepositNote) -> Result<Note, String> {
    let recipient = Option::from(Address::from_raw_address_bytes(&d.recipient))
        .ok_or_else(|| "invalid recipient bytes".to_string())?;
    let rho = Option::from(Rho::from_bytes(&d.rho))
        .ok_or_else(|| "invalid rho".to_string())?;
    let rseed = Option::from(RandomSeed::from_bytes(d.rseed, &rho))
        .ok_or_else(|| "invalid rseed".to_string())?;
    let note: Note = Option::<Note>::from(Note::from_parts(recipient, NoteValue::from_raw(d.value_zat), rho, rseed))
        .ok_or_else(|| "Note::from_parts failed".to_string())?;
    let computed = orchard::note::ExtractedNoteCommitment::from(note.commitment()).to_bytes();
    if computed != d.cmx {
        return Err(format!("cmx mismatch: stored={} reconstructed={}",
            hex::encode(d.cmx), hex::encode(computed)));
    }
    Ok(note)
}

/// Helper: parse a zcash UA string into an `orchard::Address` for use in a `PayoutOutput`.
/// Thin wrapper around `zecli::tx::parse_orchard_address`. Used by callers wiring real
/// `seat_payout_addresses` from `EscrowRoom` into a `PayoutPlan`.
pub fn parse_orchard_ua(ua: &str, mainnet: bool) -> Result<Address, String> {
    zecli::tx::parse_orchard_address(ua, mainnet).map_err(|e| format!("{}", e))
}
