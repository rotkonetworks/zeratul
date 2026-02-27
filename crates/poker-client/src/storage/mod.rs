//! client-side shielded state storage
//!
//! stores encrypted notes, nullifiers, and merkle witnesses locally.
//! also stores playmate history and settings.
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
//!
//! PlaymateStorage
//! ├── playmates: friends and recent players
//! └── settings: user preferences
//! ```

#[cfg(not(target_arch = "wasm32"))]
mod native;
#[cfg(target_arch = "wasm32")]
mod wasm;

use std::collections::HashMap;

use crate::friends::{FriendsState, Playmate, PlayerId};

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

/// playmate storage trait
pub trait PlaymateStorage: Send + Sync {
    /// load all playmates
    fn load_playmates(&self) -> Result<HashMap<PlayerId, Playmate>, StorageError>;

    /// save all playmates
    fn save_playmates(&self, playmates: &HashMap<PlayerId, Playmate>) -> Result<(), StorageError>;
}

/// simple file-based playmate storage (for native)
#[cfg(not(target_arch = "wasm32"))]
pub struct FilePlaymateStorage {
    path: std::path::PathBuf,
    encryption_key: [u8; 32],
}

#[cfg(not(target_arch = "wasm32"))]
impl FilePlaymateStorage {
    pub fn new(path: &str, encryption_key: &[u8; 32]) -> Self {
        Self {
            path: std::path::PathBuf::from(path).join("playmates.dat"),
            encryption_key: *encryption_key,
        }
    }
}

#[cfg(not(target_arch = "wasm32"))]
impl PlaymateStorage for FilePlaymateStorage {
    fn load_playmates(&self) -> Result<HashMap<PlayerId, Playmate>, StorageError> {
        use chacha20poly1305::{ChaCha20Poly1305, KeyInit, aead::Aead};
        use chacha20poly1305::aead::generic_array::GenericArray;

        if !self.path.exists() {
            return Ok(HashMap::new());
        }

        let data = std::fs::read(&self.path)
            .map_err(|e| StorageError::Io(e.to_string()))?;

        if data.len() < 12 {
            return Ok(HashMap::new());
        }

        // first 12 bytes are nonce
        let nonce = GenericArray::from_slice(&data[..12]);
        let ciphertext = &data[12..];

        let cipher = ChaCha20Poly1305::new(GenericArray::from_slice(&self.encryption_key));
        let plaintext = cipher.decrypt(nonce, ciphertext)
            .map_err(|e| StorageError::Encryption(e.to_string()))?;

        FriendsState::deserialize(&plaintext)
            .ok_or_else(|| StorageError::Serialization("failed to deserialize playmates".into()))
    }

    fn save_playmates(&self, playmates: &HashMap<PlayerId, Playmate>) -> Result<(), StorageError> {
        use chacha20poly1305::{ChaCha20Poly1305, KeyInit, aead::Aead};
        use chacha20poly1305::aead::generic_array::GenericArray;
        use rand::RngCore;

        let records: Vec<&Playmate> = playmates.values().collect();
        let plaintext = bincode::serialize(&records)
            .map_err(|e| StorageError::Serialization(e.to_string()))?;

        // generate random nonce
        let mut nonce_bytes = [0u8; 12];
        rand::thread_rng().fill_bytes(&mut nonce_bytes);
        let nonce = GenericArray::from_slice(&nonce_bytes);

        let cipher = ChaCha20Poly1305::new(GenericArray::from_slice(&self.encryption_key));
        let ciphertext = cipher.encrypt(nonce, plaintext.as_ref())
            .map_err(|e| StorageError::Encryption(e.to_string()))?;

        // write nonce + ciphertext
        let mut output = nonce_bytes.to_vec();
        output.extend(ciphertext);

        // ensure parent dir exists
        if let Some(parent) = self.path.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| StorageError::Io(e.to_string()))?;
        }

        std::fs::write(&self.path, output)
            .map_err(|e| StorageError::Io(e.to_string()))?;

        Ok(())
    }
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
