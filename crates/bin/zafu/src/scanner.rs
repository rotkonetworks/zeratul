//! local scanning with incoming viewing key (ivk)

use anyhow::Result;
use tracing::{info, debug};
use orchard::{
    keys::{IncomingViewingKey, FullViewingKey},
    note::{Note, RandomSeed},
    tree::MerklePath,
};

use crate::tx_builder::SpendableNote;

pub struct WalletScanner {
    // ivk for trial decryption
    ivk: Option<IncomingViewingKey>,

    // full viewing key for spending
    fvk: Option<FullViewingKey>,

    // scanned notes
    owned_notes: Vec<ScannedNote>,

    // full orchard notes (needed for spending)
    spendable_notes: Vec<SpendableNoteEntry>,

    // nullifier set (spent notes)
    spent_nullifiers: std::collections::HashSet<[u8; 32]>,
}

/// lightweight scanned note info for display
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

/// full note entry for spending (includes note and merkle path)
pub struct SpendableNoteEntry {
    pub note: Note,
    pub merkle_path: Option<MerklePath>,
    pub commitment: [u8; 32],
    pub nullifier: [u8; 32],
    pub block_height: u32,
    pub is_spent: bool,
}

impl WalletScanner {
    pub fn new(ivk: Option<IncomingViewingKey>) -> Self {
        Self {
            ivk,
            fvk: None,
            owned_notes: Vec::new(),
            spendable_notes: Vec::new(),
            spent_nullifiers: std::collections::HashSet::new(),
        }
    }

    /// set full viewing key for spending
    pub fn set_fvk(&mut self, fvk: FullViewingKey) {
        self.fvk = Some(fvk);
    }

    /// take spendable notes for building transaction (consumes them)
    /// notes are marked as spent after taking
    pub fn take_spendable_notes(&mut self, amount: u64) -> Vec<SpendableNote> {
        let fvk = match &self.fvk {
            Some(f) => f.clone(),
            None => return Vec::new(),
        };

        let mut result = Vec::new();
        let mut total = 0u64;

        // find indices of notes we'll use
        let mut indices_to_take = Vec::new();
        for (i, entry) in self.spendable_notes.iter().enumerate() {
            if !entry.is_spent && entry.merkle_path.is_some() {
                indices_to_take.push(i);
                total += entry.note.value().inner();
                if total >= amount {
                    break;
                }
            }
        }

        // take notes in reverse order to preserve indices
        for i in indices_to_take.into_iter().rev() {
            let mut entry = self.spendable_notes.remove(i);
            if let Some(path) = entry.merkle_path.take() {
                result.push(SpendableNote {
                    note: entry.note,
                    merkle_path: path,
                    fvk: fvk.clone(),
                });
            }
        }

        result.reverse(); // restore original order
        result
    }

    /// count of spendable notes
    pub fn spendable_count(&self) -> usize {
        self.spendable_notes.iter()
            .filter(|n| !n.is_spent && n.merkle_path.is_some())
            .count()
    }

    /// total spendable balance
    pub fn spendable_balance(&self) -> u64 {
        self.spendable_notes.iter()
            .filter(|n| !n.is_spent && n.merkle_path.is_some())
            .map(|n| n.note.value().inner())
            .sum()
    }

    /// update merkle path for a note (called after commitment tree update)
    pub fn update_merkle_path(&mut self, commitment: &[u8; 32], path: MerklePath) {
        for entry in &mut self.spendable_notes {
            if &entry.commitment == commitment {
                entry.merkle_path = Some(path);
                debug!("updated merkle path for {}", hex::encode(commitment));
                return;
            }
        }
    }

    /// scan compact block for owned notes
    pub fn scan_block(&mut self, block: &crate::client::CompactBlock) -> Result<usize> {
        let mut found = 0;

        for action in &block.actions {
            // trial decrypt with ivk
            if let Some(note) = self.try_decrypt(action, block.height) {
                self.owned_notes.push(note);
                found += 1;
            }

            // check if any of our notes were spent
            if self.spent_nullifiers.contains(&action.nullifier) {
                // mark note as spent
                self.mark_spent(&action.nullifier);
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
        _action: &crate::client::CompactAction,
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
