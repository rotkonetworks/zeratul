//! chain scanner for shielded syndicates
//!
//! scans compact blocks using the syndicate's full viewing key (FVK)
//! to discover notes owned by the syndicate.
//!
//! # architecture
//!
//! ```text
//! ┌─────────────────┐     compact blocks     ┌─────────────────┐
//! │ Offchain Worker │ ────────────────────▶  │    Scanner      │
//! │ (zidecar/grpc)  │                        │                 │
//! └─────────────────┘                        │  trial decrypt  │
//!                                            │  with FVK       │
//!                                            └────────┬────────┘
//!                                                     │
//!                                                     ▼
//!                                            ┌─────────────────┐
//!                                            │ SyndicateWallet │
//!                                            │  - notes        │
//!                                            │  - nullifiers   │
//!                                            │  - witnesses    │
//!                                            └─────────────────┘
//! ```
//!
//! # scanning modes
//!
//! - **full scan**: scan from genesis/activation height (initial sync)
//! - **incremental**: scan from last known height (ongoing)
//! - **targeted**: scan specific block range (recovery)
//!
//! # feature flags
//!
//! - `zcash`: enables orchard trial decryption via zync-core

use alloc::collections::BTreeSet;
use alloc::string::String;
use alloc::vec;
use alloc::vec::Vec;

// re-export zync-core scanner when zcash feature enabled
#[cfg(feature = "zcash")]
pub use zync_core::{
    Scanner as ZcashScanner,
    ScanAction as ZcashScanAction,
    DecryptedNote as ZcashDecryptedNote,
    BatchScanner as ZcashBatchScanner,
    OrchardFvk,
};

use crate::wallet::{SyndicateWallet, ShieldedNote, NoteWitness, ShieldedChain};
use crate::wire::Hash32;

/// compact block for scanning (chain-agnostic)
#[derive(Clone, Debug)]
pub struct CompactBlock {
    /// block height
    pub height: u64,
    /// block hash
    pub hash: Hash32,
    /// compact outputs (encrypted notes)
    pub outputs: Vec<CompactOutput>,
    /// nullifiers (spent notes)
    pub nullifiers: Vec<Hash32>,
    /// commitment tree updates
    pub tree_updates: Option<TreeUpdate>,
}

/// compact output (encrypted note data)
#[derive(Clone, Debug)]
pub struct CompactOutput {
    /// note commitment
    pub commitment: Hash32,
    /// ephemeral public key (for ECDH)
    pub ephemeral_key: [u8; 32],
    /// encrypted ciphertext (compact form)
    pub ciphertext: Vec<u8>,
    /// position in commitment tree
    pub position: u64,
}

/// commitment tree update
#[derive(Clone, Debug)]
pub struct TreeUpdate {
    /// new commitments added
    pub commitments: Vec<Hash32>,
    /// merkle frontier (for witness updates)
    pub frontier: Vec<Hash32>,
    /// tree root after update
    pub root: Hash32,
}

/// decrypted note (after successful trial decryption)
#[derive(Clone, Debug)]
pub struct DecryptedNote {
    /// note commitment
    pub commitment: Hash32,
    /// nullifier (derived from note + nullifier key)
    pub nullifier: Hash32,
    /// value/amount
    pub amount: u128,
    /// asset id
    pub asset: Hash32,
    /// recipient address index (for multi-address wallets)
    pub address_index: u32,
    /// memo (if present)
    pub memo: Option<Vec<u8>>,
    /// position in tree
    pub position: u64,
    /// rseed (for re-deriving note fields)
    pub rseed: [u8; 32],
}

/// full viewing key (chain-specific, but common interface)
#[derive(Clone)]
pub struct FullViewingKey {
    /// chain type
    pub chain: ShieldedChain,
    /// key bytes (format depends on chain)
    /// penumbra: 64 bytes (ak || nk)
    /// zcash orchard: 96 bytes (ak || nk || rivk)
    pub key_bytes: Vec<u8>,
}

impl FullViewingKey {
    /// create penumbra FVK
    pub fn penumbra(key_bytes: Vec<u8>) -> Self {
        Self {
            chain: ShieldedChain::Penumbra,
            key_bytes,
        }
    }

    /// create zcash orchard FVK
    pub fn zcash_orchard(key_bytes: Vec<u8>) -> Self {
        Self {
            chain: ShieldedChain::ZcashOrchard,
            key_bytes,
        }
    }

