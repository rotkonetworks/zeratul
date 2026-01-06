//! native storage backend using NOMT
//!
//! stores encrypted shielded state in a local NOMT database.
//! all data is encrypted with ChaCha20Poly1305 before storage.

use super::{EncryptedNote, ShieldedState, ShieldedStorage, StorageError};
use chacha20poly1305::{
    aead::{Aead, KeyInit},
    ChaCha20Poly1305, Nonce,
};
use nomt::{hasher::Blake3Hasher, KeyReadWrite, Nomt, Options, SessionParams, WitnessMode};
use nomt::trie::KeyPath;
use parking_lot::RwLock;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;

/// key prefixes for different data types
const PREFIX_NOTE: u8 = 0x01;
const PREFIX_NULLIFIER: u8 = 0x02;
const PREFIX_SYNC: u8 = 0x03;

/// native storage using NOMT + encryption
pub struct NativeStorage {
    db: Arc<Nomt<Blake3Hasher>>,
    cipher: ChaCha20Poly1305,
    /// cached state for fast reads
    cache: RwLock<Option<ShieldedState>>,
}

impl NativeStorage {
    /// create new native storage
    pub fn new(path: &str, encryption_key: &[u8; 32]) -> Result<Self, StorageError> {
        let mut options = Options::new();
        options.path(path);
        options.bitbox_seed([0; 16]);
        options.commit_concurrency(1);

        let db = Nomt::<Blake3Hasher>::open(options)
            .map_err(|e| StorageError::Nomt(e.to_string()))?;

        let cipher = ChaCha20Poly1305::new_from_slice(encryption_key)
            .map_err(|e| StorageError::Encryption(e.to_string()))?;

        Ok(Self {
            db: Arc::new(db),
            cipher,
            cache: RwLock::new(None),
        })
    }

    /// encrypt data with random nonce
    fn encrypt(&self, data: &[u8]) -> Result<Vec<u8>, StorageError> {
        // generate random nonce
        let mut nonce_bytes = [0u8; 12];
        getrandom::getrandom(&mut nonce_bytes)
            .map_err(|e| StorageError::Encryption(e.to_string()))?;
        let nonce = Nonce::from_slice(&nonce_bytes);

        // encrypt
        let ciphertext = self.cipher.encrypt(nonce, data)
            .map_err(|e| StorageError::Encryption(e.to_string()))?;

        // prepend nonce to ciphertext
        let mut result = nonce_bytes.to_vec();
        result.extend(ciphertext);
        Ok(result)
    }

    /// decrypt data (nonce prepended)
    fn decrypt(&self, data: &[u8]) -> Result<Vec<u8>, StorageError> {
        if data.len() < 12 {
            return Err(StorageError::Encryption("data too short".into()));
        }

        let nonce = Nonce::from_slice(&data[..12]);
        let ciphertext = &data[12..];

        self.cipher.decrypt(nonce, ciphertext)
            .map_err(|e| StorageError::Encryption(e.to_string()))
    }

    /// generate key path for note
    fn note_key(commitment: &[u8; 32]) -> KeyPath {
        let mut hasher = blake3::Hasher::new();
        hasher.update(&[PREFIX_NOTE]);
        hasher.update(commitment);
        *hasher.finalize().as_bytes()
    }

    /// generate key path for nullifier
    fn nullifier_key(nullifier: &[u8; 32]) -> KeyPath {
        let mut hasher = blake3::Hasher::new();
        hasher.update(&[PREFIX_NULLIFIER]);
        hasher.update(nullifier);
        *hasher.finalize().as_bytes()
    }

    /// generate key path for sync state
    fn sync_key() -> KeyPath {
        let mut hasher = blake3::Hasher::new();
        hasher.update(&[PREFIX_SYNC]);
        hasher.update(b"sync_state");
        *hasher.finalize().as_bytes()
    }
}

impl ShieldedStorage for NativeStorage {
    fn load(&self) -> Result<ShieldedState, StorageError> {
        // check cache first
        if let Some(state) = self.cache.read().as_ref() {
            return Ok(state.clone());
        }

        // load sync state
        let sync_key = Self::sync_key();
        let sync_data = self.db.read(sync_key)
            .map_err(|e| StorageError::Nomt(e.to_string()))?;

        let (sync_height, merkle_root) = if let Some(data) = sync_data {
            let decrypted = self.decrypt(&data)?;
            if decrypted.len() >= 40 {
                let height = u64::from_le_bytes(decrypted[..8].try_into().unwrap());
                let mut root = [0u8; 32];
                root.copy_from_slice(&decrypted[8..40]);
                (height, root)
            } else {
                (0, [0u8; 32])
            }
        } else {
            (0, [0u8; 32])
        };

        // note: full note/nullifier loading would require scanning
        // for now return empty state with sync info
        let state = ShieldedState {
            notes: HashMap::new(),
            nullifiers: HashSet::new(),
            sync_height,
            merkle_root,
        };

        // update cache
        *self.cache.write() = Some(state.clone());

        Ok(state)
    }

