//! storage layer using NOMT for sparse merkle trees + sled for proof cache

use crate::error::{Result, ZidecarError};
use nomt::{
    hasher::Blake3Hasher,
    KeyReadWrite, Nomt, Options as NomtOptions, Root, SessionParams,
    proof::PathProofTerminal,
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
        // use 50% of CPU threads for NOMT (leaves room for other ops)
        let all_threads = std::thread::available_parallelism()
            .map(|p| p.get())
            .unwrap_or(8);
        let nomt_threads = (all_threads / 2).max(4);
        nomt_opts.commit_concurrency(nomt_threads);
        info!("nomt commit_concurrency: {} threads (50% of {})", nomt_threads, all_threads);

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

    // ===== NULLIFIER SYNC =====

    /// Set nullifier sync progress height
    pub fn set_nullifier_sync_height(&self, height: u32) -> Result<()> {
        self.sled
            .insert(b"nullifier_sync_height", &height.to_le_bytes())
            .map_err(|e| ZidecarError::Storage(format!("sled: {}", e)))?;
        Ok(())
    }

    /// Get nullifier sync progress height
    pub fn get_nullifier_sync_height(&self) -> Result<Option<u32>> {
        match self.sled.get(b"nullifier_sync_height") {
            Ok(Some(bytes)) if bytes.len() == 4 => {
                let height = u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]);
                Ok(Some(height))
            }
            Ok(_) => Ok(None),
            Err(e) => Err(ZidecarError::Storage(format!("sled: {}", e))),
        }
    }

    /// Get current nullifier set root from nomt
    pub fn get_nullifier_root(&self) -> [u8; 32] {
        root_to_bytes(&self.nomt.root())
    }

    /// Batch insert nullifiers with block height (for sync)
    pub fn batch_insert_nullifiers(&self, nullifiers: &[[u8; 32]], block_height: u32) -> Result<Root> {
        if nullifiers.is_empty() {
            return Ok(self.nomt.root());
        }

        let session = self.nomt.begin_session(SessionParams::default());

        let mut ops = Vec::with_capacity(nullifiers.len());

        // Value: block height as 4 bytes (allows lookup of when nullifier was revealed)
        let height_bytes = block_height.to_le_bytes().to_vec();

        for nullifier in nullifiers {
            let key = key_for_nullifier(nullifier);
            session.warm_up(key);
            ops.push((key, KeyReadWrite::Write(Some(height_bytes.clone()))));
        }

        // Sort by key (required by nomt)
        ops.sort_by_key(|(k, _)| *k);

        let finished = session
            .finish(ops)
            .map_err(|e| ZidecarError::Storage(format!("nomt finish: {:?}", e)))?;

        let root = finished.root();
        finished
            .commit(&self.nomt)
            .map_err(|e| ZidecarError::Storage(format!("nomt commit: {}", e)))?;

        Ok(root)
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

    // ===== HEADER CACHE =====

    /// store block header (hash + prev_hash + bits) by height
    pub fn store_header(&self, height: u32, hash: &str, prev_hash: &str, bits: &str) -> Result<()> {
        let mut key = Vec::with_capacity(5);
        key.push(b'h'); // header prefix
        key.extend_from_slice(&height.to_le_bytes());

        // store as "hash:prev_hash:bits"
        let value = format!("{}:{}:{}", hash, prev_hash, bits);
        self.sled
            .insert(key, value.as_bytes())
            .map_err(|e| ZidecarError::Storage(format!("sled: {}", e)))?;
        Ok(())
    }

    /// get cached header by height
    /// returns (hash, prev_hash, bits) - bits may be empty for old cache entries
    pub fn get_header(&self, height: u32) -> Result<Option<(String, String, String)>> {
        let mut key = Vec::with_capacity(5);
        key.push(b'h');
        key.extend_from_slice(&height.to_le_bytes());

        match self.sled.get(key) {
            Ok(Some(bytes)) => {
                let s = String::from_utf8_lossy(&bytes);
                let parts: Vec<&str> = s.splitn(3, ':').collect();
                match parts.len() {
                    2 => Ok(Some((parts[0].to_string(), parts[1].to_string(), String::new()))),
                    3 => Ok(Some((parts[0].to_string(), parts[1].to_string(), parts[2].to_string()))),
                    _ => Ok(None),
                }
            }
            Ok(None) => Ok(None),
            Err(e) => Err(ZidecarError::Storage(format!("sled: {}", e))),
        }
    }

    /// get highest cached header height
    pub fn get_max_cached_header_height(&self) -> Result<Option<u32>> {
        let prefix = vec![b'h'];
        for item in self.sled.scan_prefix(&prefix).rev() {
            if let Ok((key, _)) = item {
                if key.len() == 5 {
                    let height = u32::from_le_bytes([key[1], key[2], key[3], key[4]]);
                    return Ok(Some(height));
                }
            }
        }
        Ok(None)
    }

    /// batch store headers (with bits for PoW verification)
    pub fn store_headers_batch(&self, headers: &[(u32, String, String, String)]) -> Result<()> {
        let mut batch = sled::Batch::default();
        for (height, hash, prev_hash, bits) in headers {
            let mut key = Vec::with_capacity(5);
            key.push(b'h');
            key.extend_from_slice(&height.to_le_bytes());
            let value = format!("{}:{}:{}", hash, prev_hash, bits);
            batch.insert(key, value.as_bytes());
        }
        self.sled
            .apply_batch(batch)
            .map_err(|e| ZidecarError::Storage(format!("sled batch: {}", e)))?;
        Ok(())
    }

    // ===== CHECKPOINT STORAGE =====

    /// store FROST checkpoint
    pub fn store_checkpoint(&self, epoch: u64, checkpoint_bytes: &[u8]) -> Result<()> {
        let mut key = Vec::with_capacity(9);
        key.push(b'c'); // checkpoint prefix
        key.extend_from_slice(&epoch.to_le_bytes());
        self.sled
            .insert(key, checkpoint_bytes)
            .map_err(|e| ZidecarError::Storage(format!("sled: {}", e)))?;
        Ok(())
    }

    /// get FROST checkpoint by epoch
    pub fn get_checkpoint(&self, epoch: u64) -> Result<Option<Vec<u8>>> {
        let mut key = Vec::with_capacity(9);
        key.push(b'c');
        key.extend_from_slice(&epoch.to_le_bytes());
        self.sled
            .get(key)
            .map(|v| v.map(|iv| iv.to_vec()))
            .map_err(|e| ZidecarError::Storage(format!("sled: {}", e)))
    }

    /// get latest checkpoint epoch
    pub fn get_latest_checkpoint_epoch(&self) -> Result<Option<u64>> {
        // scan checkpoints in reverse to find latest
        let prefix = vec![b'c'];
        for item in self.sled.scan_prefix(&prefix).rev() {
            if let Ok((key, _)) = item {
                if key.len() == 9 {
                    let epoch = u64::from_le_bytes([
                        key[1], key[2], key[3], key[4],
                        key[5], key[6], key[7], key[8],
                    ]);
                    return Ok(Some(epoch));
                }
            }
        }
        Ok(None)
    }

    // ===== STATE ROOT TRACKING =====

    /// store state roots at height
    pub fn store_state_roots(
        &self,
        height: u32,
        tree_root: &[u8; 32],
        nullifier_root: &[u8; 32],
    ) -> Result<()> {
        let mut key = Vec::with_capacity(5);
        key.push(b'r'); // roots prefix
        key.extend_from_slice(&height.to_le_bytes());

        let mut value = Vec::with_capacity(64);
        value.extend_from_slice(tree_root);
        value.extend_from_slice(nullifier_root);

        self.sled
            .insert(key, value)
            .map_err(|e| ZidecarError::Storage(format!("sled: {}", e)))?;
        Ok(())
    }

    /// get state roots at height
    pub fn get_state_roots(&self, height: u32) -> Result<Option<([u8; 32], [u8; 32])>> {
        let mut key = Vec::with_capacity(5);
        key.push(b'r');
        key.extend_from_slice(&height.to_le_bytes());

        match self.sled.get(key) {
            Ok(Some(bytes)) if bytes.len() == 64 => {
                let mut tree_root = [0u8; 32];
                let mut nullifier_root = [0u8; 32];
                tree_root.copy_from_slice(&bytes[..32]);
                nullifier_root.copy_from_slice(&bytes[32..]);
                Ok(Some((tree_root, nullifier_root)))
            }
            Ok(_) => Ok(None),
            Err(e) => Err(ZidecarError::Storage(format!("sled: {}", e))),
        }
    }

    /// get highest height with stored state roots
    pub fn get_latest_state_height(&self) -> Result<Option<u32>> {
        let prefix = vec![b'r'];
        for item in self.sled.scan_prefix(&prefix).rev() {
            if let Ok((key, _)) = item {
                if key.len() == 5 {
                    let height = u32::from_le_bytes([key[1], key[2], key[3], key[4]]);
                    return Ok(Some(height));
                }
            }
        }
        Ok(None)
    }

    // ===== ACTION COUNT TRACKING =====

    /// increment total action count and return new total
    pub fn increment_action_count(&self, count: u64) -> Result<u64> {
        let current = self.get_action_count()?;
        let new_total = current + count;
        self.sled
            .insert(b"total_actions", &new_total.to_le_bytes())
            .map_err(|e| ZidecarError::Storage(format!("sled: {}", e)))?;
        Ok(new_total)
    }

    /// get total action count
    pub fn get_action_count(&self) -> Result<u64> {
        match self.sled.get(b"total_actions") {
            Ok(Some(bytes)) if bytes.len() == 8 => {
                Ok(u64::from_le_bytes([
                    bytes[0], bytes[1], bytes[2], bytes[3],
                    bytes[4], bytes[5], bytes[6], bytes[7],
                ]))
            }
            Ok(_) => Ok(0),
            Err(e) => Err(ZidecarError::Storage(format!("sled: {}", e))),
        }
    }

    // ===== COMMITMENT POSITION TRACKING =====

    /// store commitment position in tree
    pub fn store_commitment_position(&self, cmx: &[u8; 32], position: u64) -> Result<()> {
        let mut key = Vec::with_capacity(33);
        key.push(b'p'); // position prefix
        key.extend_from_slice(cmx);
        self.sled
            .insert(key, &position.to_le_bytes())
            .map_err(|e| ZidecarError::Storage(format!("sled: {}", e)))?;
        Ok(())
    }

    /// get commitment position in tree
    pub fn get_commitment_position(&self, cmx: &[u8; 32]) -> Result<Option<u64>> {
        let mut key = Vec::with_capacity(33);
        key.push(b'p');
        key.extend_from_slice(cmx);
        match self.sled.get(key) {
            Ok(Some(bytes)) if bytes.len() == 8 => {
                Ok(Some(u64::from_le_bytes([
                    bytes[0], bytes[1], bytes[2], bytes[3],
                    bytes[4], bytes[5], bytes[6], bytes[7],
                ])))
            }
            Ok(_) => Ok(None),
            Err(e) => Err(ZidecarError::Storage(format!("sled: {}", e))),
        }
    }

    // ===== GIGAPROOF METADATA =====

    /// store the epoch that the current gigaproof covers up to
    pub fn set_gigaproof_epoch(&self, epoch: u32) -> Result<()> {
        self.sled
            .insert(b"gigaproof_epoch", &epoch.to_le_bytes())
            .map_err(|e| ZidecarError::Storage(format!("sled: {}", e)))?;
        Ok(())
    }

    /// get the epoch that the current gigaproof covers up to
    pub fn get_gigaproof_epoch(&self) -> Result<Option<u32>> {
        match self.sled.get(b"gigaproof_epoch") {
            Ok(Some(bytes)) if bytes.len() == 4 => {
                let epoch = u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]);
                Ok(Some(epoch))
            }
            Ok(_) => Ok(None),
            Err(e) => Err(ZidecarError::Storage(format!("sled: {}", e))),
        }
    }

    /// store the start height for gigaproof
    pub fn set_gigaproof_start(&self, height: u32) -> Result<()> {
        self.sled
            .insert(b"gigaproof_start", &height.to_le_bytes())
            .map_err(|e| ZidecarError::Storage(format!("sled: {}", e)))?;
        Ok(())
    }

    /// get the start height for gigaproof
    pub fn get_gigaproof_start(&self) -> Result<Option<u32>> {
        match self.sled.get(b"gigaproof_start") {
            Ok(Some(bytes)) if bytes.len() == 4 => {
                let height = u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]);
                Ok(Some(height))
            }
            Ok(_) => Ok(None),
            Err(e) => Err(ZidecarError::Storage(format!("sled: {}", e))),
        }
    }

    // ===== EPOCH BOUNDARY HASHES =====
    // Critical for proof verification: client must know first/last hash of each epoch
    // to verify chain continuity between proofs

    /// store epoch boundary hashes (first block hash, last block hash)
    /// Used to verify:
    /// 1. Gigaproof starts from known checkpoint
    /// 2. Each epoch's last_hash.prev = this epoch's content
    /// 3. Next epoch's first_prev_hash = this epoch's last_hash
    pub fn store_epoch_boundary(
        &self,
        epoch: u32,
        first_height: u32,
        first_hash: &[u8; 32],
        first_prev_hash: &[u8; 32],
        last_height: u32,
        last_hash: &[u8; 32],
    ) -> Result<()> {
        let mut key = Vec::with_capacity(5);
        key.push(b'e'); // epoch boundary prefix
        key.extend_from_slice(&epoch.to_le_bytes());

        // value: first_height(4) + first_hash(32) + first_prev_hash(32) + last_height(4) + last_hash(32) = 104 bytes
        let mut value = Vec::with_capacity(104);
        value.extend_from_slice(&first_height.to_le_bytes());
        value.extend_from_slice(first_hash);
        value.extend_from_slice(first_prev_hash);
        value.extend_from_slice(&last_height.to_le_bytes());
        value.extend_from_slice(last_hash);

        self.sled
            .insert(key, value)
            .map_err(|e| ZidecarError::Storage(format!("sled: {}", e)))?;

        debug!(
            "stored epoch {} boundary: first={}@{} last={}@{}",
            epoch,
            hex::encode(&first_hash[..4]),
            first_height,
            hex::encode(&last_hash[..4]),
            last_height
        );

        Ok(())
    }

    /// get epoch boundary hashes
    /// Returns: (first_height, first_hash, first_prev_hash, last_height, last_hash)
    pub fn get_epoch_boundary(&self, epoch: u32) -> Result<Option<EpochBoundary>> {
        let mut key = Vec::with_capacity(5);
        key.push(b'e');
        key.extend_from_slice(&epoch.to_le_bytes());

        match self.sled.get(key) {
            Ok(Some(bytes)) if bytes.len() == 104 => {
                let first_height = u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]);
                let mut first_hash = [0u8; 32];
                first_hash.copy_from_slice(&bytes[4..36]);
                let mut first_prev_hash = [0u8; 32];
                first_prev_hash.copy_from_slice(&bytes[36..68]);
                let last_height = u32::from_le_bytes([bytes[68], bytes[69], bytes[70], bytes[71]]);
                let mut last_hash = [0u8; 32];
                last_hash.copy_from_slice(&bytes[72..104]);

                Ok(Some(EpochBoundary {
                    epoch,
                    first_height,
                    first_hash,
                    first_prev_hash,
                    last_height,
                    last_hash,
                }))
            }
            Ok(_) => Ok(None),
            Err(e) => Err(ZidecarError::Storage(format!("sled: {}", e))),
        }
    }

    /// verify epoch chain continuity (prev epoch's last_hash == this epoch's first_prev_hash)
    pub fn verify_epoch_continuity(&self, epoch: u32) -> Result<bool> {
        if epoch == 0 {
            return Ok(true); // genesis epoch has no previous
        }

        let prev_boundary = self.get_epoch_boundary(epoch - 1)?;
        let this_boundary = self.get_epoch_boundary(epoch)?;

        match (prev_boundary, this_boundary) {
            (Some(prev), Some(this)) => {
                // this epoch's first block must have prev_hash = previous epoch's last block hash
                Ok(prev.last_hash == this.first_prev_hash)
            }
            _ => Ok(false), // missing boundary data
        }
    }

    // ===== PROOF GENERATION (NOMT) =====

    /// generate nullifier proof (membership or non-membership)
    pub fn generate_nullifier_proof(&self, nullifier: &[u8; 32]) -> Result<NomtProof> {
        let session = self.nomt.begin_session(SessionParams::default());
        let key = key_for_nullifier(nullifier);

        // warm up the key for proof generation
        session.warm_up(key);

        // generate merkle proof using NOMT's prove method
        let path_proof = session
            .prove(key)
            .map_err(|e| ZidecarError::Storage(format!("nomt prove: {}", e)))?;

        // check if value exists (leaf vs terminator)
        let exists = matches!(&path_proof.terminal, PathProofTerminal::Leaf(leaf) if leaf.key_path == key);

        // get root
        let root = self.nomt.root();

        // extract sibling hashes from proof
        let path: Vec<[u8; 32]> = path_proof.siblings.clone();

        // extract path indices from key (bit path through tree)
        // indices indicate which side (left=false, right=true) the node is on
        let indices: Vec<bool> = key_to_bits(&key, path_proof.siblings.len());

        debug!(
            "generated nullifier proof: {} siblings, exists={}",
            path.len(),
            exists
        );

        Ok(NomtProof {
            key,
            root: root_to_bytes(&root),
            exists,
            path,
            indices,
        })
    }

    /// generate commitment proof (note in tree)
    pub fn generate_commitment_proof(&self, cmx: &[u8; 32]) -> Result<NomtProof> {
        let session = self.nomt.begin_session(SessionParams::default());
        let key = key_for_note(cmx);

        // warm up the key for proof generation
        session.warm_up(key);

        // generate merkle proof using NOMT's prove method
        let path_proof = session
            .prove(key)
            .map_err(|e| ZidecarError::Storage(format!("nomt prove: {}", e)))?;

        // check if value exists (leaf vs terminator)
        let exists = matches!(&path_proof.terminal, PathProofTerminal::Leaf(leaf) if leaf.key_path == key);

        let root = self.nomt.root();

        // extract sibling hashes from proof
        let path: Vec<[u8; 32]> = path_proof.siblings.clone();

        // extract path indices from key (bit path through tree)
        let indices: Vec<bool> = key_to_bits(&key, path_proof.siblings.len());

        debug!(
            "generated commitment proof: {} siblings, exists={}",
            path.len(),
            exists
        );

        Ok(NomtProof {
            key,
            root: root_to_bytes(&root),
            exists,
            path,
            indices,
        })
    }
}

