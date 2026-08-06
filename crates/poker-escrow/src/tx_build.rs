//! Build a FROST-payable payout PCZT using the standard pczt pipeline.
//!
//! Uses zcash_primitives::Builder → pczt::Creator → Prover → IoFinalizer to produce
//! a standard pczt::Pczt byte stream. The host includes the PCZT hex in the SIGN message
//! so the zafu joiner can independently verify the recipient/amount via OVK-decryption
//! before contributing their FROST share (gh #17 migration, matches zafu relay-protocol.ts).
//!
//! ## NU6.3 / Ironwood cutover (step 2)
//!
//! At the NU6.3 activation height the legacy Orchard pool is SEALED: a cross-address
//! Orchard OUTPUT (paying the winner's UA out of the escrow's Orchard deposit notes) is
//! un-encodable (`CrossAddressDisabled`) on the PostNu6_3 circuit. So above the activation
//! height the payout is rebuilt as a **V6 transaction** that SPENDS the escrow's Orchard
//! deposit notes and OUTPUTS the winnings into the **Ironwood** pool (Ironwood permits
//! arbitrary/cross-address recipients). Below the activation height the historical
//! Orchard-output path is still consensus-valid and is kept verbatim.
//!
//! The V6 path mirrors the proven turnstile builder in
//! `zcli-ironwood/crates/zcash-wasm/src/lib.rs::build_turnstile_migration_pczt_proven`,
//! specialised to send to the winner/house instead of self. The FROST signing shape is
//! UNCHANGED — the payout is still FROST-cosigned over the Orchard SPEND actions (alphas
//! come from the orchard bundle spends exactly as before) — but it now signs the **V6
//! sighash** that `shielded_sighash()` returns for the V6 PCZT.

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

/// Mainnet NU6.3 activation height. At/above this the legacy Orchard pool is sealed and
/// payouts must be built as V6 (Orchard-spend → Ironwood-output). Matches the vendored
/// librustzcash consensus patch (`Nu6_3 => 0x37a5165b`, activation 3_428_143 on mainnet).
pub const NU6_3_ACTIVATION_MAINNET: u32 = 3_428_143;
/// Testnet NU6.3 activation height (Zcash testnet). Used only for the non-mainnet code
/// path so escrow test/staging deployments cut over on the right height too.
pub const NU6_3_ACTIVATION_TESTNET: u32 = 3_397_752;
/// The REAL NU6.3 / Ironwood consensus branch id (from the live network). The vendored
/// fork binds this at any NU6.3-active height; the fail-closed guard refuses to build a
/// V6 payout unless the branch id we would actually bind equals this.
pub const NU6_3_BRANCH_ID: u32 = 0x37a5_165b;
/// The unpatched fork's placeholder branch id. A tx binding this is unspendable on-chain;
/// the guard refuses it outright so we can never ship an un-broadcastable payout.
const NU6_3_PLACEHOLDER_BRANCH_ID: u32 = 0xffff_ffff;

/// NU6.3 activation height for the network selected by `mainnet`.
fn nu6_3_activation(mainnet: bool) -> u32 {
    if mainnet {
        NU6_3_ACTIVATION_MAINNET
    } else {
        NU6_3_ACTIVATION_TESTNET
    }
}

/// Consensus-params wrapper that reports NU6.3 as active from a given height. The vendored
/// librustzcash already carries the real NU6.3 activation height + branch id, so on mainnet
/// this is a no-op (the real height wins in the `.or()` below). It remains so the builder
/// binds the real `Nu6_3` branch id (→ 0x37a5165b) and creates the ironwood builder even
/// if a caller passes a network whose base params don't yet know NU6.3. Mirrors the
/// `Nu63Activated` wrapper in zcash-wasm. The fail-closed branch-id guard in
/// `build_pczt_sync` still refuses unless the bound id equals `NU6_3_BRANCH_ID`.
#[cfg(zcash_unstable = "nu6.3")]
#[derive(Clone, Copy, Debug)]
struct Nu63Activated<P> {
    inner: P,
    nu6_3_from: zcash_protocol::consensus::BlockHeight,
}

#[cfg(zcash_unstable = "nu6.3")]
impl<P: zcash_protocol::consensus::Parameters> zcash_protocol::consensus::Parameters
    for Nu63Activated<P>
{
    fn network_type(&self) -> zcash_protocol::consensus::NetworkType {
        self.inner.network_type()
    }

    fn activation_height(
        &self,
        nu: zcash_protocol::consensus::NetworkUpgrade,
    ) -> Option<zcash_protocol::consensus::BlockHeight> {
        match nu {
            zcash_protocol::consensus::NetworkUpgrade::Nu6_3 => {
                // Respect a real upstream activation height once one exists.
                self.inner.activation_height(nu).or(Some(self.nu6_3_from))
            }
            _ => self.inner.activation_height(nu),
        }
    }
}

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
    /// The shielded sighash the FROST cohort signs. For a V6 payout this is the V6 sighash;
    /// for a pre-NU6.3 payout it is the V5 sighash. Signing is otherwise identical.
    pub sighash: [u8; 32],
    pub alphas: Vec<[u8; 32]>,
    /// Action index of each real (non-dummy) Orchard SPEND — maps 1:1 with alphas and the
    /// sigs returned by host_sign_pczt. Passed to Signer::apply_orchard_signature. Unchanged
    /// for V6: the payout still FROST-signs the orchard spend actions.
    pub spend_indices: Vec<usize>,
}

/// Build the unsigned PCZT for a payout using the standard pczt crate pipeline.
/// Returns a `PcztBuildResult` whose `sighash` + `alphas` feed the FROST relay dance;
/// call `complete_payout_pczt` after signing to produce a broadcast-ready tx (v5 pre-NU6.3,
/// v6 at/above the activation height).
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

