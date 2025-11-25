//! local scanning with incoming viewing key (ivk)

use anyhow::Result;
use tracing::{info, debug};
use orchard::{
    keys::IncomingViewingKey,
    note_encryption::OrchardDomain,
    note::RandomSeed,
};
use zcash_note_encryption::try_compact_note_decryption;

pub struct WalletScanner {
    // ivk for trial decryption
    ivk: Option<IncomingViewingKey>,

    // scanned notes
    owned_notes: Vec<ScannedNote>,

    // nullifier set (spent notes)
    spent_nullifiers: std::collections::HashSet<[u8; 32]>,
}

#[derive(Debug, Clone)]
pub struct ScannedNote {
    pub commitment: [u8; 32],
    pub nullifier: [u8; 32],
    pub value: u64,
    pub rseed: RandomSeed,
    pub memo: Vec<u8>,
    pub block_height: u32,
    pub is_spent: bool,
}

impl WalletScanner {
    pub fn new(ivk: Option<IncomingViewingKey>) -> Self {
        Self {
            ivk,
            owned_notes: Vec::new(),
            spent_nullifiers: std::collections::HashSet::new(),
        }
    }

    /// scan compact block for owned notes
    pub fn scan_block(&mut self, block: &crate::zidecar::CompactBlock) -> Result<usize> {
        let mut found = 0;

        for action in &block.actions {
            // trial decrypt with ivk
            if let Some(note) = self.try_decrypt(action, block.height) {
                self.owned_notes.push(note);
                found += 1;
            }

            // check if any of our notes were spent
            let nf_array: [u8; 32] = action.nullifier.as_slice().try_into()
                .unwrap_or([0u8; 32]);
            if self.spent_nullifiers.contains(&nf_array) {
                // mark note as spent
                self.mark_spent(&nf_array);
                debug!("note spent at height {}", block.height);
            }
        }

        if found > 0 {
            info!("found {} notes in block {}", found, block.height);
        }

        Ok(found)
    }

    /// try to decrypt action with ivk
    fn try_decrypt(
        &self,
        _action: &crate::zidecar::CompactAction,
        _block_height: u32,
    ) -> Option<ScannedNote> {
        // TODO: implement actual orchard trial decryption
        //
        // The orchard 0.7 API requires PreparedIncomingViewingKey and has changed
        // the compact note decryption interface. Need to:
        //
        // 1. Convert IncomingViewingKey to PreparedIncomingViewingKey
        // 2. Parse ephemeral_key, cmx, and nullifier properly using .into_option()?
        // 3. Use try_note_decryption with proper domain construction
        // 4. Handle the returned Note type correctly
        //
        // For now, returning None until full Orchard integration is needed.
        // The infrastructure is in place and ready for real implementation.

        None
    }

    /// mark note as spent
    fn mark_spent(&mut self, nullifier: &[u8; 32]) {
        for note in &mut self.owned_notes {
            if &note.nullifier == nullifier {
                note.is_spent = true;
                debug!("marked note {} as spent", hex::encode(&note.commitment));
            }
        }
    }

    /// get total balance (unspent notes)
    pub fn balance(&self) -> u64 {
        self.owned_notes
            .iter()
            .filter(|n| !n.is_spent)
            .map(|n| n.value)
            .sum()
    }

    /// get all notes (for history view)
    pub fn notes(&self) -> &[ScannedNote] {
        &self.owned_notes
    }
}
