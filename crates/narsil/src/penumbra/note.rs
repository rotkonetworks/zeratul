//! note management for syndicate UTXOs
//!
//! penumbra uses a UTXO model - funds exist as "notes" that are
//! consumed when spent. syndicates need to:
//!
//! 1. scan chain for notes belonging to syndicate address
//! 2. maintain synchronized view of available notes
//! 3. select notes for spending
//!
//! all members can scan (using shared viewing key) but only
//! threshold can spend (using OSST).

use alloc::collections::BTreeMap;
use alloc::vec::Vec;
use sha2::{Digest, Sha256};

use super::action::{AssetId, Value};

/// a note (UTXO) belonging to the syndicate
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SyndicateNote {
    /// note commitment (unique identifier)
    pub commitment: [u8; 32],
    /// value contained
    pub value: Value,
    /// position in the note commitment tree
    pub position: u64,
    /// block height where note was created
    pub height: u64,
    /// whether note has been spent
    pub spent: bool,
    /// nullifier (revealed when spent)
    pub nullifier: Option<[u8; 32]>,
    /// penumbra-specific: rseed for note reconstruction (32 bytes)
    pub rseed: Option<[u8; 32]>,
    /// penumbra-specific: diversifier index for address derivation
    pub address_index: Option<u32>,
}

impl SyndicateNote {
    pub fn new(commitment: [u8; 32], value: Value, position: u64, height: u64) -> Self {
        Self {
            commitment,
            value,
            position,
            height,
            spent: false,
            nullifier: None,
            rseed: None,
            address_index: None,
        }
    }

    /// create note with penumbra-specific data for proper reconstruction
    pub fn with_penumbra_data(
        commitment: [u8; 32],
        value: Value,
        position: u64,
        height: u64,
        rseed: [u8; 32],
        address_index: u32,
    ) -> Self {
        Self {
            commitment,
            value,
            position,
            height,
            spent: false,
            nullifier: None,
            rseed: Some(rseed),
            address_index: Some(address_index),
        }
    }

    /// compute nullifier for this note
    /// (requires viewing key, simplified here)
    pub fn compute_nullifier(&self, viewing_key: &[u8; 32]) -> [u8; 32] {
        let mut hasher = Sha256::new();
        hasher.update(b"narsil-nullifier");
        hasher.update(viewing_key);
        hasher.update(&self.commitment);
        hasher.update(self.position.to_le_bytes());
        hasher.finalize().into()
    }

    /// mark as spent
    pub fn mark_spent(&mut self, nullifier: [u8; 32]) {
        self.spent = true;
        self.nullifier = Some(nullifier);
    }
}

/// set of notes belonging to syndicate
#[derive(Clone, Debug, Default)]
pub struct NoteSet {
    /// all notes by commitment
    notes: BTreeMap<[u8; 32], SyndicateNote>,
    /// known nullifiers (spent notes)
    nullifiers: BTreeMap<[u8; 32], [u8; 32]>,  // nullifier -> commitment
    /// last scanned height
    pub scanned_height: u64,
}

impl NoteSet {
    pub fn new() -> Self {
        Self::default()
    }

    /// add a newly discovered note
    pub fn add_note(&mut self, note: SyndicateNote) {
        self.notes.insert(note.commitment, note);
    }

    /// mark note as spent via nullifier
    /// in real impl, would verify nullifier matches note
    pub fn mark_spent(&mut self, nullifier: [u8; 32]) -> bool {
        // find first unspent note and mark it
        // (simplified - real impl would verify nullifier cryptographically)
        let commitment = self.notes
            .values()
            .find(|n| !n.spent)
            .map(|n| n.commitment);

        if let Some(commitment) = commitment {
            return self.mark_spent_by_commitment(&commitment, nullifier);
        }
        false
    }

    /// mark spent by commitment (when we know which note)
    pub fn mark_spent_by_commitment(&mut self, commitment: &[u8; 32], nullifier: [u8; 32]) -> bool {
        if let Some(note) = self.notes.get_mut(commitment) {
            if !note.spent {
                note.mark_spent(nullifier);
                self.nullifiers.insert(nullifier, *commitment);
                return true;
            }
        }
        false
    }

    /// get unspent notes
    pub fn unspent(&self) -> impl Iterator<Item = &SyndicateNote> {
        self.notes.values().filter(|n| !n.spent)
    }

    /// get unspent notes for specific asset
    pub fn unspent_for_asset<'a>(&'a self, asset: &'a AssetId) -> impl Iterator<Item = &'a SyndicateNote> {
        self.notes.values().filter(move |n| !n.spent && n.value.asset_id == *asset)
    }