/// Injects the aggregated FROST signatures back into the PCZT and extracts the final tx.
/// `spend_indices` must parallel `sigs` and match what `build_payout_pczt` returned.
///
/// Selects the orchard verifying key from the PCZT's own tx version: a V6 payout's orchard
/// bundle was proved with the PostNu6_3 circuit and additionally carries an Ironwood output
/// bundle (also PostNu6_3), so we add `.with_ironwood(vk)`. A pre-NU6.3 V5 payout still uses
/// the historical circuit that matches the branch it targets. Mirrors
/// zcash-wasm::extract_signed_tx_from_pczt_bytes.
pub fn complete_payout_pczt(
    pczt_bytes: &[u8],
    sigs: &[[u8; 64]],
    spend_indices: &[usize],
) -> Result<Vec<u8>, String> {
    use orchard::circuit::{OrchardCircuitVersion, VerifyingKey};
    use orchard::primitives::redpallas;
    use pczt::roles::signer::Signer;
    use pczt::roles::tx_extractor::TransactionExtractor;

    let pczt = pczt::Pczt::parse(pczt_bytes).map_err(|e| format!("pczt parse: {:?}", e))?;

    // Is this a V6 (orchard-spend → ironwood-output) payout? V6 bundles — both orchard and
    // ironwood — verify against the PostNu6_3 circuit. A V5 payout uses the historical
    // (InsecurePreNu6_2) circuit that matches the consensus branch it was proved on.
    #[cfg(zcash_unstable = "nu6.3")]
    let is_v6 = *pczt.global().tx_version() == zcash_protocol::constants::V6_TX_VERSION;
    #[cfg(not(zcash_unstable = "nu6.3"))]
    let is_v6 = false;

    let orchard_cv = if is_v6 {
        OrchardCircuitVersion::PostNu6_3
    } else {
        OrchardCircuitVersion::InsecurePreNu6_2
    };

    // One VK per circuit version, built once and cached.
    static VK_PRE: std::sync::OnceLock<VerifyingKey> = std::sync::OnceLock::new();
    static VK_POST: std::sync::OnceLock<VerifyingKey> = std::sync::OnceLock::new();
    let vk = match orchard_cv {
        OrchardCircuitVersion::PostNu6_3 => {
            VK_POST.get_or_init(|| VerifyingKey::build(OrchardCircuitVersion::PostNu6_3))
        }
        _ => VK_PRE.get_or_init(|| VerifyingKey::build(OrchardCircuitVersion::InsecurePreNu6_2)),
    };

    let mut signer = Signer::new(pczt).map_err(|e| format!("signer init: {:?}", e))?;

    for (sig_bytes, idx) in sigs.iter().zip(spend_indices.iter()) {
        let sig = redpallas::Signature::<redpallas::SpendAuth>::from(*sig_bytes);
        signer
            .apply_orchard_signature(*idx, sig)
            .map_err(|e| format!("apply_orchard_signature[{}]: {:?}", idx, e))?;
    }

    let signed = signer.finish();
    let extractor = TransactionExtractor::new(signed).with_orchard(vk);
    // V6 carries an Ironwood output bundle that must be verified with the PostNu6_3 VK.
    #[cfg(zcash_unstable = "nu6.3")]
    let extractor = if is_v6 {
        static IW_VK: std::sync::OnceLock<VerifyingKey> = std::sync::OnceLock::new();
        extractor.with_ironwood(IW_VK.get_or_init(|| VerifyingKey::build(OrchardCircuitVersion::PostNu6_3)))
    } else {
        extractor
    };
    let tx = extractor
        .extract()
        .map_err(|e| format!("tx extract: {:?}", e))?;

    let mut tx_bytes = Vec::new();
    tx.write(&mut tx_bytes)
        .map_err(|e| format!("tx serialize: {}", e))?;
    Ok(tx_bytes)
}

// ── internals ────────────────────────────────────────────────────────────────

#[allow(clippy::too_many_arguments)]
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
    // Height-gated cutover. Below NU6.3 activation the legacy Orchard-output payout is still
    // consensus-valid; at/above, the Orchard pool is sealed and we must emit a V6
    // Orchard-spend → Ironwood-output tx.
    if anchor_height >= nu6_3_activation(mainnet) {
        #[cfg(zcash_unstable = "nu6.3")]
        {
            return build_pczt_v6_ironwood(
                fvk_bytes, spends, outputs, change, anchor, anchor_height, fee_zat, mainnet,
            );
        }
        #[cfg(not(zcash_unstable = "nu6.3"))]
        {
            return Err(format!(
                "payout at height {} is at/above NU6.3 activation {} but this build lacks the \
                 nu6.3 cfg — rebuild with RUSTFLAGS='--cfg zcash_unstable=\"nu6.3\"'",
                anchor_height,
                nu6_3_activation(mainnet)
            ));
        }
    }
    build_pczt_orchard_legacy(
        fvk_bytes, spends, outputs, change, anchor, anchor_height, fee_zat, mainnet,
    )
}

/// Pre-NU6.3 payout: spend + output entirely in the legacy Orchard pool, extract a V5 tx.
/// Unchanged from the deployed money path (this is what today's mainnet accepts).
#[allow(clippy::too_many_arguments)]
fn build_pczt_orchard_legacy(
    fvk_bytes: [u8; 96],
    spends: Vec<(Note, MerklePath)>,
    outputs: Vec<(Address, u64)>,
    change: u64,
    anchor: Anchor,
    anchor_height: u32,
    fee_zat: u64,
    mainnet: bool,
) -> Result<PcztBuildResult, String> {
    use pczt::roles::creator::Creator;
    use pczt::roles::io_finalizer::IoFinalizer;
    use pczt::roles::prover::Prover;
    use pczt::roles::signer::Signer;
    use zcash_primitives::transaction::builder::{BuildConfig, Builder, BundlePadding};
    use zcash_primitives::transaction::fees::fixed::FeeRule;
    use zcash_protocol::consensus::{MainNetwork, TestNetwork};
    use zcash_protocol::memo::MemoBytes;

    let fvk = FullViewingKey::from_bytes(&fvk_bytes)
        .ok_or_else(|| "invalid FVK bytes".to_string())?;
    let ovk_ext: OutgoingViewingKey = fvk.to_ovk(Scope::External);
    let ovk_int: OutgoingViewingKey = fvk.to_ovk(Scope::Internal);

    let build_config = BuildConfig::Standard {
        sapling_anchor: None,
        orchard_anchor: Some(anchor),
        ironwood_anchor: None,
        orchard_padding: BundlePadding::DEFAULT,
        ironwood_padding: BundlePadding::DEFAULT,
    };
    let fee = Zatoshis::from_u64(fee_zat).map_err(|e| format!("invalid fee: {:?}", e))?;
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

    // Legacy V5 payout proves against the pre-NU6.2 (historical) circuit to match the VK on
    // the branch it targets today.
    static PK: std::sync::OnceLock<orchard::circuit::ProvingKey> = std::sync::OnceLock::new();
    let pk = PK.get_or_init(|| {
        orchard::circuit::ProvingKey::build(orchard::circuit::OrchardCircuitVersion::InsecurePreNu6_2)
    });

    let pczt = Prover::new(pczt_parts)
        .create_orchard_proof(pk)
        .map_err(|e| format!("create_orchard_proof: {:?}", e))?
        .finish();

    let pczt = IoFinalizer::new(pczt)
        .finalize_io()
        .map_err(|e| format!("finalize_io: {:?}", e))?;

    let pczt_bytes = pczt
        .serialize()
        .map_err(|e| format!("pczt serialize: {:?}", e))?;

    let sighash = {
        let reparsed =
            pczt::Pczt::parse(&pczt_bytes).map_err(|e| format!("pczt reparse: {:?}", e))?;
        Signer::new(reparsed)
            .map_err(|e| format!("signer for sighash: {:?}", e))?
            .shielded_sighash()
    };

    Ok(PcztBuildResult { pczt_bytes, sighash, alphas, spend_indices })
}