    fn save(&self, state: &ShieldedState) -> Result<(), StorageError> {
        let session = self.db.begin_session(
            SessionParams::default().witness_mode(WitnessMode::disabled())
        );

        let mut access = Vec::new();

        // save sync state
        let mut sync_data = Vec::with_capacity(40);
        sync_data.extend_from_slice(&state.sync_height.to_le_bytes());
        sync_data.extend_from_slice(&state.merkle_root);
        let encrypted_sync = self.encrypt(&sync_data)?;

        access.push((
            Self::sync_key(),
            KeyReadWrite::Write(Some(encrypted_sync)),
        ));

        // save notes
        for (commitment, note) in &state.notes {
            let note_data = bincode::serialize(note)
                .map_err(|e| StorageError::Serialization(e.to_string()))?;
            let encrypted = self.encrypt(&note_data)?;

            access.push((
                Self::note_key(commitment),
                KeyReadWrite::Write(Some(encrypted)),
            ));
        }

        // save nullifiers (just marker, no data needed)
        for nullifier in &state.nullifiers {
            access.push((
                Self::nullifier_key(nullifier),
                KeyReadWrite::Write(Some(vec![1])), // marker
            ));
        }

        // sort by key (required by NOMT)
        access.sort_by_key(|(k, _)| *k);

        // commit
        let finished = session.finish(access)
            .map_err(|e| StorageError::Nomt(e.to_string()))?;
        finished.commit(&self.db)
            .map_err(|e| StorageError::Nomt(e.to_string()))?;

        // update cache
        *self.cache.write() = Some(state.clone());

        Ok(())
    }

    fn add_note(&self, note: EncryptedNote) -> Result<(), StorageError> {
        let mut state = self.load()?;
        state.notes.insert(note.commitment, note);
        self.save(&state)
    }

    fn spend_note(&self, nullifier: [u8; 32]) -> Result<(), StorageError> {
        let mut state = self.load()?;
        state.nullifiers.insert(nullifier);
        self.save(&state)
    }

    fn update_sync(&self, height: u64, root: [u8; 32]) -> Result<(), StorageError> {
        let mut state = self.load()?;
        state.sync_height = height;
        state.merkle_root = root;
        self.save(&state)
    }

    fn get_unspent_notes(&self) -> Result<Vec<EncryptedNote>, StorageError> {
        let state = self.load()?;
        Ok(state.notes.values()
            .filter(|note| !state.nullifiers.contains(&note.commitment))
            .cloned()
            .collect())
    }

    fn is_spent(&self, nullifier: &[u8; 32]) -> Result<bool, StorageError> {
        // check NOMT directly
        let key = Self::nullifier_key(nullifier);
        let exists = self.db.read(key)
            .map_err(|e| StorageError::Nomt(e.to_string()))?
            .is_some();
        Ok(exists)
    }
}

// implement Clone for ShieldedState
impl Clone for ShieldedState {
    fn clone(&self) -> Self {
        Self {
            notes: self.notes.clone(),
            nullifiers: self.nullifiers.clone(),
            sync_height: self.sync_height,
            merkle_root: self.merkle_root,
        }
    }
}

// implement Serialize/Deserialize for EncryptedNote
impl serde::Serialize for EncryptedNote {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        use serde::ser::SerializeStruct;
        let mut state = serializer.serialize_struct("EncryptedNote", 4)?;
        state.serialize_field("commitment", &self.commitment)?;
        state.serialize_field("ciphertext", &self.ciphertext)?;
        state.serialize_field("position", &self.position)?;
        state.serialize_field("witness", &self.witness)?;
        state.end()
    }
}

impl<'de> serde::Deserialize<'de> for EncryptedNote {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        #[derive(serde::Deserialize)]
        struct NoteHelper {
            commitment: [u8; 32],
            ciphertext: Vec<u8>,
            position: u64,
            witness: Vec<[u8; 32]>,
        }

        let helper = NoteHelper::deserialize(deserializer)?;
        Ok(EncryptedNote {
            commitment: helper.commitment,
            ciphertext: helper.ciphertext,
            position: helper.position,
            witness: helper.witness,
        })
    }
}