    /// try to decrypt a compact output
    /// returns Some(DecryptedNote) if this output belongs to us
    pub fn try_decrypt(&self, output: &CompactOutput) -> Option<DecryptedNote> {
        // chain-specific trial decryption
        match self.chain {
            ShieldedChain::Penumbra => self.try_decrypt_penumbra(output),
            ShieldedChain::ZcashOrchard => self.try_decrypt_orchard(output),
            ShieldedChain::ZcashSapling => self.try_decrypt_sapling(output),
        }
    }

    fn try_decrypt_penumbra(&self, output: &CompactOutput) -> Option<DecryptedNote> {
        // TODO: actual penumbra decryption
        // 1. ECDH with ephemeral_key and ivk
        // 2. derive shared secret
        // 3. decrypt ciphertext
        // 4. parse note plaintext
        // 5. derive nullifier
        let _ = output;
        None
    }

    #[cfg(feature = "zcash")]
    fn try_decrypt_orchard(&self, output: &CompactOutput) -> Option<DecryptedNote> {
        // parse FVK bytes to orchard FullViewingKey
        // orchard FVK is 96 bytes: ak(32) || nk(32) || rivk(32)
        if self.key_bytes.len() != 96 {
            return None;
        }

        let fvk_bytes: [u8; 96] = self.key_bytes.as_slice().try_into().ok()?;
        let orchard_fvk = OrchardFvk::from_bytes(&fvk_bytes);
        let orchard_fvk = Option::from(orchard_fvk)?;

        // convert CompactOutput to zync-core ScanAction
        let scan_action = ZcashScanAction {
            nullifier: [0u8; 32], // compact outputs don't include nullifier
            cmx: output.commitment,
            ephemeral_key: output.ephemeral_key,
            enc_ciphertext: output.ciphertext.clone(),
            block_height: 0, // filled by caller
            position_in_block: output.position as u32,
        };

        // use zync-core scanner for trial decryption
        let scanner = ZcashScanner::from_fvk(&orchard_fvk);
        let zync_note = scanner.try_decrypt(&scan_action)?;

        // convert to narsil DecryptedNote
        Some(DecryptedNote {
            commitment: output.commitment,
            nullifier: zync_note.nullifier,
            amount: zync_note.value as u128,
            asset: [0u8; 32], // orchard uses ZEC native asset
            address_index: 0, // would need to derive from address
            memo: None, // compact decryption doesn't include memo
            position: output.position,
            rseed: [0u8; 32], // would need to extract from note
        })
    }

    #[cfg(not(feature = "zcash"))]
    fn try_decrypt_orchard(&self, output: &CompactOutput) -> Option<DecryptedNote> {
        // orchard decryption requires zcash feature
        let _ = output;
        None
    }

    fn try_decrypt_sapling(&self, output: &CompactOutput) -> Option<DecryptedNote> {
        // TODO: actual sapling decryption
        let _ = output;
        None
    }

    /// derive nullifier for a note
    pub fn derive_nullifier(&self, note: &DecryptedNote) -> Hash32 {
        // chain-specific nullifier derivation
        // uses nullifier key component of FVK
        let _ = note;
        [0u8; 32] // TODO
    }
}

/// scanner configuration
#[derive(Clone, Debug)]
pub struct ScannerConfig {
    /// chain to scan
    pub chain: ShieldedChain,
    /// start height (activation height for the pool)
    pub start_height: u64,
    /// batch size for fetching blocks
    pub batch_size: u32,
    /// parallel decryption threads (if std)
    pub parallel: bool,
}

impl ScannerConfig {
    /// config for penumbra
    pub fn penumbra() -> Self {
        Self {
            chain: ShieldedChain::Penumbra,
            start_height: 1, // penumbra genesis
            batch_size: 1000,
            parallel: true,
        }
    }

    /// config for zcash orchard
    pub fn zcash_orchard() -> Self {
        Self {
            chain: ShieldedChain::ZcashOrchard,
            start_height: 1_687_104, // orchard activation on mainnet
            batch_size: 1000,
            parallel: true,
        }
    }

    /// config for zcash testnet
    pub fn zcash_testnet() -> Self {
        Self {
            chain: ShieldedChain::ZcashOrchard,
            start_height: 1_842_420, // orchard activation on testnet
            batch_size: 1000,
            parallel: true,
        }
    }
}

/// scan result for a block range
#[derive(Clone, Debug, Default)]
pub struct ScanResult {
    /// newly discovered notes
    pub new_notes: Vec<DecryptedNote>,
    /// nullifiers that spent our notes
    pub spent_nullifiers: Vec<Hash32>,
    /// highest scanned height
    pub scan_height: u64,
    /// blocks scanned
    pub blocks_scanned: u64,
    /// outputs checked
    pub outputs_checked: u64,
}

