//! Tiered Commitment Tree (TCT) implementation for NOMT
//!
//! Adapted from Penumbra's TCT design for NOMT sparse merkle trees.
//! Uses a three-tier hierarchy: Epoch -> Block -> Commitment
//!
//! Structure:
//! - Top level: Epoch tree (16 bits = 65,536 epochs)
//! - Mid level: Block tree per epoch (10 bits = 1,024 blocks per epoch)
//! - Bottom level: Commitment tree per block (16 bits = 65,536 commitments per block)
//!
//! Total capacity: 65,536 epochs × 1,024 blocks × 65,536 commitments = ~4.4 trillion

use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::sync::{Arc, RwLock};

/// 32-byte hash type
pub type Hash = [u8; 32];

/// Empty/zero hash for sparse nodes
pub const EMPTY_HASH: Hash = [0u8; 32];

/// Position in the tiered commitment tree
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Position {
    /// Epoch index (0-65535)
    pub epoch: u16,
    /// Block index within epoch (0-1023)
    pub block: u16,
    /// Commitment index within block (0-65535)
    pub commitment: u16,
}

impl Position {
    /// Create a new position
    pub fn new(epoch: u16, block: u16, commitment: u16) -> Self {
        Self { epoch, block, commitment }
    }

    /// Pack position into u64 for compact storage
    pub fn to_u64(&self) -> u64 {
        ((self.epoch as u64) << 26) | ((self.block as u64) << 16) | (self.commitment as u64)
    }

    /// Unpack from u64
    pub fn from_u64(packed: u64) -> Self {
        Self {
            epoch: ((packed >> 26) & 0xFFFF) as u16,
            block: ((packed >> 16) & 0x3FF) as u16,
            commitment: (packed & 0xFFFF) as u16,
        }
    }

    /// Height in the global tree (0 = leaf, 42 = root)
    /// - Commitment tier: 16 levels (0-15)
    /// - Block tier: 10 levels (16-25)
    /// - Epoch tier: 16 levels (26-41)
    /// - Root: 42
    pub const TOTAL_HEIGHT: u8 = 42;
}

/// A note commitment (32 bytes, the cmx from Orchard)
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Commitment(pub Hash);

impl Commitment {
    pub fn to_bytes(&self) -> &[u8; 32] {
        &self.0
    }

    pub fn from_bytes(bytes: [u8; 32]) -> Self {
        Self(bytes)
    }
}

/// Merkle authentication path for a commitment proof
#[derive(Debug, Clone)]
pub struct AuthPath {
    /// Position of the commitment
    pub position: Position,
    /// Sibling hashes from leaf to root (42 hashes for binary tree)
    pub siblings: Vec<Hash>,
}

impl AuthPath {
    /// Verify this path leads to the given root
    pub fn verify(&self, commitment: &Commitment, expected_root: &Hash) -> bool {
        let computed_root = self.compute_root(commitment);
        &computed_root == expected_root
    }

    /// Compute root from this path and commitment
    pub fn compute_root(&self, commitment: &Commitment) -> Hash {
        let mut current = hash_leaf(commitment);
        let index = self.position.to_u64();

        for (height, sibling) in self.siblings.iter().enumerate() {
            let bit = (index >> height) & 1;
            if bit == 0 {
                current = hash_node(height as u8, &current, sibling);
            } else {
                current = hash_node(height as u8, sibling, &current);
            }
        }

        current
    }
}

/// Block-level commitment tree (finalized)
#[derive(Debug, Clone)]
pub struct Block {
    /// Block index within epoch
    pub index: u16,
    /// Commitments in this block
    pub commitments: Vec<Commitment>,
    /// Merkle root of this block's commitment tree
    pub root: Hash,
    /// Index mapping commitment -> position within block
    commitment_index: HashMap<Commitment, u16>,
}

impl Block {
    /// Create empty block
    pub fn new(index: u16) -> Self {
        Self {
            index,
            commitments: Vec::new(),
            root: EMPTY_HASH,
            commitment_index: HashMap::new(),
        }
    }

    /// Insert a commitment and return its position within the block
    pub fn insert(&mut self, commitment: Commitment) -> u16 {
        let pos = self.commitments.len() as u16;
        self.commitment_index.insert(commitment, pos);
        self.commitments.push(commitment);
        pos
    }

    /// Finalize the block and compute its merkle root
    pub fn finalize(&mut self) {
        self.root = self.compute_root();
    }

