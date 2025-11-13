//! NOMT storage wrapper
//!
//! Manages authenticated state storage using NOMT.

use anyhow::Result;
use nomt::{Nomt, Blake3};
use std::path::Path;

/// Wrapper around NOMT database
pub struct NomtStorage {
    db: Nomt<Blake3>,
}

impl NomtStorage {
    /// Open or create NOMT database
    pub fn new(path: &str) -> Result<Self> {
        let db = Nomt::<Blake3>::open(Path::new(path))?;
        Ok(Self { db })
    }

    /// Get current state root
    pub fn get_root(&self) -> Result<[u8; 32]> {
        Ok(self.db.root())
    }

    /// Update commitments for sender and receiver
    pub fn update_commitments(
        &mut self,
        sender_id: u64,
        sender_commitment: &[u8; 32],
        receiver_id: u64,
        receiver_commitment: &[u8; 32],
    ) -> Result<()> {
        // Create batch update
        let mut batch = self.db.batch();

        // Update sender commitment
        let sender_key = format!("account:{}", sender_id);
        batch.set(sender_key.as_bytes(), sender_commitment);

        // Update receiver commitment
        let receiver_key = format!("account:{}", receiver_id);
        batch.set(receiver_key.as_bytes(), receiver_commitment);

        // Commit batch
        batch.commit()?;

        Ok(())
    }

    /// Get commitment for account
    pub fn get_commitment(&self, account_id: u64) -> Result<Option<[u8; 32]>> {
        let key = format!("account:{}", account_id);
        if let Some(value) = self.db.get(key.as_bytes())? {
            if value.len() == 32 {
                let mut commitment = [0u8; 32];
                commitment.copy_from_slice(&value);
                return Ok(Some(commitment));
            }
        }
        Ok(None)
    }
}
