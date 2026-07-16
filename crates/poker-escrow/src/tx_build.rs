//! Build a FROST-payable Orchard payout PCZT using the standard pczt pipeline.
//!
//! Uses zcash_primitives::Builder → pczt::Creator → Prover → IoFinalizer to produce
//! a standard pczt::Pczt byte stream. The host includes the PCZT hex in the SIGN message
//! so the zafu joiner can independently verify the recipient/amount via OVK-decryption
//! before contributing their FROST share (gh #17 migration, matches zafu relay-protocol.ts).

use ff::PrimeField;
use orchard::keys::{FullViewingKey, OutgoingViewingKey, Scope};
use orchard::note::{RandomSeed, Rho};
use orchard::tree::{Anchor, MerklePath};
use orchard::value::NoteValue;
use orchard::{Address, Note};
use rand::rngs::OsRng;
use zcash_protocol::consensus::BlockHeight;
use zcash_protocol::value::Zatoshis;

use zecli::client::ZidecarClient;
use zecli::wallet::WalletNote;

use crate::scanner::DepositNote;

/// One output line in a payout: where to send + how much.
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
    #[error("pczt build: {0}")]
    Pczt(String),
}

/// Build result from the PCZT pipeline: serialized PCZT bytes + FROST signing data.
pub struct PcztBuildResult {
    /// Standard pczt::Pczt bytes; included as pcztHex in the SIGN relay message so the
    /// zafu joiner can OVK-verify outputs before contributing their FROST share.
    pub pczt_bytes: Vec<u8>,
    pub sighash: [u8; 32],
    pub alphas: Vec<[u8; 32]>,
    /// Action index of each real (non-dummy) spend — maps 1:1 with alphas and the sigs
    /// returned by host_sign_pczt. Passed to Signer::apply_orchard_signature.
    pub spend_indices: Vec<usize>,
}

/// Build the unsigned PCZT for a payout using the standard pczt crate pipeline.
/// Returns a `PcztBuildResult` whose `sighash` + `alphas` feed the FROST relay dance;
/// call `complete_payout_pczt` after signing to produce a broadcast-ready v5 tx.
pub async fn build_payout_pczt(
    client: &ZidecarClient,
    fvk_bytes: &[u8; 96],
    notes: &[DepositNote],
    plan: &PayoutPlan,
    anchor_height: u32,
    mainnet: bool,
) -> Result<PcztBuildResult, TxBuildError> {
    if notes.is_empty() {
        return Err(TxBuildError::NoteReconstruct("no input notes".into()));
    }

    let total_in: u64 = notes.iter().map(|n| n.value_zat).sum();
    let total_out: u64 = plan.outputs.iter().map(|o| o.amount_zat).sum();
    let need = total_out.saturating_add(plan.fee_zat);
    if total_in < need {
        return Err(TxBuildError::Balance { have: total_in, need });
    }
    let change = total_in - need;

    let wallet_notes: Vec<WalletNote> = notes.iter().map(deposit_to_wallet_note).collect();
    let min_note_height = notes.iter().map(|n| n.block_height).min().unwrap_or(anchor_height);
    let sync_height = min_note_height.saturating_sub(1).max(1);
    let (anchor, paths) = zecli::witness::build_witnesses(
        client,
        &wallet_notes,
        anchor_height,
        mainnet,
        false,
        None,
        sync_height,
    )
    .await
    .map_err(|e| TxBuildError::Witness(e.to_string()))?;

    if paths.len() != notes.len() {
        return Err(TxBuildError::Witness(format!(
            "{} paths but {} notes",
            paths.len(),
            notes.len()
        )));
    }

    let mut spends: Vec<(Note, MerklePath)> = Vec::with_capacity(notes.len());
    for (i, (n, path)) in notes.iter().zip(paths.into_iter()).enumerate() {
        let note = reconstruct_note(n)
            .map_err(|e| TxBuildError::NoteReconstruct(format!("note {}: {}", i, e)))?;
        spends.push((note, path));
    }

    let fvk_bytes = *fvk_bytes;
    let outputs: Vec<(Address, u64)> =
        plan.outputs.iter().map(|o| (o.address, o.amount_zat)).collect();
    let fee_zat = plan.fee_zat;

    // CPU-bound PCZT building (proving takes seconds) — run off the async executor.
    tokio::task::spawn_blocking(move || {
        build_pczt_sync(fvk_bytes, spends, outputs, change, anchor, anchor_height, fee_zat, mainnet)
    })
    .await
    .map_err(|e| TxBuildError::Pczt(format!("spawn_blocking: {}", e)))?
    .map_err(TxBuildError::Pczt)
}