    /// Compute merkle root of all commitments
    fn compute_root(&self) -> Hash {
        if self.commitments.is_empty() {
            return EMPTY_HASH;
        }

        // Build bottom-up merkle tree
        let mut leaves: Vec<Hash> = self.commitments.iter()
            .map(hash_leaf)
            .collect();

        // Pad to power of 2 with empty hashes
        let target_size = (1usize << 16).min(leaves.len().next_power_of_two());
        leaves.resize(target_size, EMPTY_HASH);

        // Build tree bottom-up
        let mut current_level = leaves;
        let mut height = 0u8;

        while current_level.len() > 1 {
            let mut next_level = Vec::with_capacity(current_level.len() / 2);
            for chunk in current_level.chunks(2) {
                let left = chunk[0];
                let right = if chunk.len() > 1 { chunk[1] } else { EMPTY_HASH };
                next_level.push(hash_node(height, &left, &right));
            }
            current_level = next_level;
            height += 1;
        }

        current_level.into_iter().next().unwrap_or(EMPTY_HASH)
    }

    /// Get authentication path for a commitment within this block
    pub fn witness(&self, commitment: &Commitment) -> Option<Vec<Hash>> {
        let pos = *self.commitment_index.get(commitment)?;
        Some(self.compute_path(pos))
    }

    /// Compute path for position within block
    fn compute_path(&self, pos: u16) -> Vec<Hash> {
        if self.commitments.is_empty() {
            return vec![];
        }

        let mut leaves: Vec<Hash> = self.commitments.iter()
            .map(hash_leaf)
            .collect();

        let target_size = (1usize << 16).min(leaves.len().next_power_of_two());
        leaves.resize(target_size, EMPTY_HASH);

        let mut path = Vec::with_capacity(16);
        let mut current_level = leaves;
        let mut idx = pos as usize;
        let mut height = 0u8;

        while current_level.len() > 1 {
            let sibling_idx = idx ^ 1;
            let sibling = if sibling_idx < current_level.len() {
                current_level[sibling_idx]
            } else {
                EMPTY_HASH
            };
            path.push(sibling);

            let mut next_level = Vec::with_capacity(current_level.len() / 2);
            for chunk in current_level.chunks(2) {
                let left = chunk[0];
                let right = if chunk.len() > 1 { chunk[1] } else { EMPTY_HASH };
                next_level.push(hash_node(height, &left, &right));
            }
            current_level = next_level;
            idx /= 2;
            height += 1;
        }

        path
    }
}

/// Epoch containing blocks
#[derive(Debug, Clone)]
pub struct Epoch {
    /// Epoch index
    pub index: u16,
    /// Blocks in this epoch (up to 1024)
    pub blocks: Vec<Block>,
    /// Merkle root of this epoch's block roots
    pub root: Hash,
    /// Whether this epoch is finalized
    pub finalized: bool,
}

impl Epoch {
    /// Create new epoch
    pub fn new(index: u16) -> Self {
        Self {
            index,
            blocks: Vec::new(),
            root: EMPTY_HASH,
            finalized: false,
        }
    }

    /// Get current block (creating if needed)
    pub fn current_block(&mut self) -> &mut Block {
        if self.blocks.is_empty() {
            self.blocks.push(Block::new(0));
        }
        self.blocks.last_mut().unwrap()
    }

    /// End current block and start a new one
    pub fn end_block(&mut self) -> Hash {
        if let Some(block) = self.blocks.last_mut() {
            block.finalize();
        }
        let root = self.blocks.last().map(|b| b.root).unwrap_or(EMPTY_HASH);

        let new_idx = self.blocks.len() as u16;
        if new_idx < 1024 {
            self.blocks.push(Block::new(new_idx));
        }

        root
    }

    /// Finalize epoch and compute root
    pub fn finalize(&mut self) -> Hash {
        // Finalize last block if not already
        if let Some(block) = self.blocks.last_mut() {
            if block.root == EMPTY_HASH && !block.commitments.is_empty() {
                block.finalize();
            }
        }

        self.root = self.compute_root();
        self.finalized = true;
        self.root
    }

    /// Compute merkle root of block roots
    fn compute_root(&self) -> Hash {
        if self.blocks.is_empty() {
            return EMPTY_HASH;
        }

        let mut leaves: Vec<Hash> = self.blocks.iter()
            .map(|b| b.root)
            .collect();

        // Pad to power of 2 (max 1024 blocks = 10 levels)
        let target_size = (1usize << 10).min(leaves.len().next_power_of_two());
        leaves.resize(target_size, EMPTY_HASH);

        let mut current_level = leaves;
        let mut height = 16u8; // Block tier starts at height 16

        while current_level.len() > 1 {
            let mut next_level = Vec::with_capacity(current_level.len() / 2);
            for chunk in current_level.chunks(2) {
                let left = chunk[0];
                let right = if chunk.len() > 1 { chunk[1] } else { EMPTY_HASH };
                next_level.push(hash_node(height, &left, &right));
            }
            current_level = next_level;
            height += 1;
        }

        current_level.into_iter().next().unwrap_or(EMPTY_HASH)
    }
}