impl ScanResult {
    /// merge another result into this one
    pub fn merge(&mut self, other: ScanResult) {
        self.new_notes.extend(other.new_notes);
        self.spent_nullifiers.extend(other.spent_nullifiers);
        self.scan_height = self.scan_height.max(other.scan_height);
        self.blocks_scanned += other.blocks_scanned;
        self.outputs_checked += other.outputs_checked;
    }
}

/// chain scanner
pub struct Scanner {
    /// full viewing key
    fvk: FullViewingKey,
    /// configuration
    config: ScannerConfig,
    /// known nullifiers (to detect spends)
    known_nullifiers: BTreeSet<Hash32>,
}

impl Scanner {
    /// create scanner with FVK
    pub fn new(fvk: FullViewingKey, config: ScannerConfig) -> Self {
        Self {
            fvk,
            config,
            known_nullifiers: BTreeSet::new(),
        }
    }

    /// register known nullifiers (from existing notes)
    pub fn register_nullifiers(&mut self, nullifiers: impl IntoIterator<Item = Hash32>) {
        self.known_nullifiers.extend(nullifiers);
    }

    /// scan a batch of compact blocks
    pub fn scan_blocks(&mut self, blocks: &[CompactBlock]) -> ScanResult {
        let mut result = ScanResult::default();

        for block in blocks {
            result.blocks_scanned += 1;
            result.scan_height = result.scan_height.max(block.height);

            // check for spent notes
            for nullifier in &block.nullifiers {
                if self.known_nullifiers.contains(nullifier) {
                    result.spent_nullifiers.push(*nullifier);
                }
            }

            // trial decrypt outputs
            for output in &block.outputs {
                result.outputs_checked += 1;

                if let Some(note) = self.fvk.try_decrypt(output) {
                    // register nullifier for future spend detection
                    self.known_nullifiers.insert(note.nullifier);
                    result.new_notes.push(note);
                }
            }
        }

        result
    }

    /// apply scan result to wallet
    pub fn apply_to_wallet(&self, wallet: &mut SyndicateWallet, result: &ScanResult) {
        // add new notes
        for note in &result.new_notes {
            wallet.add_note(ShieldedNote {
                commitment: note.commitment,
                nullifier: note.nullifier,
                amount: note.amount,
                asset: note.asset,
                position: note.position,
                height: result.scan_height,
                memo: note.memo.clone(),
                spent: false,
            });
        }

        // mark spent notes
        for nullifier in &result.spent_nullifiers {
            wallet.spend_note(nullifier);
        }

        // update scan height
        wallet.update_scan_height(result.scan_height);
    }

    /// get start height for scanning
    pub fn start_height(&self) -> u64 {
        self.config.start_height
    }

    /// get batch size
    pub fn batch_size(&self) -> u32 {
        self.config.batch_size
    }
}

/// witness builder for creating spends
pub struct WitnessBuilder {
    /// commitment tree frontier
    frontier: Vec<Hash32>,
    /// tree depth
    depth: u8,
    /// current position
    position: u64,
}

impl WitnessBuilder {
    /// create for penumbra (depth 24)
    pub fn penumbra() -> Self {
        Self {
            frontier: Vec::new(),
            depth: 24,
            position: 0,
        }
    }

    /// create for zcash orchard (depth 32)
    pub fn orchard() -> Self {
        Self {
            frontier: Vec::new(),
            depth: 32,
            position: 0,
        }
    }

    /// update with tree changes
    pub fn update(&mut self, tree_update: &TreeUpdate) {
        self.frontier = tree_update.frontier.clone();
        self.position += tree_update.commitments.len() as u64;
    }

    /// build witness for a note at given position
    pub fn build_witness(&self, note_position: u64, anchor: Hash32) -> Option<NoteWitness> {
        if note_position >= self.position {
            return None; // note not yet in tree
        }

        // TODO: actual merkle path computation
        // requires storing intermediate nodes or recomputing from frontier

        Some(NoteWitness {
            commitment: [0u8; 32], // would be filled by caller
            auth_path: vec![[0u8; 32]; self.depth as usize],
            position: note_position,
            anchor,
        })
    }

    /// get current tree root
    pub fn root(&self) -> Hash32 {
        // TODO: compute from frontier
        [0u8; 32]
    }
}

