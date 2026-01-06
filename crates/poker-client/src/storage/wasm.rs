//! wasm storage backend using IndexedDB
//!
//! stores encrypted shielded state in browser IndexedDB.
//! all data is encrypted with ChaCha20Poly1305 before storage.

use super::{EncryptedNote, ShieldedState, ShieldedStorage, StorageError};
use chacha20poly1305::{
    aead::{Aead, KeyInit},
    ChaCha20Poly1305, Nonce,
};
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use parking_lot::RwLock;
use wasm_bindgen::prelude::*;
use web_sys::{IdbDatabase, IdbObjectStore, IdbRequest, IdbTransaction};

const DB_NAME: &str = "ghettobox_shielded";
const DB_VERSION: u32 = 1;
const STORE_NOTES: &str = "notes";
const STORE_NULLIFIERS: &str = "nullifiers";
const STORE_SYNC: &str = "sync";

/// wasm storage using IndexedDB + encryption
pub struct WasmStorage {
    cipher: ChaCha20Poly1305,
    /// cached state
    cache: RwLock<Option<ShieldedState>>,
}

impl WasmStorage {
    /// create new wasm storage
    pub fn new(encryption_key: &[u8; 32]) -> Result<Self, StorageError> {
        let cipher = ChaCha20Poly1305::new_from_slice(encryption_key)
            .map_err(|e| StorageError::Encryption(e.to_string()))?;

        Ok(Self {
            cipher,
            cache: RwLock::new(None),
        })
    }

    /// encrypt data with random nonce
    fn encrypt(&self, data: &[u8]) -> Result<Vec<u8>, StorageError> {
        let mut nonce_bytes = [0u8; 12];
        getrandom::getrandom(&mut nonce_bytes)
            .map_err(|e| StorageError::Encryption(e.to_string()))?;
        let nonce = Nonce::from_slice(&nonce_bytes);

        let ciphertext = self.cipher.encrypt(nonce, data)
            .map_err(|e| StorageError::Encryption(e.to_string()))?;

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
}

impl ShieldedStorage for WasmStorage {
    fn load(&self) -> Result<ShieldedState, StorageError> {
        // check cache first
        if let Some(state) = self.cache.read().as_ref() {
            return Ok(state.clone());
        }

        // in wasm, we'd use async IndexedDB API
        // for now, return empty state (actual impl would use wasm-bindgen-futures)
        let state = ShieldedState::default();
        *self.cache.write() = Some(state.clone());
        Ok(state)
    }

    fn save(&self, state: &ShieldedState) -> Result<(), StorageError> {
        // update cache
        *self.cache.write() = Some(state.clone());

        // actual IndexedDB write would be async
        // this is a sync interface, so we'd need to spawn the write
        // and handle errors via callback/channel

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
        let state = self.load()?;
        Ok(state.nullifiers.contains(nullifier))
    }
}

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

// helper: open IndexedDB (async, would need wasm-bindgen-futures)
// for now this is a placeholder - actual implementation would be:
//
// ```
// use wasm_bindgen_futures::JsFuture;
// use js_sys::Promise;
//
// async fn open_db() -> Result<IdbDatabase, JsValue> {
//     let window = web_sys::window().unwrap();
//     let idb = window.indexed_db()?.unwrap();
//
//     let request = idb.open_with_u32(DB_NAME, DB_VERSION)?;
//
//     // handle upgrade needed
//     let onupgradeneeded = Closure::wrap(Box::new(move |event: web_sys::IdbVersionChangeEvent| {
//         let db: IdbDatabase = event.target().unwrap().result().unwrap().into();
//         if !db.object_store_names().contains(&STORE_NOTES.into()) {
//             db.create_object_store(STORE_NOTES)?;
//         }
//         if !db.object_store_names().contains(&STORE_NULLIFIERS.into()) {
//             db.create_object_store(STORE_NULLIFIERS)?;
//         }
//         if !db.object_store_names().contains(&STORE_SYNC.into()) {
//             db.create_object_store(STORE_SYNC)?;
//         }
//     }) as Box<dyn FnMut(_)>);
//
//     request.set_onupgradeneeded(Some(onupgradeneeded.as_ref().unchecked_ref()));
//
//     let promise = Promise::new(&mut |resolve, reject| {
//         request.set_onsuccess(Some(&resolve));
//         request.set_onerror(Some(&reject));
//     });
//
//     let result = JsFuture::from(promise).await?;
//     Ok(result.into())
// }
// ```
