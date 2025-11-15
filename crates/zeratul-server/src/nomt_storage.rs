//! NOMT storage wrapper
//!
//! Manages authenticated state storage using NOMT.

use anyhow::Result;
use nomt::{Nomt, Options, SessionParams, WitnessMode, KeyReadWrite, hasher::Blake3Hasher};
use nomt::trie::KeyPath;
use std::collections::HashMap;

/// Wrapper around NOMT database
pub struct NomtStorage {
    db: Nomt<Blake3Hasher>,
}

impl NomtStorage {
    /// Open or create NOMT database
    pub fn new(path: &str) -> Result<Self> {
        let mut options = Options::new();
        options.path(path);
        options.bitbox_seed([0; 16]); // Use a fixed seed for deterministic behavior
        options.commit_concurrency(1);
        let db = Nomt::<Blake3Hasher>::open(options)?;
        Ok(Self { db })
    }

    /// Get current state root
    pub fn get_root(&self) -> Result<[u8; 32]> {
        Ok(self.db.root().into_inner())
    }

    /// Update commitments for sender and receiver
    pub fn update_commitments(
        &mut self,
        sender_id: u64,
        sender_commitment: &[u8; 32],
        receiver_id: u64,
        receiver_commitment: &[u8; 32],
    ) -> Result<()> {
        // Start a new session
        let session = self.db.begin_session(
            SessionParams::default().witness_mode(WitnessMode::disabled())
        );

        // Prepare access patterns
        let mut access = HashMap::new();

        // Write sender commitment
        let sender_key = account_key_path(sender_id);
        access.insert(sender_key, KeyReadWrite::Write(Some(sender_commitment.to_vec())));

        // Write receiver commitment
        let receiver_key = account_key_path(receiver_id);
        access.insert(receiver_key, KeyReadWrite::Write(Some(receiver_commitment.to_vec())));

        // Sort access by key (required by NOMT)
        let mut access_vec: Vec<_> = access.into_iter().collect();
        access_vec.sort_by_key(|(k, _)| *k);

        // Finish session and commit
        let finished = session.finish(access_vec)?;
        finished.commit(&self.db)?;

        Ok(())
    }

    /// Get commitment for account
    pub fn get_commitment(&self, account_id: u64) -> Result<Option<[u8; 32]>> {
        let key = account_key_path(account_id);

        if let Some(value) = self.db.read(key)? {
            if value.len() == 32 {
                let mut commitment = [0u8; 32];
                commitment.copy_from_slice(&value);
                return Ok(Some(commitment));
            }
        }
        Ok(None)
    }
}

/// Convert account ID to NOMT key path
///
/// KeyPaths must be uniformly distributed. We use a simple PRNG seeded with the account ID.
fn account_key_path(id: u64) -> KeyPath {
    use rand::{RngCore, SeedableRng};
    let mut seed = [0u8; 32]; // ChaCha8Rng needs 32 bytes
    seed[0..8].copy_from_slice(&id.to_le_bytes());
    let mut rng = rand_chacha::ChaCha8Rng::from_seed(seed);

    let mut path = KeyPath::default();
    for i in 0..8 {
        path[i * 4..][..4].copy_from_slice(&rng.next_u32().to_le_bytes());
    }
    path
}
