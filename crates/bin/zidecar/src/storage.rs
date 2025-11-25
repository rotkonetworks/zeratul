//! storage layer using NOMT for sparse merkle trees + sled for proof cache

use crate::error::{Result, ZidecarError};
use nomt::{
    hasher::Blake3Hasher,
    KeyReadWrite, Nomt, Options as NomtOptions, Root, SessionParams,
};
use sha2::{Digest, Sha256};
use tracing::{info, debug};

/// storage combining NOMT (for merkle state) and sled (for proof cache)
pub struct Storage {
    /// nomt database for sparse merkle trees
    nomt: Nomt<Blake3Hasher>,
    /// sled for simple key-value (proof cache)
    sled: sled::Db,
}

impl Storage {
    pub fn open(path: &str) -> Result<Self> {
        info!("opening storage at {}", path);

        // open nomt for merkle state
        let mut nomt_opts = NomtOptions::new();
        nomt_opts.path(format!("{}/nomt", path));
        nomt_opts.commit_concurrency(1);

        let nomt = Nomt::<Blake3Hasher>::open(nomt_opts)
            .map_err(|e| ZidecarError::Storage(format!("nomt: {}", e)))?;

        info!("opened nomt database");

        // open sled for proof cache
        let sled = sled::open(format!("{}/sled", path))
            .map_err(|e| ZidecarError::Storage(format!("sled: {}", e)))?;

        Ok(Self { nomt, sled })
    }

    /// get current merkle root
    pub fn root(&self) -> Root {
        self.nomt.root()
    }

    /// store proof for height range (simple cache)
    pub fn store_proof(
        &self,
        from_height: u32,
        to_height: u32,
        proof_bytes: &[u8],
    ) -> Result<()> {
        let key = proof_key(from_height, to_height);
        self.sled
            .insert(key, proof_bytes)
            .map_err(|e| ZidecarError::Storage(format!("sled: {}", e)))?;
        Ok(())
    }

    /// get proof for height range
    pub fn get_proof(&self, from_height: u32, to_height: u32) -> Result<Option<Vec<u8>>> {
        let key = proof_key(from_height, to_height);
        self.sled
            .get(key)
            .map(|v| v.map(|iv| iv.to_vec()))
            .map_err(|e| ZidecarError::Storage(format!("sled: {}", e)))
    }

    /// store last proven height
    pub fn set_last_proven_height(&self, height: u32) -> Result<()> {
        self.sled
            .insert(b"last_proven_height", &height.to_le_bytes())
            .map_err(|e| ZidecarError::Storage(format!("sled: {}", e)))?;
        Ok(())
    }

    /// get last proven height
    pub fn get_last_proven_height(&self) -> Result<Option<u32>> {
        match self.sled.get(b"last_proven_height") {
            Ok(Some(bytes)) if bytes.len() == 4 => {
                let height = u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]);
                Ok(Some(height))
            }
            Ok(_) => Ok(None),
            Err(e) => Err(ZidecarError::Storage(format!("sled: {}", e))),
        }
    }

    /// insert nullifier into sparse merkle tree
    /// returns new root after insertion
    pub fn insert_nullifier(&self, nullifier: &[u8; 32]) -> Result<Root> {
        let session = self.nomt.begin_session(SessionParams::default());

        // key = hash("nullifier" || nullifier)
        let key = key_for_nullifier(nullifier);

        // value = [1] (spent)
        let value = vec![1u8];

        // warm up for write
        session.warm_up(key);

        // commit write
        let finished = session
            .finish(vec![(key, KeyReadWrite::Write(Some(value)))])
            .map_err(|e| ZidecarError::Storage(format!("nomt finish: {:?}", e)))?;

        let root = finished.root();
        finished
            .commit(&self.nomt)
            .map_err(|e| ZidecarError::Storage(format!("nomt commit: {}", e)))?;

        debug!("inserted nullifier");

        Ok(root)
    }

    /// check if nullifier exists in tree
    pub fn has_nullifier(&self, nullifier: &[u8; 32]) -> Result<bool> {
        let session = self.nomt.begin_session(SessionParams::default());
        let key = key_for_nullifier(nullifier);

        let value = session
            .read(key)
            .map_err(|e| ZidecarError::Storage(format!("nomt read: {}", e)))?;

        Ok(value.is_some())
    }

    /// insert owned note into sparse merkle tree
    /// value = serialized NoteData
    pub fn insert_note(&self, note_commitment: &[u8; 32], note_data: &[u8]) -> Result<Root> {
        let session = self.nomt.begin_session(SessionParams::default());

        let key = key_for_note(note_commitment);
        let value = note_data.to_vec();

        session.warm_up(key);

        let finished = session
            .finish(vec![(key, KeyReadWrite::Write(Some(value)))])
            .map_err(|e| ZidecarError::Storage(format!("nomt finish: {:?}", e)))?;

        let root = finished.root();
        finished
            .commit(&self.nomt)
            .map_err(|e| ZidecarError::Storage(format!("nomt commit: {}", e)))?;

        Ok(root)
    }

    /// read note data
    pub fn get_note(&self, note_commitment: &[u8; 32]) -> Result<Option<Vec<u8>>> {
        let session = self.nomt.begin_session(SessionParams::default());
        let key = key_for_note(note_commitment);

        session
            .read(key)
            .map_err(|e| ZidecarError::Storage(format!("nomt read: {}", e)))
    }

    /// batch insert (nullifiers + notes) - more efficient
    pub fn batch_insert(
        &self,
        nullifiers: &[[u8; 32]],
        notes: &[([u8; 32], Vec<u8>)],
    ) -> Result<Root> {
        let session = self.nomt.begin_session(SessionParams::default());

        let mut ops = Vec::new();

        // prepare nullifier inserts
        for nullifier in nullifiers {
            let key = key_for_nullifier(nullifier);
            session.warm_up(key);
            ops.push((key, KeyReadWrite::Write(Some(vec![1u8]))));
        }

        // prepare note inserts
        for (cmx, data) in notes {
            let key = key_for_note(cmx);
            session.warm_up(key);
            ops.push((key, KeyReadWrite::Write(Some(data.clone()))));
        }

        // sort by key (required by nomt)
        ops.sort_by_key(|(k, _)| *k);

        let finished = session
            .finish(ops)
            .map_err(|e| ZidecarError::Storage(format!("nomt finish: {:?}", e)))?;

        let root = finished.root();
        finished
            .commit(&self.nomt)
            .map_err(|e| ZidecarError::Storage(format!("nomt commit: {}", e)))?;

        info!(
            "batch inserted {} nullifiers + {} notes",
            nullifiers.len(),
            notes.len()
        );

        Ok(root)
    }
}