/// Extract MSB-first bits from a 32-byte key
fn key_to_bits(key: &[u8; 32], count: usize) -> Vec<bool> {
    let mut bits = Vec::with_capacity(count);
    for i in 0..count {
        let byte_idx = i / 8;
        let bit_idx = 7 - (i % 8); // MSB first
        if byte_idx < 32 {
            bits.push((key[byte_idx] >> bit_idx) & 1 == 1);
        } else {
            bits.push(false);
        }
    }
    bits
}

/// NOMT sparse merkle proof
#[derive(Debug, Clone)]
pub struct NomtProof {
    pub key: [u8; 32],
    pub root: [u8; 32],
    pub exists: bool,
    pub path: Vec<[u8; 32]>,
    pub indices: Vec<bool>,
}

/// Epoch boundary information for chain continuity verification
#[derive(Debug, Clone)]
pub struct EpochBoundary {
    pub epoch: u32,
    /// First block height in epoch
    pub first_height: u32,
    /// First block hash
    pub first_hash: [u8; 32],
    /// First block's prev_hash (links to previous epoch)
    pub first_prev_hash: [u8; 32],
    /// Last block height in epoch
    pub last_height: u32,
    /// Last block hash
    pub last_hash: [u8; 32],
}

/// convert nomt Root to bytes
fn root_to_bytes(root: &Root) -> [u8; 32] {
    let mut bytes = [0u8; 32];
    bytes.copy_from_slice(root.as_ref());
    bytes
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