/// NU6.3 payout: SPEND the escrow's legacy Orchard deposit notes, OUTPUT the winnings as
/// **Ironwood** notes (cross-address is legal in Ironwood), extract a V6 tx. Structurally
/// identical to zcash-wasm's proven turnstile builder, but the outputs go to the winner /
/// house (external OVK) and change returns to the escrow's own internal Ironwood address.
///
/// FROST is preserved: we still extract alphas + spend indices from the ORCHARD bundle
/// spends, and `shielded_sighash()` now returns the V6 sighash the cohort signs.
#[cfg(zcash_unstable = "nu6.3")]
#[allow(clippy::too_many_arguments)]
fn build_pczt_v6_ironwood(
    fvk_bytes: [u8; 96],
    spends: Vec<(Note, MerklePath)>,
    outputs: Vec<(Address, u64)>,
    change: u64,
    anchor: Anchor,
    anchor_height: u32,
    fee_zat: u64,
    mainnet: bool,
) -> Result<PcztBuildResult, String> {
    use orchard::circuit::OrchardCircuitVersion;
    use pczt::roles::creator::Creator;
    use pczt::roles::io_finalizer::IoFinalizer;
    use pczt::roles::prover::Prover;
    use pczt::roles::signer::Signer;
    use zcash_primitives::transaction::builder::{BuildConfig, Builder, BundlePadding};
    use zcash_primitives::transaction::fees::fixed::FeeRule;
    use zcash_primitives::transaction::TxVersion;
    use zcash_protocol::consensus::{BranchId, MainNetwork, TestNetwork};
    use zcash_protocol::memo::MemoBytes;

    let fvk = FullViewingKey::from_bytes(&fvk_bytes)
        .ok_or_else(|| "invalid FVK bytes".to_string())?;
    let ovk_ext: OutgoingViewingKey = fvk.to_ovk(Scope::External);
    let ovk_int: OutgoingViewingKey = fvk.to_ovk(Scope::Internal);

    let fee = Zatoshis::from_u64(fee_zat).map_err(|e| format!("invalid fee: {:?}", e))?;
    let fee_rule = FeeRule::non_standard(fee);
    let target = BlockHeight::from(anchor_height);

    // Output-only ironwood bundle: only ever anchors dummy spends, so the empty-tree anchor
    // is the correct convention (mirrors the turnstile producer). Real ironwood SPENDs are a
    // later step (Ironwood deposit scan).
    let build_config = BuildConfig::Standard {
        sapling_anchor: None,
        orchard_anchor: Some(anchor),
        ironwood_anchor: Some(Anchor::empty_tree()),
    };

    // Build over the Nu63Activated-wrapped params so `is_nu_active(Nu6_3, target)` is true
    // (→ the ironwood builder is created) AND `BranchId::for_height` binds the REAL Nu6_3
    // branch id (→ 0x37a5165b via the vendored consensus patch). The fail-closed guard below
    // refuses to build if the bound id is the placeholder or ≠ NU6_3_BRANCH_ID.
    let nu6_3_from = BlockHeight::from(nu6_3_activation(mainnet));

    // The FeeRule error type the generic `add_*::<FE>` methods want.
    type FeError = <FeeRule as zcash_primitives::transaction::fees::FeeRule>::Error;

    // The V6 build closure, generic over base network params.
    macro_rules! run_v6_builder {
        ($base:expr) => {{
            let params = Nu63Activated { inner: $base, nu6_3_from };

            // FAIL-CLOSED branch-id guard (money path): refuse the placeholder outright, and
            // refuse to build unless the id we would bind at `target` equals the real one.
            let bound: u32 = BranchId::for_height(&params, target).into();
            if bound == NU6_3_PLACEHOLDER_BRANCH_ID {
                return Err(format!(
                    "refusing to build V6 payout: params would bind the NU6.3 placeholder \
                     branch id {:#010x} at height {} — the librustzcash fork is not patched \
                     with the real NU6.3 branch id",
                    NU6_3_PLACEHOLDER_BRANCH_ID, anchor_height
                ));
            }
            if bound != NU6_3_BRANCH_ID {
                return Err(format!(
                    "refusing to build V6 payout: branch id that would bind at height {} is \
                     {:#010x} but the real NU6.3 branch id is {:#010x} (NU6.3 not active at \
                     this height, or a branch-id mismatch)",
                    anchor_height, bound, NU6_3_BRANCH_ID
                ));
            }

            let mut builder = Builder::new(params, target, build_config);
            // Propose V6 up front (mirrors the turnstile builder): this validates ironwood
            // availability against the Nu6_3 branch and re-selects the orchard protocol.
            builder
                .propose_version::<FeError>(TxVersion::V6)
                .map_err(|e| format!("propose_version(V6): {:?}", e))?;

            // SPEND the escrow's legacy Orchard deposit notes (unchanged — still V2 orchard).
            for (note, path) in &spends {
                builder
                    .add_orchard_spend::<FeError>(fvk.clone(), *note, path.clone())
                    .map_err(|e| format!("add_orchard_spend: {:?}", e))?;
            }
            // OUTPUT the winnings as IRONWOOD notes to the winner / house UAs. External OVK
            // so the escrow retains outgoing viewing visibility (parity with the legacy path
            // and what the zafu joiner OVK-verifies).
            for (addr, amount_zat) in &outputs {
                let zat = Zatoshis::from_u64(*amount_zat)
                    .map_err(|e| format!("invalid amount: {:?}", e))?;
                builder
                    .add_ironwood_output::<FeError>(Some(ovk_ext.clone()), *addr, zat, MemoBytes::empty())
                    .map_err(|e| format!("add_ironwood_output: {:?}", e))?;
            }
            // CHANGE returns to the escrow's OWN internal Ironwood address.
            if change > 0 {
                let change_addr = fvk.address_at(0u64, Scope::Internal);
                let change_zat = Zatoshis::from_u64(change)
                    .map_err(|e| format!("invalid change: {:?}", e))?;
                builder
                    .add_ironwood_change_output::<FeError>(
                        fvk.clone(),
                        Some(ovk_int.clone()),
                        change_addr,
                        change_zat,
                        MemoBytes::empty(),
                    )
                    .map_err(|e| format!("add_ironwood_change_output: {:?}", e))?;
            }
            builder
                .build_for_pczt(OsRng, &fee_rule)
                .map_err(|e| format!("build_for_pczt: {:?}", e))?
                .pczt_parts
        }};
    }

    // Alphas + spend indices still come from the ORCHARD bundle spends (the FROST-cosigned
    // actions) — the ironwood bundle is output-only, so it contributes no real spend to sign.
    let (pczt_parts, alphas, spend_indices) = if mainnet {
        let parts = run_v6_builder!(MainNetwork);
        let (a, s) = extract_alphas(&parts.orchard);
        (Creator::build_from_parts(parts).ok_or("Creator::build_from_parts failed (mainnet V6)")?, a, s)
    } else {
        let parts = run_v6_builder!(TestNetwork);
        let (a, s) = extract_alphas(&parts.orchard);
        (Creator::build_from_parts(parts).ok_or("Creator::build_from_parts failed (testnet V6)")?, a, s)
    };

    // Canonical role order (turnstile contract): IoFinalizer binds the sighash, then the
    // Prover attaches BOTH proofs. Both bundles of a V6 tx prove against the PostNu6_3 circuit.
    let pczt = IoFinalizer::new(pczt_parts)
        .finalize_io()
        .map_err(|e| format!("finalize_io: {:?}", e))?;

    static PK: std::sync::OnceLock<orchard::circuit::ProvingKey> = std::sync::OnceLock::new();
    let pk = PK.get_or_init(|| orchard::circuit::ProvingKey::build(OrchardCircuitVersion::PostNu6_3));

    let pczt = Prover::new(pczt)
        .create_orchard_proof(pk)
        .map_err(|e| format!("create_orchard_proof: {:?}", e))?
        .create_ironwood_proof(pk)
        .map_err(|e| format!("create_ironwood_proof: {:?}", e))?
        .finish();

    let pczt_bytes = pczt
        .serialize()
        .map_err(|e| format!("pczt serialize: {:?}", e))?;

    // The V6 sighash the FROST cohort signs. Same call as the legacy path — the underlying
    // sighash algorithm binds the V6 tx version + the real Nu6_3 branch id automatically.
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
        pool: match d.pool {
            crate::scanner::NotePool::Orchard => zecli::wallet::Pool::Orchard,
            crate::scanner::NotePool::Ironwood => zecli::wallet::Pool::Ironwood,
        },
    }
}