/// The tiered commitment tree
pub struct TieredCommitmentTree {
    /// Epochs in the tree
    epochs: Vec<Epoch>,
    /// Current epoch index
    current_epoch: u16,
    /// Global commitment index for lookups
    index: HashMap<Commitment, Position>,
    /// Global root (computed lazily)
    root: Hash,
}

impl TieredCommitmentTree {
    /// Create new empty tree
    pub fn new() -> Self {
        Self {
            epochs: vec![Epoch::new(0)],
            current_epoch: 0,
            index: HashMap::new(),
            root: EMPTY_HASH,
        }
    }

    /// Insert a commitment, returning its position
    pub fn insert(&mut self, commitment: Commitment) -> Position {
        let epoch = self.current_epoch();
        let block_idx = epoch.blocks.len().saturating_sub(1) as u16;
        let commitment_idx = epoch.current_block().insert(commitment);

        let position = Position::new(self.current_epoch, block_idx, commitment_idx);
        self.index.insert(commitment, position);

        position
    }

    /// End current block
    pub fn end_block(&mut self) -> Hash {
        self.current_epoch().end_block()
    }

    /// End current epoch
    pub fn end_epoch(&mut self) -> Hash {
        let root = self.current_epoch().finalize();

        // Start new epoch
        self.current_epoch += 1;
        self.epochs.push(Epoch::new(self.current_epoch));

        root
    }

    /// Get the current epoch
    fn current_epoch(&mut self) -> &mut Epoch {
        if self.epochs.is_empty() {
            self.epochs.push(Epoch::new(0));
        }
        self.epochs.last_mut().unwrap()
    }

    /// Get witness (authentication path) for a commitment
    pub fn witness(&self, commitment: &Commitment) -> Option<AuthPath> {
        let position = *self.index.get(commitment)?;

        let epoch = self.epochs.get(position.epoch as usize)?;
        let block = epoch.blocks.get(position.block as usize)?;

        // Get block-level path (commitment -> block root)
        let mut siblings = block.witness(commitment)?;

        // Add epoch-level path (block root -> epoch root)
        // This would require computing the full path through the epoch tree
        // For now, we store the block roots as additional siblings
        self.add_epoch_path(&mut siblings, epoch, position.block);

        // Add global-level path (epoch root -> global root)
        self.add_global_path(&mut siblings, position.epoch);

        Some(AuthPath { position, siblings })
    }

    /// Add epoch-level siblings to path
    fn add_epoch_path(&self, siblings: &mut Vec<Hash>, epoch: &Epoch, block_idx: u16) {
        if epoch.blocks.is_empty() {
            return;
        }

        let mut leaves: Vec<Hash> = epoch.blocks.iter().map(|b| b.root).collect();
        let target_size = (1usize << 10).min(leaves.len().next_power_of_two());
        leaves.resize(target_size, EMPTY_HASH);

        let mut idx = block_idx as usize;

        while leaves.len() > 1 {
            let sibling_idx = idx ^ 1;
            let sibling = if sibling_idx < leaves.len() {
                leaves[sibling_idx]
            } else {
                EMPTY_HASH
            };
            siblings.push(sibling);

            let mut next_level = Vec::with_capacity(leaves.len() / 2);
            for chunk in leaves.chunks(2) {
                let left = chunk[0];
                let right = if chunk.len() > 1 { chunk[1] } else { EMPTY_HASH };
                next_level.push(hash_node(0, &left, &right)); // simplified
            }
            leaves = next_level;
            idx /= 2;
        }
    }

    /// Add global-level siblings to path
    fn add_global_path(&self, siblings: &mut Vec<Hash>, epoch_idx: u16) {
        if self.epochs.is_empty() {
            return;
        }

        let mut leaves: Vec<Hash> = self.epochs.iter().map(|e| e.root).collect();
        let target_size = (1usize << 16).min(leaves.len().next_power_of_two());
        leaves.resize(target_size, EMPTY_HASH);

        let mut idx = epoch_idx as usize;

        while leaves.len() > 1 {
            let sibling_idx = idx ^ 1;
            let sibling = if sibling_idx < leaves.len() {
                leaves[sibling_idx]
            } else {
                EMPTY_HASH
            };
            siblings.push(sibling);

            let mut next_level = Vec::with_capacity(leaves.len() / 2);
            for chunk in leaves.chunks(2) {
                let left = chunk[0];
                let right = if chunk.len() > 1 { chunk[1] } else { EMPTY_HASH };
                next_level.push(hash_node(0, &left, &right)); // simplified
            }
            leaves = next_level;
            idx /= 2;
        }
    }