/// sync state for a syndicate wallet
#[derive(Clone, Debug)]
pub struct SyncState {
    /// chain being synced
    pub chain: ShieldedChain,
    /// current sync height
    pub height: u64,
    /// target height (chain tip)
    pub target: u64,
    /// sync status
    pub status: SyncStatus,
    /// last error (if any)
    pub last_error: Option<String>,
}

/// sync status
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SyncStatus {
    /// not started
    Idle,
    /// fetching blocks
    Fetching,
    /// scanning blocks
    Scanning,
    /// updating witnesses
    UpdatingWitnesses,
    /// caught up
    Synced,
    /// error occurred
    Error,
}

impl SyncState {
    /// create new sync state
    pub fn new(chain: ShieldedChain, start_height: u64) -> Self {
        Self {
            chain,
            height: start_height,
            target: start_height,
            status: SyncStatus::Idle,
            last_error: None,
        }
    }

    /// progress percentage
    pub fn progress(&self) -> f32 {
        if self.target <= self.height {
            100.0
        } else {
            (self.height as f32 / self.target as f32) * 100.0
        }
    }

    /// blocks remaining
    pub fn remaining(&self) -> u64 {
        self.target.saturating_sub(self.height)
    }

    /// is fully synced?
    pub fn is_synced(&self) -> bool {
        self.status == SyncStatus::Synced && self.height >= self.target
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_compact_block(height: u64, num_outputs: usize, num_nullifiers: usize) -> CompactBlock {
        CompactBlock {
            height,
            hash: [height as u8; 32],
            outputs: (0..num_outputs)
                .map(|i| CompactOutput {
                    commitment: [(height as u8).wrapping_add(i as u8); 32],
                    ephemeral_key: [0u8; 32],
                    ciphertext: vec![0u8; 52],
                    position: height * 100 + i as u64,
                })
                .collect(),
            nullifiers: (0..num_nullifiers)
                .map(|i| [200u8.wrapping_add(i as u8); 32])
                .collect(),
            tree_updates: None,
        }
    }

    #[test]
    fn test_scanner_config() {
        let penumbra = ScannerConfig::penumbra();
        assert_eq!(penumbra.chain, ShieldedChain::Penumbra);
        assert_eq!(penumbra.start_height, 1);

        let zcash = ScannerConfig::zcash_orchard();
        assert_eq!(zcash.chain, ShieldedChain::ZcashOrchard);
        assert_eq!(zcash.start_height, 1_687_104);
    }

    #[test]
    fn test_scan_result_merge() {
        let mut r1 = ScanResult {
            new_notes: vec![],
            spent_nullifiers: vec![[1u8; 32]],
            scan_height: 100,
            blocks_scanned: 10,
            outputs_checked: 50,
        };

        let r2 = ScanResult {
            new_notes: vec![],
            spent_nullifiers: vec![[2u8; 32]],
            scan_height: 200,
            blocks_scanned: 10,
            outputs_checked: 60,
        };

        r1.merge(r2);
        assert_eq!(r1.spent_nullifiers.len(), 2);
        assert_eq!(r1.scan_height, 200);
        assert_eq!(r1.blocks_scanned, 20);
        assert_eq!(r1.outputs_checked, 110);
    }

    #[test]
    fn test_scanner_nullifier_detection() {
        let fvk = FullViewingKey::penumbra(vec![0u8; 64]);
        let config = ScannerConfig::penumbra();
        let mut scanner = Scanner::new(fvk, config);

        // register a known nullifier
        scanner.register_nullifiers([[200u8; 32]]);

        // scan block with that nullifier
        let block = make_compact_block(100, 0, 1);
        let result = scanner.scan_blocks(&[block]);

        assert_eq!(result.spent_nullifiers.len(), 1);
        assert_eq!(result.spent_nullifiers[0], [200u8; 32]);
    }

    #[test]
    fn test_sync_state() {
        let mut state = SyncState::new(ShieldedChain::Penumbra, 0);
        state.target = 1000;
        state.height = 500;

        assert_eq!(state.progress(), 50.0);
        assert_eq!(state.remaining(), 500);
        assert!(!state.is_synced());

        state.height = 1000;
        state.status = SyncStatus::Synced;
        assert!(state.is_synced());
    }

    #[test]
    fn test_witness_builder() {
        let builder = WitnessBuilder::orchard();
        assert_eq!(builder.depth, 32);

        let builder = WitnessBuilder::penumbra();
        assert_eq!(builder.depth, 24);
    }
}