fn reconstruct_note(d: &DepositNote) -> Result<Note, String> {
    let recipient = Option::from(Address::from_raw_address_bytes(&d.recipient))
        .ok_or_else(|| "invalid recipient bytes".to_string())?;
    let rho = Option::from(Rho::from_bytes(&d.rho)).ok_or_else(|| "invalid rho".to_string())?;
    let rseed = Option::from(RandomSeed::from_bytes(d.rseed, &rho))
        .ok_or_else(|| "invalid rseed".to_string())?;
    // Reconstruct with the note's OWN pool version (step 3): legacy Orchard deposits are
    // `NoteVersion::V2`; Ironwood-pool deposits + the escrow's own V6-payout change are
    // `NoteVersion::V3`. The scanner carries the pool on `DepositNote`, and the version must
    // match exactly or the recomputed cmx below won't equal the stored one. NOTE: reconstructing
    // a V3 note correctly is necessary but not sufficient to SPEND it — the payout builder must
    // also route V3 notes through `add_ironwood_spend` (V2 → `add_orchard_spend`); see the
    // build_pczt_* spend loops.
    let note: Note = Option::<Note>::from(Note::from_parts(
        recipient,
        NoteValue::from_raw(d.value_zat),
        rho,
        rseed,
        d.pool.note_version(),
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

// ── tests ─────────────────────────────────────────────────────────────────────

/// V6 payout builder test. Mirrors `zcli-ironwood/crates/zcash-wasm/tests/turnstile_v6.rs`:
/// build a payout from a mocked Orchard deposit note + a payout plan, then extract a
/// TxVersion::V6 tx whose consensus branch id is 0x37a5165b and that carries an Ironwood
/// output bundle to the winner. Only meaningful with the nu6.3 cfg:
///   RUSTFLAGS='--cfg zcash_unstable="nu6.3"' cargo test --release
#[cfg(all(test, zcash_unstable = "nu6.3"))]
mod v6_tests {
    use super::*;
    use pczt::roles::low_level_signer;
    use pczt::roles::signer::Signer;
    use zcash_primitives::transaction::{Transaction, TxVersion};
    use zcash_protocol::consensus::{BlockHeight, BranchId, MainNetwork};

    /// Build the V6 payout PCZT directly from an in-memory note+witness (bypassing zidecar),
    /// exercising the exact `build_pczt_v6_ironwood` money path. Returns the proven,
    /// unsigned PCZT bytes plus the alphas/spend_indices from the orchard spends.
    #[allow(clippy::too_many_arguments)]
    fn build_v6_direct(
        fvk: &FullViewingKey,
        note: Note,
        witness: MerklePath,
        winner: Address,
        payout_zat: u64,
        change_addr_unused: bool,
        fee_zat: u64,
        target_height: u32,
    ) -> Result<super::PcztBuildResult, String> {
        let _ = change_addr_unused;
        let fvk_bytes = fvk.to_bytes();
        let cmx: orchard::note::ExtractedNoteCommitment = note.commitment().into();
        let anchor = witness.root(cmx);
        let total_in = note.value().inner();
        let change = total_in - payout_zat - fee_zat;
        super::build_pczt_v6_ironwood(
            fvk_bytes,
            vec![(note, witness)],
            vec![(winner, payout_zat)],
            change,
            anchor,
            target_height,
            fee_zat,
            true, // mainnet params (real 0x37a5165b branch id via Nu63Activated)
        )
    }

    /// A spendable legacy Orchard note (V2) with a single-leaf witness — the escrow's
    /// deposit-note fixture, same shape as the turnstile test.
    fn mock_deposit(fvk: &FullViewingKey, value: u64) -> (Note, MerklePath) {
        let rho = orchard::note::Rho::from_bytes(&[1u8; 32]).unwrap();
        let rseed = (0u8..=255)
            .find_map(|b| Option::from(orchard::note::RandomSeed::from_bytes([b; 32], &rho)))
            .expect("test rseed");
        let note: Note = Option::from(Note::from_parts(
            fvk.address_at(0u32, Scope::External),
            NoteValue::from_raw(value),
            rho,
            rseed,
            orchard::note::NoteVersion::V2,
        ))
        .expect("test note");
        let zero = Option::from(orchard::tree::MerkleHashOrchard::from_bytes(&[0u8; 32]))
            .expect("zero merkle hash");
        let witness = MerklePath::from_parts(0, [zero; 32]);
        (note, witness)
    }

    #[test]
    fn nu6_3_activation_constant_matches_real_branch_id_via_wrapper() {
        // The Nu63Activated wrapper over mainnet params must bind the REAL branch id at the
        // activation height (not the fork's 0xffff_ffff placeholder). This is the guard the
        // V6 builder relies on.
        let params = Nu63Activated {
            inner: MainNetwork,
            nu6_3_from: BlockHeight::from(NU6_3_ACTIVATION_MAINNET),
        };
        let bound: u32 =
            BranchId::for_height(&params, BlockHeight::from(NU6_3_ACTIVATION_MAINNET)).into();
        assert_eq!(bound, NU6_3_BRANCH_ID, "V6 payout must bind the real NU6.3 branch id");
    }

    #[test]
    fn v6_payout_extracts_v6_tx_with_ironwood_output() {
        // -- keys: sk-derived so we also hold the spend authorizing key for signing --
        let sk = orchard::keys::SpendingKey::from_bytes([9u8; 32]).unwrap();
        let ask = orchard::keys::SpendAuthorizingKey::from(&sk);
        let fvk = FullViewingKey::from(&sk);

        let deposit_value = 1_000_000u64;
        let fee = 10_000u64;
        let payout = 600_000u64; // winner gets 600k, change = 390k, fee = 10k

        // Winner is a DIFFERENT address than the escrow's own — a cross-address output,
        // which is exactly what the legacy Orchard pool forbids at NU6.3 and Ironwood allows.
        let winner_sk = orchard::keys::SpendingKey::from_bytes([11u8; 32]).unwrap();
        let winner_fvk = FullViewingKey::from(&winner_sk);
        let winner = winner_fvk.address_at(0u32, Scope::External);
        assert_ne!(
            winner.to_raw_address_bytes(),
            fvk.address_at(0u32, Scope::Internal).to_raw_address_bytes(),
            "winner must differ from the escrow's own change address"
        );

        let (note, witness) = mock_deposit(&fvk, deposit_value);

        // Height at/above mainnet NU6.3 activation → the builder must take the V6 path.
        let target_height = NU6_3_ACTIVATION_MAINNET + 100;
        let built = build_v6_direct(
            &fvk, note, witness, winner, payout, false, fee, target_height,
        )
        .expect("build V6 payout PCZT");

        // At least one real orchard spend to FROST-sign; alphas parallel spend_indices.
        assert_eq!(
            built.alphas.len(),
            built.spend_indices.len(),
            "alphas and spend_indices must be 1:1"
        );
        assert!(built.alphas.len() >= 1, "at least one orchard spend to co-sign");

        // The unsigned PCZT is a V6 tx.
        let pczt = pczt::Pczt::parse(&built.pczt_bytes).expect("PCZT parses");
        assert_eq!(
            *pczt.global().tx_version(),
            zcash_protocol::constants::V6_TX_VERSION,
            "payout PCZT must be V6"
        );
        assert_eq!(
            *pczt.global().consensus_branch_id(),
            NU6_3_BRANCH_ID,
            "payout PCZT must bind the real NU6.3 branch id"
        );
        assert!(
            !pczt.ironwood().actions().is_empty(),
            "payout must carry an ironwood output bundle"
        );

        // -- FROST-signing-shape check: the sighash the builder exposes IS the V6 sighash,
        //    and the orchard spends are signable against it. We drive the low-level Signer the
        //    same way the coordinated cohort does (reconstructed fvk + ask). This proves the
        //    FROST path can produce a valid V6 signature; the real cohort just replaces the
        //    single-key `action.sign` with the aggregated redpallas signature applied at the
        //    same spend_indices via complete_payout_pczt. --
        let sighash_from_result = built.sighash;
        let sighash_recomputed = Signer::new(pczt.clone())
            .expect("signer accepts PCZT")
            .shielded_sighash();
        assert_eq!(
            sighash_from_result, sighash_recomputed,
            "PcztBuildResult.sighash must equal the PCZT's shielded (V6) sighash"
        );

        use rand::rngs::OsRng as SignRng;
        let signed_actions = std::cell::Cell::new(0usize);
        let low = low_level_signer::Signer::new(pczt);
        let low = low
            .sign_orchard_with(
                |_pczt, bundle, _tx_modifiable| -> Result<(), pczt::orchard::BundleParseError> {
                    for action in bundle.actions_mut().iter_mut() {
                        match action.spend().verify_nullifier(Some(&fvk)) {
                            Ok(())
                            | Err(
                                orchard::pczt::VerifyError::MissingRecipient
                                | orchard::pczt::VerifyError::MissingValue
                                | orchard::pczt::VerifyError::MissingRho
                                | orchard::pczt::VerifyError::MissingRandomSeed,
                            ) => {}
                            Err(_) => continue,
                        }
                        if action.sign(sighash_recomputed, &ask, SignRng).is_ok() {
                            signed_actions.set(signed_actions.get() + 1);
                        }
                    }
                    Ok(())
                },
            )
            .expect("low-level orchard signing");
        assert!(
            signed_actions.get() >= 1,
            "no orchard action accepted the ask against the V6 sighash"
        );
        let signed = low.finish();

        // -- extract via the escrow's own completion path (V6 → PostNu6_3 VK + with_ironwood) --
        let tx_bytes = complete_payout_pczt_from_signed(&signed.serialize())
            .expect("extract V6 tx");

        let tx = Transaction::read(&tx_bytes[..], BranchId::Nu6_3).expect("tx parses");
        assert_eq!(tx.version(), TxVersion::V6, "extracted tx must be V6");
        assert!(tx.orchard_bundle().is_some(), "orchard spend bundle present");
        assert!(
            tx.ironwood_bundle().is_some(),
            "ironwood output bundle present (winnings to the winner)"
        );

        // Sanity: the wrapper bound the real branch id all the way through.
        assert_eq!(u32::from(BranchId::Nu6_3), NU6_3_BRANCH_ID);
    }

    /// Thin helper: run the escrow's `complete_payout_pczt` circuit-selection + extraction
    /// on an already-signed PCZT (the test signs directly rather than through the FROST
    /// relay, so we skip apply_orchard_signature and just extract). Exercises the exact V6
    /// VK selection + `.with_ironwood` code in `complete_payout_pczt`.
    fn complete_payout_pczt_from_signed(pczt_bytes: &[u8]) -> Result<Vec<u8>, String> {
        use orchard::circuit::{OrchardCircuitVersion, VerifyingKey};
        use pczt::roles::tx_extractor::TransactionExtractor;

        let pczt = pczt::Pczt::parse(pczt_bytes).map_err(|e| format!("pczt parse: {:?}", e))?;
        let is_v6 = *pczt.global().tx_version() == zcash_protocol::constants::V6_TX_VERSION;
        assert!(is_v6, "test PCZT should be V6");
        let vk = VerifyingKey::build(OrchardCircuitVersion::PostNu6_3);
        let iw_vk = VerifyingKey::build(OrchardCircuitVersion::PostNu6_3);
        let tx = TransactionExtractor::new(pczt)
            .with_orchard(&vk)
            .with_ironwood(&iw_vk)
            .extract()
            .map_err(|e| format!("tx extract: {:?}", e))?;
        let mut out = Vec::new();
        tx.write(&mut out).map_err(|e| format!("tx serialize: {}", e))?;
        Ok(out)
    }

    // ── STEP 4: full 2-of-3 FROST co-sign round-trip over the V6 sighash ──────────
    //
    // Proves the escrow can pay out an EXISTING Orchard (V2) deposit after NU6.3: it runs a
    // REAL redpallas FROST 2-party sign against the V6 Ironwood-payout sighash — the same
    // `frost_spend::orchestrate` primitives the live relay drives via
    // `payout_signing::run_multi_rounds` — and applies the aggregated spend-auth signature back
    // into the V6 PCZT. Offline (no relay, no broadcast): the relay is pure transport for the
    // hex blobs exchanged below, so calling the crypto directly is byte-identical crypto with
    // the network hop removed.

    use frost_spend::orchestrate as fs;

    /// A 2-of-3 escrow FROST group whose group verifying key IS the Orchard spend-validating
    /// key (`ak`). Mirrors `frost_dkg::run_dkg`: a real 3-party DKG (`dkg_part1/2/3`), then a
    /// host-sampled `sk` seeds nk/rivk so `FullViewingKey::from_sk_ak(sk, group_ak)` yields the
    /// group FVK. Any note owned by `group_fvk` is spend-authorized only by t-of-n FROST sigs.
    struct EscrowGroup {
        public_key_package_hex: String,
        /// (key_package_hex, ephemeral_seed_hex) for each of the 3 participants.
        shares: [(String, String); 3],
        /// group Orchard FVK — its `ak` == the FROST group verifying key.
        group_fvk: FullViewingKey,
    }

    /// Run the real interactive DKG in-process (no relay) and derive the group FVK exactly as
    /// the escrow does. Trusted-dealer is NOT used — this is the same `dkg_part1/2/3` dance the
    /// production DKG runs, so the resulting shares co-sign identically.
    fn escrow_2of3_dkg() -> EscrowGroup {
        let r1_a = fs::dkg_part1(3, 2).unwrap();
        let r1_b = fs::dkg_part1(3, 2).unwrap();
        let r1_c = fs::dkg_part1(3, 2).unwrap();
        let bc_a = vec![r1_b.broadcast_hex.clone(), r1_c.broadcast_hex.clone()];
        let bc_b = vec![r1_a.broadcast_hex.clone(), r1_c.broadcast_hex.clone()];
        let bc_c = vec![r1_a.broadcast_hex.clone(), r1_b.broadcast_hex.clone()];
        let r2_a = fs::dkg_part2(&r1_a.secret_hex, &bc_a).unwrap();
        let r2_b = fs::dkg_part2(&r1_b.secret_hex, &bc_b).unwrap();
        let r2_c = fs::dkg_part2(&r1_c.secret_hex, &bc_c).unwrap();
        let all_r2: Vec<String> = r2_a
            .peer_packages
            .iter()
            .chain(r2_b.peer_packages.iter())
            .chain(r2_c.peer_packages.iter())
            .cloned()
            .collect();
        let r3_a = fs::dkg_part3(&r2_a.secret_hex, &bc_a, &all_r2).unwrap();
        let r3_b = fs::dkg_part3(&r2_b.secret_hex, &bc_b, &all_r2).unwrap();
        let r3_c = fs::dkg_part3(&r2_c.secret_hex, &bc_c, &all_r2).unwrap();
        let pkg = r3_a.public_key_package_hex.clone();
        assert_eq!(pkg, r3_b.public_key_package_hex, "all parties share one group key");
        assert_eq!(pkg, r3_c.public_key_package_hex);

        // Host samples the FVK seed sk (Pallas-scalar valid), exactly like run_dkg.
        let sk_bytes = loop {
            use rand::RngCore;
            let mut b = [0u8; 32];
            rand::thread_rng().fill_bytes(&mut b);
            if bool::from(pasta_curves::pallas::Scalar::from_repr(b).is_some()) {
                break b;
            }
        };
        let fvk_bytes = crate::orchard_ua::fvk_bytes_from_sk(&pkg, sk_bytes)
            .expect("derive group FVK from (pkg, sk)");
        let group_fvk = FullViewingKey::from_bytes(&fvk_bytes).expect("group FVK parses");

        EscrowGroup {
            public_key_package_hex: pkg,
            shares: [
                (r3_a.key_package_hex, r3_a.ephemeral_seed_hex),
                (r3_b.key_package_hex, r3_b.ephemeral_seed_hex),
                (r3_c.key_package_hex, r3_c.ephemeral_seed_hex),
            ],
            group_fvk,
        }
    }

    /// A legacy Orchard (V2) deposit note owned by `group_fvk`'s external address — the mocked
    /// EXISTING deposit. Its `rk` under any alpha is `group_ak.randomize(alpha)`, so only the
    /// FROST cohort can authorize spending it. Single-leaf witness (root == the note's cmx).
    fn mock_group_deposit(group_fvk: &FullViewingKey, value: u64) -> (Note, MerklePath) {
        let rho = orchard::note::Rho::from_bytes(&[7u8; 32]).unwrap();
        let rseed = (0u8..=255)
            .find_map(|b| Option::from(orchard::note::RandomSeed::from_bytes([b; 32], &rho)))
            .expect("test rseed");
        let note: Note = Option::from(Note::from_parts(
            group_fvk.address_at(0u32, Scope::External),
            NoteValue::from_raw(value),
            rho,
            rseed,
            orchard::note::NoteVersion::V2, // EXISTING legacy Orchard deposit
        ))
        .expect("group-owned deposit note");
        let zero = Option::from(orchard::tree::MerkleHashOrchard::from_bytes(&[0u8; 32]))
            .expect("zero merkle hash");
        let witness = MerklePath::from_parts(0, [zero; 32]);
        (note, witness)
    }

    /// Real in-process 2-party FROST round-trip for ONE Orchard spend. Byte-for-byte the crypto
    /// `payout_signing::run_multi_rounds` runs, minus the relay transport: host + joiner each
    /// commit (round 1), then each signs a share bound to (sighash, alpha) (round 2), then the
    /// host aggregates → a 64-byte redpallas SpendAuth signature. Uses shares 0 (host) + 1
    /// (joiner) — a 2-of-3 quorum. Returns the aggregated signature both parties converge on.
    fn frost_cosign_one(
        group: &EscrowGroup,
        host_idx: usize,
        joiner_idx: usize,
        sighash: &[u8; 32],
        alpha: &[u8; 32],
    ) -> [u8; 64] {
        let (host_kp, host_seed) = &group.shares[host_idx];
        let (join_kp, join_seed) = &group.shares[joiner_idx];
        let host_seed = decode_seed_hex(host_seed);
        let join_seed = decode_seed_hex(join_seed);

        // round 1 — one fresh nonce/commitment each (over the relay these are the `C:` msgs)
        let (host_nonces, host_commit) = fs::sign_round1(&host_seed, host_kp).expect("host r1");
        let (join_nonces, join_commit) = fs::sign_round1(&join_seed, join_kp).expect("joiner r1");
        // both sides assemble the SAME commitment set (order-independent: FROST maps by identity)
        let all_commits = vec![host_commit.clone(), join_commit.clone()];

        // round 2 — each produces an authenticated share bound to (sighash, alpha) (`S:` msgs)
        let host_share = fs::spend_sign_round2_signed(
            &host_seed, host_kp, &host_nonces, sighash, alpha, &all_commits,
        )
        .expect("host r2 share");
        let join_share = fs::spend_sign_round2_signed(
            &join_seed, join_kp, &join_nonces, sighash, alpha, &all_commits,
        )
        .expect("joiner r2 share");
        let all_shares = vec![host_share.clone(), join_share.clone()];

        // aggregate — host and joiner independently reach the identical 64-byte SpendAuth sig
        let host_sig = fs::spend_aggregate(
            &group.public_key_package_hex, sighash, alpha, &all_commits, &all_shares,
        )
        .expect("host aggregate");
        let join_sig = fs::spend_aggregate(
            &group.public_key_package_hex, sighash, alpha, &all_commits, &all_shares,
        )
        .expect("joiner aggregate");
        assert_eq!(
            host_sig, join_sig,
            "host and joiner must converge on the same aggregated SpendAuth signature"
        );
        decode_sig_64_hex(&host_sig)
    }

    fn decode_seed_hex(h: &str) -> [u8; 32] {
        let v = hex::decode(h.trim()).expect("seed hex");
        let mut a = [0u8; 32];
        a.copy_from_slice(&v);
        a
    }

    fn decode_sig_64_hex(h: &str) -> [u8; 64] {
        let v = hex::decode(h.trim()).expect("sig hex");
        assert_eq!(v.len(), 64, "SpendAuth sig must be 64 bytes");
        let mut a = [0u8; 64];
        a.copy_from_slice(&v);
        a
    }

    /// STEP 4 END-TO-END: build a V6 Ironwood payout that SPENDS a mocked EXISTING Orchard (V2)
    /// deposit controlled by a real 2-of-3 FROST group, run the REAL FROST co-sign round-trip
    /// over the V6 sighash, apply the aggregated signature(s), and assert the extracted tx is a
    /// structurally-valid V6 payout whose orchard spend authorization verifies.
    #[test]
    fn frost_2of3_cosigns_v6_ironwood_payout_end_to_end() {
        // 1) escrow 2-of-3 FROST group controlling the Orchard deposit.
        let group = escrow_2of3_dkg();

        // The mocked EXISTING legacy-Orchard deposit, owned by the group key.
        let deposit_value = 1_000_000u64;
        let (note, witness) = mock_group_deposit(&group.group_fvk, deposit_value);

        // Payout plan: winner + house + change. (change is auto-derived by the builder.)
        let fee = 10_000u64;
        let winner_amt = 600_000u64;
        let house_amt = 90_000u64;
        let change = deposit_value - winner_amt - house_amt - fee; // 300_000

        // Winner + house are DIFFERENT UAs than the escrow's own — cross-address outputs, exactly
        // what the sealed legacy Orchard pool forbids at NU6.3 and Ironwood permits.
        let winner_fvk = FullViewingKey::from(&orchard::keys::SpendingKey::from_bytes([21u8; 32]).unwrap());
        let house_fvk = FullViewingKey::from(&orchard::keys::SpendingKey::from_bytes([22u8; 32]).unwrap());
        let winner = winner_fvk.address_at(0u32, Scope::External);
        let house = house_fvk.address_at(0u32, Scope::External);

        // 2) build the V6 payout at a height ≥ NU6.3 activation → forces the Ironwood path.
        let target_height = NU6_3_ACTIVATION_MAINNET + 500;
        assert!(target_height >= 3_428_143);
        let cmx: orchard::note::ExtractedNoteCommitment = note.commitment().into();
        let anchor = witness.root(cmx);
        let built = super::build_pczt_v6_ironwood(
            group.group_fvk.to_bytes(),
            vec![(note, witness)],
            vec![(winner, winner_amt), (house, house_amt)],
            change,
            anchor,
            target_height,
            fee,
            true, // mainnet params → real 0x37a5165b branch id via Nu63Activated
        )
        .expect("build V6 ironwood payout PCZT");

        // The build is a V6 tx binding the real NU6.3 branch id, with real orchard spend(s).
        let pczt = pczt::Pczt::parse(&built.pczt_bytes).expect("PCZT parses");
        assert_eq!(
            *pczt.global().tx_version(),
            zcash_protocol::constants::V6_TX_VERSION,
            "payout PCZT must be V6"
        );
        assert_eq!(
            *pczt.global().consensus_branch_id(),
            NU6_3_BRANCH_ID,
            "payout PCZT must bind the real NU6.3 branch id 0x37a5165b"
        );
        assert!(
            !pczt.ironwood().actions().is_empty(),
            "payout carries the ironwood output bundle (winnings to winner/house)"
        );
        assert_eq!(
            built.alphas.len(),
            built.spend_indices.len(),
            "alphas and spend_indices are 1:1"
        );
        assert!(built.alphas.len() >= 1, "at least one orchard spend to FROST-cosign");

        // 3) EXTRACT the V6 sighash the cohort signs; confirm it equals the PCZT's own sighash.
        let sighash = built.sighash;
        let sighash_check = Signer::new(pczt.clone())
            .expect("signer accepts V6 PCZT")
            .shielded_sighash();
        assert_eq!(sighash, sighash_check, "exposed sighash == the V6 shielded sighash");

        // Run the REAL 2-of-3 FROST round-trip per orchard spend, over the V6 sighash.
        let mut sigs: Vec<[u8; 64]> = Vec::with_capacity(built.alphas.len());
        for alpha in &built.alphas {
            sigs.push(frost_cosign_one(&group, 0, 1, &sighash, alpha));
        }

        // 4) APPLY the aggregated signatures via the escrow's own completion path.
        //
        // AUTHORITATIVE VERIFICATION: `complete_payout_pczt` → `apply_orchard_signature` →
        // `orchard::pczt::Action::apply_signature`, which runs `self.spend.rk.verify(sighash,
        // sig)` and returns `InvalidExternalSignature` on ANY mismatch (sighash personalization,
        // branch-id binding, or rk derivation). rk here is the spend's own rk = fvk.ak + [alpha]G
        // and fvk.ak == the FROST group verifying key, so a successful extract means the
        // FROST-aggregated signature is a redpallas-VERIFIED spend authorization for the V6
        // sighash — not merely a structural pass. This is the fork's own verification path and
        // is exactly the code the live relay drives post-signing.
        let tx_bytes = complete_payout_pczt(&built.pczt_bytes, &sigs, &built.spend_indices)
            .expect("apply FROST sigs + extract V6 tx (apply_orchard_signature verifies each sig)");

        // ASSERT the extracted tx is a structurally-valid V6 payout.
        let tx = Transaction::read(&tx_bytes[..], BranchId::Nu6_3).expect("extracted tx parses");
        assert_eq!(tx.version(), TxVersion::V6, "extracted tx is V6");
        assert!(
            tx.orchard_bundle().is_some(),
            "orchard SPEND bundle present (the FROST-cosigned deposit spend)"
        );
        assert!(
            tx.ironwood_bundle().is_some(),
            "ironwood OUTPUT bundle present (the winnings to winner/house)"
        );
        assert_eq!(u32::from(BranchId::Nu6_3), NU6_3_BRANCH_ID, "branch id wired through");

        // NON-VACUITY GUARD: prove the acceptance above is a REAL redpallas verification and not
        // a no-op. Feed a corrupted signature at the same spend index straight into the escrow's
        // `apply_orchard_signature`; it must be REJECTED (InvalidExternalSignature) because the
        // corrupted bytes don't verify under the spend's rk over the V6 sighash. A green accept
        // for the true FROST sig + a hard reject for a corrupted one == the signature genuinely
        // authorizes this exact V6 sighash.
        {
            use pczt::roles::signer::Signer as PcztSigner;
            let mut bad = sigs[0];
            bad[0] ^= 0xff; // flip a byte → no longer a valid sig for (rk, sighash)
            let pczt_bad = pczt::Pczt::parse(&built.pczt_bytes).expect("reparse for reject check");
            let mut signer = PcztSigner::new(pczt_bad).expect("signer for reject check");
            let bad_sig = orchard::primitives::redpallas::Signature::<
                orchard::primitives::redpallas::SpendAuth,
            >::from(bad);
            let res = signer.apply_orchard_signature(built.spend_indices[0], bad_sig);
            assert!(
                res.is_err(),
                "apply_orchard_signature must REJECT a corrupted signature — proves the accept \
                 of the real FROST sig was a genuine rk/sighash verification, not vacuous"
            );
        }
    }
}