/// proof cache key
fn proof_key(from_height: u32, to_height: u32) -> Vec<u8> {
    let mut key = Vec::with_capacity(9);
    key.push(b'p'); // prefix
    key.extend_from_slice(&from_height.to_le_bytes());
    key.extend_from_slice(&to_height.to_le_bytes());
    key
}

/// nomt key for nullifier (domain-separated hash)
fn key_for_nullifier(nullifier: &[u8; 32]) -> [u8; 32] {
    let mut hasher = Sha256::new();
    hasher.update(b"zidecar:nullifier:");
    hasher.update(nullifier);
    hasher.finalize().into()
}

/// nomt key for note commitment (domain-separated hash)
fn key_for_note(cmx: &[u8; 32]) -> [u8; 32] {
    let mut hasher = Sha256::new();
    hasher.update(b"zidecar:note:");
    hasher.update(cmx);
    hasher.finalize().into()
}

// storage error wrapper
impl From<String> for ZidecarError {
    fn from(s: String) -> Self {
        ZidecarError::Storage(s)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn test_storage_proof_roundtrip() {
        let dir = tempdir().unwrap();
        let storage = Storage::open(dir.path().to_str().unwrap()).unwrap();

        let proof = b"fake proof data";
        storage.store_proof(100, 200, proof).unwrap();

        let loaded = storage.get_proof(100, 200).unwrap();
        assert_eq!(loaded, Some(proof.to_vec()));

        let not_found = storage.get_proof(300, 400).unwrap();
        assert_eq!(not_found, None);
    }

    #[test]
    fn test_storage_nullifier() {
        let dir = tempdir().unwrap();
        let storage = Storage::open(dir.path().to_str().unwrap()).unwrap();

        let nullifier = [42u8; 32];

        // not present initially
        assert!(!storage.has_nullifier(&nullifier).unwrap());

        // insert
        let root1 = storage.insert_nullifier(&nullifier).unwrap();
        assert!(!root1.is_empty());

        // now present
        assert!(storage.has_nullifier(&nullifier).unwrap());

        // insert another
        let nullifier2 = [99u8; 32];
        let root2 = storage.insert_nullifier(&nullifier2).unwrap();
        assert_ne!(root1, root2);

        // both present
        assert!(storage.has_nullifier(&nullifier).unwrap());
        assert!(storage.has_nullifier(&nullifier2).unwrap());
    }

    #[test]
    fn test_storage_notes() {
        let dir = tempdir().unwrap();
        let storage = Storage::open(dir.path().to_str().unwrap()).unwrap();

        let cmx = [1u8; 32];
        let data = b"note data here";

        // not present
        assert!(storage.get_note(&cmx).unwrap().is_none());

        // insert
        storage.insert_note(&cmx, data).unwrap();

        // now present
        let loaded = storage.get_note(&cmx).unwrap();
        assert_eq!(loaded, Some(data.to_vec()));
    }

    #[test]
    fn test_storage_batch_insert() {
        let dir = tempdir().unwrap();
        let storage = Storage::open(dir.path().to_str().unwrap()).unwrap();

        let nullifiers = vec![[1u8; 32], [2u8; 32], [3u8; 32]];
        let notes = vec![
            ([10u8; 32], b"note1".to_vec()),
            ([20u8; 32], b"note2".to_vec()),
        ];

        let root = storage.batch_insert(&nullifiers, &notes).unwrap();
        assert!(!root.is_empty());

        // verify all inserted
        for nf in &nullifiers {
            assert!(storage.has_nullifier(nf).unwrap());
        }
        for (cmx, _) in &notes {
            assert!(storage.get_note(cmx).unwrap().is_some());
        }
    }
}
