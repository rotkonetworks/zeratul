// local wallet state backed by sled

use orchard::note::{Rho, RandomSeed};
use orchard::value::NoteValue;
use sled::Db;

use crate::error::Error;

const SYNC_HEIGHT_KEY: &[u8] = b"sync_height";
const ORCHARD_POSITION_KEY: &[u8] = b"orchard_position";
const NOTES_TREE: &str = "notes";
const NULLIFIERS_TREE: &str = "nullifiers";

/// a received note stored in the wallet
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct WalletNote {
    pub value: u64,
    pub nullifier: [u8; 32],
    pub cmx: [u8; 32],
    pub block_height: u32,
    pub is_change: bool,
    // spend data — required for orchard spend circuit
    // orchard address is 43 bytes (11-byte diversifier + 32-byte pk_d)
    pub recipient: Vec<u8>,
    pub rho: [u8; 32],
    pub rseed: [u8; 32],
    pub position: u64,
}

impl WalletNote {
    /// reconstruct an orchard::Note from stored bytes
    pub fn reconstruct_note(&self) -> Result<orchard::Note, Error> {
        if self.recipient.len() != 43 {
            return Err(Error::Wallet(format!(
                "recipient bytes wrong length: {} (expected 43)", self.recipient.len()
            )));
        }
        let mut addr_bytes = [0u8; 43];
        addr_bytes.copy_from_slice(&self.recipient);
        let recipient = Option::from(orchard::Address::from_raw_address_bytes(&addr_bytes))
            .ok_or_else(|| Error::Wallet("invalid recipient bytes".into()))?;
        let value = NoteValue::from_raw(self.value);
        let rho = Option::from(Rho::from_bytes(&self.rho))
            .ok_or_else(|| Error::Wallet("invalid rho bytes".into()))?;
        let rseed = Option::from(RandomSeed::from_bytes(self.rseed, &rho))
            .ok_or_else(|| Error::Wallet("invalid rseed bytes".into()))?;
        Option::from(orchard::Note::from_parts(recipient, value, rho, rseed))
            .ok_or_else(|| Error::Wallet("failed to reconstruct note".into()))
    }
}

pub struct Wallet {
    db: Db,
}

impl Wallet {
    pub fn open(path: &str) -> Result<Self, Error> {
        let db = sled::open(path)
            .map_err(|e| Error::Wallet(format!("cannot open wallet db at {}: {}", path, e)))?;
        Ok(Self { db })
    }

    /// default wallet path: ~/.zcli/wallet
    pub fn default_path() -> String {
        let home = std::env::var("HOME").unwrap_or_else(|_| ".".into());
        format!("{}/.zcli/wallet", home)
    }

    pub fn sync_height(&self) -> Result<u32, Error> {
        match self.db.get(SYNC_HEIGHT_KEY)
            .map_err(|e| Error::Wallet(format!("read sync height: {}", e)))? {
            Some(bytes) => {
                if bytes.len() == 4 {
                    Ok(u32::from_le_bytes(bytes.as_ref().try_into().expect("len checked")))
                } else {
                    Ok(0)
                }
            }
            None => Ok(0),
        }
    }

    pub fn set_sync_height(&self, height: u32) -> Result<(), Error> {
        self.db.insert(SYNC_HEIGHT_KEY, &height.to_le_bytes())
            .map_err(|e| Error::Wallet(format!("write sync height: {}", e)))?;
        Ok(())
    }

    /// store a received note, keyed by nullifier
    pub fn insert_note(&self, note: &WalletNote) -> Result<(), Error> {
        let tree = self.db.open_tree(NOTES_TREE)
            .map_err(|e| Error::Wallet(format!("open notes tree: {}", e)))?;
        let value = serde_json::to_vec(note)
            .map_err(|e| Error::Wallet(format!("serialize note: {}", e)))?;
        tree.insert(&note.nullifier, value)
            .map_err(|e| Error::Wallet(format!("insert note: {}", e)))?;
        Ok(())
    }

    /// mark a nullifier as spent
    pub fn mark_spent(&self, nullifier: &[u8; 32]) -> Result<(), Error> {
        let tree = self.db.open_tree(NULLIFIERS_TREE)
            .map_err(|e| Error::Wallet(format!("open nullifiers tree: {}", e)))?;
        tree.insert(nullifier.as_ref(), &[1u8])
            .map_err(|e| Error::Wallet(format!("mark spent: {}", e)))?;
        Ok(())
    }

    pub fn is_spent(&self, nullifier: &[u8; 32]) -> Result<bool, Error> {
        let tree = self.db.open_tree(NULLIFIERS_TREE)
            .map_err(|e| Error::Wallet(format!("open nullifiers tree: {}", e)))?;
        Ok(tree.contains_key(nullifier.as_ref())
            .map_err(|e| Error::Wallet(format!("check spent: {}", e)))?)
    }

    /// global orchard commitment position counter (increments for every action in every block)
    pub fn orchard_position(&self) -> Result<u64, Error> {
        match self.db.get(ORCHARD_POSITION_KEY)
            .map_err(|e| Error::Wallet(format!("read orchard position: {}", e)))? {
            Some(bytes) => {
                if bytes.len() == 8 {
                    Ok(u64::from_le_bytes(bytes.as_ref().try_into().expect("len checked")))
                } else {
                    Ok(0)
                }
            }
            None => Ok(0),
        }
    }

    pub fn set_orchard_position(&self, pos: u64) -> Result<(), Error> {
        self.db.insert(ORCHARD_POSITION_KEY, &pos.to_le_bytes())
            .map_err(|e| Error::Wallet(format!("write orchard position: {}", e)))?;
        Ok(())
    }

    /// get all unspent notes and total shielded balance
    pub fn shielded_balance(&self) -> Result<(u64, Vec<WalletNote>), Error> {
        let notes_tree = self.db.open_tree(NOTES_TREE)
            .map_err(|e| Error::Wallet(format!("open notes tree: {}", e)))?;

        let mut balance = 0u64;
        let mut unspent = Vec::new();

        for entry in notes_tree.iter() {
            let (_, value) = entry
                .map_err(|e| Error::Wallet(format!("iterate notes: {}", e)))?;
            let note: WalletNote = serde_json::from_slice(&value)
                .map_err(|e| Error::Wallet(format!("deserialize note: {}", e)))?;
            if !self.is_spent(&note.nullifier)? {
                balance += note.value;
                unspent.push(note);
            }
        }

        Ok((balance, unspent))
    }
}