/// Injects the aggregated FROST signatures back into the PCZT and extracts a v5 tx.
/// `spend_indices` must parallel `sigs` and match what `build_payout_pczt` returned.
pub fn complete_payout_pczt(
    pczt_bytes: &[u8],
    sigs: &[[u8; 64]],
    spend_indices: &[usize],
) -> Result<Vec<u8>, String> {
    use orchard::circuit::VerifyingKey;
    use orchard::primitives::redpallas;
    use pczt::roles::signer::Signer;
    use pczt::roles::tx_extractor::TransactionExtractor;

    static VK: std::sync::OnceLock<VerifyingKey> = std::sync::OnceLock::new();
    let vk = VK.get_or_init(VerifyingKey::build);

    let pczt = pczt::Pczt::parse(pczt_bytes).map_err(|e| format!("pczt parse: {:?}", e))?;
    let mut signer = Signer::new(pczt).map_err(|e| format!("signer init: {:?}", e))?;

    for (sig_bytes, idx) in sigs.iter().zip(spend_indices.iter()) {
        let sig = redpallas::Signature::<redpallas::SpendAuth>::from(*sig_bytes);
        signer
            .apply_orchard_signature(*idx, sig)
            .map_err(|e| format!("apply_orchard_signature[{}]: {:?}", idx, e))?;
    }

    let signed = signer.finish();
    let tx = TransactionExtractor::new(signed)
        .with_orchard(vk)
        .extract()
        .map_err(|e| format!("tx extract: {:?}", e))?;

    let mut tx_bytes = Vec::new();
    tx.write(&mut tx_bytes)
        .map_err(|e| format!("tx serialize: {}", e))?;
    Ok(tx_bytes)
}

// ── internals ────────────────────────────────────────────────────────────────

