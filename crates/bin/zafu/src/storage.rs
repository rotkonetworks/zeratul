//! wallet storage using sled

use anyhow::Result;
use sled::Db;
use tracing::info;
use orchard::tree::Anchor;

pub struct WalletStorage {
    db: Db,
}

impl WalletStorage {
    pub fn open(path: &str) -> Result<Self> {
        info!("opening wallet storage at {}", path);
        let db = sled::open(path)?;
        Ok(Self { db })
    }

    /// store last synced height
    pub fn set_last_sync_height(&self, height: u32) -> Result<()> {
        self.db.insert(b"last_sync_height", &height.to_le_bytes())?;
        Ok(())
    }

    /// get last synced height
    pub fn get_last_sync_height(&self) -> Result<Option<u32>> {
        match self.db.get(b"last_sync_height")? {
            Some(bytes) if bytes.len() == 4 => {
                let height = u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]);
                Ok(Some(height))
            }
            _ => Ok(None),
        }
    }

    /// store verified proof hash (to avoid re-verification)
    pub fn set_verified_proof(&self, height: u32, proof_hash: &[u8; 32]) -> Result<()> {
        let key = format!("proof:{}", height);
        self.db.insert(key.as_bytes(), &proof_hash[..])?;
        Ok(())
    }

    /// check if proof was already verified
    pub fn has_verified_proof(&self, height: u32) -> Result<bool> {
        let key = format!("proof:{}", height);
        Ok(self.db.contains_key(key.as_bytes())?)
    }

    /// store viewing key
    pub fn set_viewing_key(&self, key: &str) -> Result<()> {
        self.db.insert(b"viewing_key", key.as_bytes())?;
        self.db.flush()?;
        Ok(())
    }

    /// get viewing key
    pub fn get_viewing_key(&self) -> Result<Option<String>> {
        match self.db.get(b"viewing_key")? {
            Some(bytes) => {
                let key = String::from_utf8(bytes.to_vec())?;
                Ok(Some(key))
            }
            None => Ok(None),
        }
    }

    /// clear viewing key
    pub fn clear_viewing_key(&self) -> Result<()> {
        self.db.remove(b"viewing_key")?;
        self.db.flush()?;
        Ok(())
    }

    /// store current anchor (commitment tree root)
    pub fn set_anchor(&self, anchor: &Anchor) -> Result<()> {
        let bytes = anchor.to_bytes();
        self.db.insert(b"anchor", &bytes)?;
        self.db.flush()?;
        Ok(())
    }

    /// get current anchor (returns empty anchor if not set)
    pub fn get_anchor(&self) -> Anchor {
        match self.db.get(b"anchor") {
            Ok(Some(bytes)) if bytes.len() == 32 => {
                let mut arr = [0u8; 32];
                arr.copy_from_slice(&bytes);
                let opt = Anchor::from_bytes(arr);
                if opt.is_some().into() {
                    opt.unwrap()
                } else {
                    Anchor::empty_tree()
                }
            }
            _ => Anchor::empty_tree(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn test_storage_height() {
        let dir = tempdir().unwrap();
        let storage = WalletStorage::open(dir.path().to_str().unwrap()).unwrap();

        assert_eq!(storage.get_last_sync_height().unwrap(), None);

        storage.set_last_sync_height(12345).unwrap();
        assert_eq!(storage.get_last_sync_height().unwrap(), Some(12345));
    }
}
