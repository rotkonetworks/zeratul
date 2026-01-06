//! client-side shielded state storage
//!
//! stores encrypted notes, nullifiers, and merkle witnesses locally.
//! uses NOMT on native (linux) or IndexedDB on wasm.
//!
//! ## architecture
//!
//! ```text
//! ShieldedState
//! ├── notes: encrypted note commitments we own
//! ├── nullifiers: spent note markers (prevents double-spend)
//! ├── witnesses: merkle proofs for our notes
//! ├── sync_height: last synced block
//! └── viewing_keys: for scanning new notes
//! ```

#[cfg(not(target_arch = "wasm32"))]
mod native;
#[cfg(target_arch = "wasm32")]
mod wasm;

use std::collections::HashMap;

/// encrypted note stored locally
#[derive(Clone, Debug)]
pub struct EncryptedNote {
    /// note commitment (public, on-chain)
    pub commitment: [u8; 32],
    /// encrypted note data (value, asset, rseed)
    pub ciphertext: Vec<u8>,
    /// position in merkle tree
    pub position: u64,
    /// merkle witness (sibling hashes)
    pub witness: Vec<[u8; 32]>,
}

/// shielded state stored client-side
#[derive(Default)]
pub struct ShieldedState {
    /// our encrypted notes (commitment -> note)
    pub notes: HashMap<[u8; 32], EncryptedNote>,
    /// nullifiers we've seen (to detect spent notes)
    pub nullifiers: std::collections::HashSet<[u8; 32]>,
    /// last synced block height
    pub sync_height: u64,
    /// merkle root at sync_height
    pub merkle_root: [u8; 32],
}

/// storage backend trait
pub trait ShieldedStorage: Send + Sync {
    /// load shielded state from storage
    fn load(&self) -> Result<ShieldedState, StorageError>;

    /// save shielded state to storage
    fn save(&self, state: &ShieldedState) -> Result<(), StorageError>;

    /// add a new note
    fn add_note(&self, note: EncryptedNote) -> Result<(), StorageError>;

    /// mark a note as spent (add nullifier)
    fn spend_note(&self, nullifier: [u8; 32]) -> Result<(), StorageError>;

    /// update sync progress
    fn update_sync(&self, height: u64, root: [u8; 32]) -> Result<(), StorageError>;

    /// get unspent notes
    fn get_unspent_notes(&self) -> Result<Vec<EncryptedNote>, StorageError>;

    /// check if nullifier exists (note already spent)
    fn is_spent(&self, nullifier: &[u8; 32]) -> Result<bool, StorageError>;
}

/// storage errors
#[derive(Debug, thiserror::Error)]
pub enum StorageError {
    #[error("storage not initialized")]
    NotInitialized,
    #[error("io error: {0}")]
    Io(String),
    #[error("serialization error: {0}")]
    Serialization(String),
    #[error("encryption error: {0}")]
    Encryption(String),
    #[error("nomt error: {0}")]
    Nomt(String),
}

/// create storage backend for current platform
#[cfg(not(target_arch = "wasm32"))]
pub fn create_storage(path: &str, encryption_key: &[u8; 32]) -> Result<Box<dyn ShieldedStorage>, StorageError> {
    native::NativeStorage::new(path, encryption_key)
        .map(|s| Box::new(s) as Box<dyn ShieldedStorage>)
}

#[cfg(target_arch = "wasm32")]
pub fn create_storage(_path: &str, encryption_key: &[u8; 32]) -> Result<Box<dyn ShieldedStorage>, StorageError> {
    wasm::WasmStorage::new(encryption_key)
        .map(|s| Box::new(s) as Box<dyn ShieldedStorage>)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_shielded_state_default() {
        let state = ShieldedState::default();
        assert!(state.notes.is_empty());
        assert!(state.nullifiers.is_empty());
        assert_eq!(state.sync_height, 0);
    }
}
