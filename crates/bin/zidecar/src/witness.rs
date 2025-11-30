//! Note witness verification
//!
//! Verifies that a note commitment exists in the Orchard commitment tree
//! by checking a Merkle path against a committed root.

use crate::constants::{ORCHARD_TREE_DEPTH, SUBTREE_DEPTH, LEAVES_PER_SUBTREE};
use crate::error::{Result, ZidecarError};
use crate::zebrad::{ZebradClient, Subtree};
use blake2::{Blake2b512, Digest};

/// A witness proving a note commitment exists at a specific position
#[derive(Debug, Clone)]
pub struct NoteWitness {
    /// The note commitment (cmx)
    pub commitment: [u8; 32],
    /// Position in the tree (global leaf index)
    pub position: u64,
    /// Merkle path from leaf to root (32 siblings)
    pub path: Vec<[u8; 32]>,
}

impl NoteWitness {
    /// Verify this witness against an expected root
    /// Uses Sinsemilla hash for Orchard merkle tree
    pub fn verify(&self, expected_root: &[u8; 32]) -> bool {
        let computed = self.compute_root();
        computed == *expected_root
    }

    /// Compute the merkle root from this witness
    pub fn compute_root(&self) -> [u8; 32] {
        if self.path.len() != ORCHARD_TREE_DEPTH as usize {
            return [0u8; 32]; // invalid path length
        }

        let mut current = self.commitment;
        let mut pos = self.position;

        for sibling in &self.path {
            // Determine if we're left (0) or right (1) child
            let is_right = (pos & 1) == 1;

            // Hash children in correct order
            // Note: Real Orchard uses Sinsemilla, this is simplified
            current = if is_right {
                merkle_hash(sibling, &current)
            } else {
                merkle_hash(&current, sibling)
            };

            pos >>= 1;
        }

        current
    }
}

/// Simplified merkle hash (real Orchard uses Sinsemilla)
/// For now use blake2b - this is for testing the structure
fn merkle_hash(left: &[u8; 32], right: &[u8; 32]) -> [u8; 32] {
    let mut hasher = Blake2b512::new();
    hasher.update(b"OrchardMerkle"); // domain separator
    hasher.update(left);
    hasher.update(right);
    let hash = hasher.finalize();
    let mut result = [0u8; 32];
    result.copy_from_slice(&hash[..32]);
    result
}

/// Witness builder that can construct witnesses using subtree data
pub struct WitnessBuilder {
    /// Cached subtree roots from z_getsubtreesbyindex
    subtrees: Vec<Subtree>,
    /// Pool name ("orchard" or "sapling")
    pool: String,
}

impl WitnessBuilder {
    /// Create a new witness builder
    pub fn new(pool: &str) -> Self {
        Self {
            subtrees: Vec::new(),
            pool: pool.to_string(),
        }
    }

    /// Load subtrees from zebrad
    pub async fn load_subtrees(&mut self, zebrad: &ZebradClient) -> Result<()> {
        let response = zebrad.get_subtrees_by_index(&self.pool, 0, None).await?;
        self.subtrees = response.subtrees;
        Ok(())
    }

    /// Get the subtree index for a given position
    pub fn subtree_index(position: u64) -> u32 {
        (position / LEAVES_PER_SUBTREE as u64) as u32
    }

    /// Get subtree root at index
    pub fn get_subtree_root(&self, index: u32) -> Option<[u8; 32]> {
        self.subtrees.get(index as usize).and_then(|s| {
            hex::decode(&s.root).ok().and_then(|b| b.try_into().ok())
        })
    }

    /// Check if a position is within a completed subtree
    pub fn is_in_completed_subtree(&self, position: u64) -> bool {
        let subtree_idx = Self::subtree_index(position);
        subtree_idx < self.subtrees.len() as u32
    }
}

/// Verify a note exists in the committed state
/// This is the main entry point for wallet verification
pub async fn verify_note_in_state(
    zebrad: &ZebradClient,
    commitment: &[u8; 32],
    position: u64,
    witness_path: &[[u8; 32]],
    expected_root: &[u8; 32],
) -> Result<bool> {
    // Build the witness
    let witness = NoteWitness {
        commitment: *commitment,
        position,
        path: witness_path.to_vec(),
    };

    // Verify against expected root
    Ok(witness.verify(expected_root))
}

/// Response from server with witness data
#[derive(Debug, Clone)]
pub struct WitnessResponse {
    /// The note commitment
    pub commitment: [u8; 32],
    /// Position in the tree
    pub position: u64,
    /// Merkle path
    pub path: Vec<[u8; 32]>,
    /// Height where this was proven
    pub height: u32,
    /// Root at that height
    pub root: [u8; 32],
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_subtree_index() {
        assert_eq!(WitnessBuilder::subtree_index(0), 0);
        assert_eq!(WitnessBuilder::subtree_index(65535), 0);
        assert_eq!(WitnessBuilder::subtree_index(65536), 1);
        assert_eq!(WitnessBuilder::subtree_index(131071), 1);
        assert_eq!(WitnessBuilder::subtree_index(131072), 2);
    }

    #[test]
    fn test_merkle_hash_deterministic() {
        let left = [1u8; 32];
        let right = [2u8; 32];
        let h1 = merkle_hash(&left, &right);
        let h2 = merkle_hash(&left, &right);
        assert_eq!(h1, h2);
    }

    #[test]
    fn test_merkle_hash_order_matters() {
        let left = [1u8; 32];
        let right = [2u8; 32];
        let h1 = merkle_hash(&left, &right);
        let h2 = merkle_hash(&right, &left);
        assert_ne!(h1, h2);
    }
}