    /// total unspent balance for asset
    pub fn balance(&self, asset: &AssetId) -> u128 {
        self.unspent_for_asset(asset).map(|n| n.value.amount).sum()
    }

    /// total unspent balance (native asset)
    pub fn native_balance(&self) -> u128 {
        self.balance(&AssetId::native())
    }

    /// select notes to cover amount (simple greedy)
    pub fn select_notes<'a>(&'a self, asset: &'a AssetId, amount: u128) -> Option<Vec<&'a SyndicateNote>> {
        let mut selected = Vec::new();
        let mut total = 0u128;

        // sort by amount descending for efficiency
        let mut available: Vec<_> = self.unspent_for_asset(asset).collect();
        available.sort_by(|a, b| b.value.amount.cmp(&a.value.amount));

        for note in available {
            selected.push(note);
            total += note.value.amount;
            if total >= amount {
                return Some(selected);
            }
        }

        None  // insufficient funds
    }

    /// number of unspent notes
    pub fn unspent_count(&self) -> usize {
        self.unspent().count()
    }

    /// check if nullifier is known (note was spent)
    pub fn is_nullifier_spent(&self, nullifier: &[u8; 32]) -> bool {
        self.nullifiers.contains_key(nullifier)
    }

    /// update scanned height
    pub fn update_height(&mut self, height: u64) {
        if height > self.scanned_height {
            self.scanned_height = height;
        }
    }

    /// get note by commitment
    pub fn get(&self, commitment: &[u8; 32]) -> Option<&SyndicateNote> {
        self.notes.get(commitment)
    }

    /// all notes (for sync)
    pub fn all_notes(&self) -> impl Iterator<Item = &SyndicateNote> {
        self.notes.values()
    }
}

/// result of scanning a block
#[derive(Clone, Debug, Default)]
pub struct ScanResult {
    /// new notes found
    pub new_notes: Vec<SyndicateNote>,
    /// nullifiers seen (notes spent)
    pub spent_nullifiers: Vec<[u8; 32]>,
    /// block height
    pub height: u64,
}

impl ScanResult {
    pub fn new(height: u64) -> Self {
        Self {
            new_notes: Vec::new(),
            spent_nullifiers: Vec::new(),
            height,
        }
    }

    /// apply scan result to note set
    pub fn apply_to(&self, note_set: &mut NoteSet) {
        for note in &self.new_notes {
            note_set.add_note(note.clone());
        }
        for nullifier in &self.spent_nullifiers {
            // try to mark spent - may fail if note not in our set
            note_set.mark_spent(*nullifier);
        }
        note_set.update_height(self.height);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_note(id: u8, amount: u128) -> SyndicateNote {
        SyndicateNote::new(
            [id; 32],
            Value::new(amount, AssetId::native()),
            id as u64,
            100,
        )
    }

    #[test]
    fn test_note_set_balance() {
        let mut set = NoteSet::new();
        set.add_note(make_note(1, 100));
        set.add_note(make_note(2, 200));
        set.add_note(make_note(3, 300));

        assert_eq!(set.native_balance(), 600);
        assert_eq!(set.unspent_count(), 3);
    }

    #[test]
    fn test_note_set_spending() {
        let mut set = NoteSet::new();
        set.add_note(make_note(1, 100));
        set.add_note(make_note(2, 200));

        assert_eq!(set.native_balance(), 300);

        set.mark_spent_by_commitment(&[1u8; 32], [99u8; 32]);

        assert_eq!(set.native_balance(), 200);
        assert_eq!(set.unspent_count(), 1);
        assert!(set.is_nullifier_spent(&[99u8; 32]));
    }

    #[test]
    fn test_note_selection() {
        let mut set = NoteSet::new();
        set.add_note(make_note(1, 100));
        set.add_note(make_note(2, 50));
        set.add_note(make_note(3, 200));

        let native = AssetId::native();

        // select notes covering 150
        let selected = set.select_notes(&native, 150).unwrap();
        let total: u128 = selected.iter().map(|n| n.value.amount).sum();
        assert!(total >= 150);

        // can't select 1000 (only have 350)
        assert!(set.select_notes(&native, 1000).is_none());
    }

    #[test]
    fn test_scan_result_apply() {
        let mut set = NoteSet::new();
        set.add_note(make_note(1, 100));

        let mut scan = ScanResult::new(101);
        scan.new_notes.push(make_note(2, 200));
        scan.apply_to(&mut set);

        assert_eq!(set.native_balance(), 300);
        assert_eq!(set.scanned_height, 101);
    }
}