fn build_pczt_sync(
    fvk_bytes: [u8; 96],
    spends: Vec<(Note, MerklePath)>,
    outputs: Vec<(Address, u64)>,
    change: u64,
    anchor: Anchor,
    anchor_height: u32,
    fee_zat: u64,
    mainnet: bool,
) -> Result<PcztBuildResult, String> {
    use zcash_primitives::transaction::builder::{BuildConfig, Builder};
    use zcash_primitives::transaction::fees::fixed::FeeRule;
    use zcash_protocol::consensus::{MainNetwork, TestNetwork};
    use zcash_protocol::memo::MemoBytes;
    use pczt::roles::creator::Creator;
    use pczt::roles::io_finalizer::IoFinalizer;
    use pczt::roles::prover::Prover;
    use pczt::roles::signer::Signer;

    let fvk = FullViewingKey::from_bytes(&fvk_bytes)
        .ok_or_else(|| "invalid FVK bytes".to_string())?;
    let ovk_ext: OutgoingViewingKey = fvk.to_ovk(Scope::External);
    let ovk_int: OutgoingViewingKey = fvk.to_ovk(Scope::Internal);

    let build_config = BuildConfig::Standard {
        sapling_anchor: None,
        orchard_anchor: Some(anchor),
    };
    let fee =
        Zatoshis::from_u64(fee_zat).map_err(|e| format!("invalid fee: {:?}", e))?;
    let fee_rule = FeeRule::non_standard(fee);
    let target = BlockHeight::from(anchor_height);
    let memo_empty = MemoBytes::empty();

    macro_rules! run_builder {
        ($params:expr) => {{
            let mut builder = Builder::new($params, target, build_config);
            for (note, path) in &spends {
                builder
                    .add_orchard_spend::<()>(fvk.clone(), *note, path.clone())
                    .map_err(|e| format!("add_orchard_spend: {:?}", e))?;
            }
            for (addr, amount_zat) in &outputs {
                let zat = Zatoshis::from_u64(*amount_zat)
                    .map_err(|e| format!("invalid amount: {:?}", e))?;
                builder
                    .add_orchard_output::<()>(Some(ovk_ext.clone()), *addr, zat, memo_empty.clone())
                    .map_err(|e| format!("add_orchard_output: {:?}", e))?;
            }
            if change > 0 {
                let change_addr = fvk.address_at(0u64, Scope::Internal);
                let change_zat = Zatoshis::from_u64(change)
                    .map_err(|e| format!("invalid change: {:?}", e))?;
                builder
                    .add_orchard_output::<()>(Some(ovk_int.clone()), change_addr, change_zat, MemoBytes::empty())
                    .map_err(|e| format!("add_orchard_output (change): {:?}", e))?;
            }
            builder
                .build_for_pczt(OsRng, &fee_rule)
                .map_err(|e| format!("build_for_pczt: {:?}", e))?
                .pczt_parts
        }};
    }

    let (pczt_parts, alphas, spend_indices) = if mainnet {
        let parts = run_builder!(MainNetwork);
        let (a, s) = extract_alphas(&parts.orchard);
        (Creator::build_from_parts(parts).ok_or("Creator::build_from_parts failed (mainnet)")?, a, s)
    } else {
        let parts = run_builder!(TestNetwork);
        let (a, s) = extract_alphas(&parts.orchard);
        (Creator::build_from_parts(parts).ok_or("Creator::build_from_parts failed (testnet)")?, a, s)
    };

    static PK: std::sync::OnceLock<orchard::circuit::ProvingKey> = std::sync::OnceLock::new();
    let pk = PK.get_or_init(orchard::circuit::ProvingKey::build);

    let pczt = Prover::new(pczt_parts)
        .create_orchard_proof(pk)
        .map_err(|e| format!("create_orchard_proof: {:?}", e))?
        .finish();

    let pczt = IoFinalizer::new(pczt)
        .finalize_io()
        .map_err(|e| format!("finalize_io: {:?}", e))?;

    let pczt_bytes = pczt.serialize();

    // Derive the canonical sighash from the PCZT bytes we'll send to the joiner.
    let sighash = {
        let reparsed =
            pczt::Pczt::parse(&pczt_bytes).map_err(|e| format!("pczt reparse: {:?}", e))?;
        Signer::new(reparsed)
            .map_err(|e| format!("signer for sighash: {:?}", e))?
            .shielded_sighash()
    };

    Ok(PcztBuildResult { pczt_bytes, sighash, alphas, spend_indices })
}

fn extract_alphas(
    orchard: &Option<orchard::pczt::Bundle>,
) -> (Vec<[u8; 32]>, Vec<usize>) {
    let mut alphas = Vec::new();
    let mut indices = Vec::new();
    if let Some(b) = orchard {
        for (i, action) in b.actions().iter().enumerate() {
            if action.spend().dummy_sk().is_none() {
                if let Some(alpha) = action.spend().alpha() {
                    alphas.push(alpha.to_repr());
                    indices.push(i);
                }
            }
        }
    }
    (alphas, indices)
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
    let rho = Option::from(Rho::from_bytes(&d.rho)).ok_or_else(|| "invalid rho".to_string())?;
    let rseed = Option::from(RandomSeed::from_bytes(d.rseed, &rho))
        .ok_or_else(|| "invalid rseed".to_string())?;
    let note: Note = Option::<Note>::from(Note::from_parts(
        recipient,
        NoteValue::from_raw(d.value_zat),
        rho,
        rseed,
    ))
    .ok_or_else(|| "Note::from_parts failed".to_string())?;
    let computed =
        orchard::note::ExtractedNoteCommitment::from(note.commitment()).to_bytes();
    if computed != d.cmx {
        return Err(format!(
            "cmx mismatch: stored={} reconstructed={}",
            hex::encode(d.cmx),
            hex::encode(computed)
        ));
    }
    Ok(note)
}

/// Parse a zcash UA string into an `orchard::Address`.
pub fn parse_orchard_ua(ua: &str, mainnet: bool) -> Result<Address, String> {
    zecli::tx::parse_orchard_address(ua, mainnet).map_err(|e| format!("{}", e))
}