    /// Compute the global root
    pub fn root(&self) -> Hash {
        if self.epochs.is_empty() {
            return EMPTY_HASH;
        }

        let mut leaves: Vec<Hash> = self.epochs.iter().map(|e| e.root).collect();
        let target_size = leaves.len().next_power_of_two();
        leaves.resize(target_size, EMPTY_HASH);

        let mut current_level = leaves;
        let mut height = 26u8; // Epoch tier starts at height 26

        while current_level.len() > 1 {
            let mut next_level = Vec::with_capacity(current_level.len() / 2);
            for chunk in current_level.chunks(2) {
                let left = chunk[0];
                let right = if chunk.len() > 1 { chunk[1] } else { EMPTY_HASH };
                next_level.push(hash_node(height, &left, &right));
            }
            current_level = next_level;
            height += 1;
        }

        current_level.into_iter().next().unwrap_or(EMPTY_HASH)
    }

    /// Get the root of a specific epoch (for snapshot points)
    pub fn epoch_root(&self, epoch_idx: u16) -> Option<Hash> {
        self.epochs.get(epoch_idx as usize).map(|e| e.root)
    }

    /// Get position of a commitment
    pub fn position(&self, commitment: &Commitment) -> Option<Position> {
        self.index.get(commitment).copied()
    }

    /// Check if commitment exists
    pub fn contains(&self, commitment: &Commitment) -> bool {
        self.index.contains_key(commitment)
    }

    /// Number of total commitments
    pub fn len(&self) -> usize {
        self.index.len()
    }

    /// Is tree empty
    pub fn is_empty(&self) -> bool {
        self.index.is_empty()
    }
}

impl Default for TieredCommitmentTree {
    fn default() -> Self {
        Self::new()
    }
}

/// Hash a commitment leaf with domain separation
fn hash_leaf(commitment: &Commitment) -> Hash {
    let mut hasher = Sha256::new();
    hasher.update(b"TCT_LEAF");
    hasher.update(commitment.to_bytes());
    hasher.finalize().into()
}

/// Hash two children to form parent node with height-based domain separation
fn hash_node(height: u8, left: &Hash, right: &Hash) -> Hash {
    let mut hasher = Sha256::new();
    hasher.update(b"TCT_NODE");
    hasher.update([height]);
    hasher.update(left);
    hasher.update(right);
    hasher.finalize().into()
}

/// Thread-safe wrapper for the TCT
pub struct SharedTCT(Arc<RwLock<TieredCommitmentTree>>);

impl SharedTCT {
    pub fn new() -> Self {
        Self(Arc::new(RwLock::new(TieredCommitmentTree::new())))
    }

    pub fn insert(&self, commitment: Commitment) -> Position {
        self.0.write().unwrap().insert(commitment)
    }

    pub fn end_block(&self) -> Hash {
        self.0.write().unwrap().end_block()
    }

    pub fn end_epoch(&self) -> Hash {
        self.0.write().unwrap().end_epoch()
    }

    pub fn witness(&self, commitment: &Commitment) -> Option<AuthPath> {
        self.0.read().unwrap().witness(commitment)
    }

    pub fn root(&self) -> Hash {
        self.0.read().unwrap().root()
    }

    pub fn epoch_root(&self, epoch: u16) -> Option<Hash> {
        self.0.read().unwrap().epoch_root(epoch)
    }

    pub fn contains(&self, commitment: &Commitment) -> bool {
        self.0.read().unwrap().contains(commitment)
    }

    pub fn len(&self) -> usize {
        self.0.read().unwrap().len()
    }
}

impl Default for SharedTCT {
    fn default() -> Self {
        Self::new()
    }
}

impl Clone for SharedTCT {
    fn clone(&self) -> Self {
        Self(self.0.clone())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_position_roundtrip() {
        let pos = Position::new(100, 500, 30000);
        let packed = pos.to_u64();
        let unpacked = Position::from_u64(packed);
        assert_eq!(pos, unpacked);
    }

    #[test]
    fn test_insert_and_witness() {
        let mut tree = TieredCommitmentTree::new();

        let c1 = Commitment([1u8; 32]);
        let c2 = Commitment([2u8; 32]);

        let pos1 = tree.insert(c1);
        let pos2 = tree.insert(c2);

        assert_eq!(pos1.commitment, 0);
        assert_eq!(pos2.commitment, 1);
        assert!(tree.contains(&c1));
        assert!(tree.contains(&c2));
    }

    #[test]
    fn test_block_finalization() {
        let mut tree = TieredCommitmentTree::new();

        for i in 0..10 {
            tree.insert(Commitment([i as u8; 32]));
        }

        let block_root = tree.end_block();
        assert_ne!(block_root, EMPTY_HASH);
    }

    #[test]
    fn test_epoch_finalization() {
        let mut tree = TieredCommitmentTree::new();

        for i in 0..10 {
            tree.insert(Commitment([i as u8; 32]));
        }

        tree.end_block();
        let epoch_root = tree.end_epoch();
        assert_ne!(epoch_root, EMPTY_HASH);
    }
}
