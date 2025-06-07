//! Merkle tree with batch opening support
//! Matches the Julia BatchedMerkleTree implementation

mod batch;

pub use batch::{BatchedMerkleProof, prove_batch, verify_batch};

use sha2::{Sha256, Digest};
use bytemuck::Pod;

/// Hash type for Merkle tree nodes
pub type Hash = [u8; 32];

/// Complete Merkle tree structure
#[derive(Clone, Debug)]
pub struct CompleteMerkleTree {
    /// Tree layers, from leaves to root
    layers: Vec<Vec<Hash>>,
}

/// Merkle root
#[derive(Clone, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct MerkleRoot {
    pub root: Option<Hash>,
}

impl MerkleRoot {
    pub fn size_of(&self) -> usize {
        self.root.map_or(0, |_| 32)
    }
}

/// Hash a leaf value
fn hash_leaf<T: Pod>(leaf: &T) -> Hash {
    let bytes = bytemuck::bytes_of(leaf);
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    hasher.finalize().into()
}

/// Hash two sibling nodes
fn hash_siblings(left: &Hash, right: &Hash) -> Hash {
    let mut hasher = Sha256::new();
    hasher.update(left);
    hasher.update(right);
    hasher.finalize().into()
}

/// Build a complete Merkle tree from leaves
pub fn build_merkle_tree<T: Pod>(leaves: &[T]) -> CompleteMerkleTree {
    assert!(leaves.len().is_power_of_two(), "Leaves must be power of 2");
    
    if leaves.is_empty() {
        return CompleteMerkleTree { layers: vec![] };
    }
    
    // Hash all leaves
    let mut current_layer: Vec<Hash> = leaves.iter()
        .map(hash_leaf)
        .collect();
    
    let mut layers = vec![current_layer.clone()];
    
    // Build tree bottom-up
    while current_layer.len() > 1 {
        let next_layer: Vec<Hash> = current_layer
            .chunks_exact(2)
            .map(|chunk| hash_siblings(&chunk[0], &chunk[1]))
            .collect();
        
        layers.push(next_layer.clone());
        current_layer = next_layer;
    }
    
    CompleteMerkleTree { layers }
}

impl CompleteMerkleTree {
    /// Get the root of the tree
    pub fn get_root(&self) -> MerkleRoot {
        MerkleRoot {
            root: self.layers.last()
                .and_then(|layer| layer.first())
                .copied(),
        }
    }
    
    /// Get the depth of the tree
    pub fn get_depth(&self) -> usize {
        if self.layers.is_empty() {
            0
        } else {
            self.layers.len() - 1
        }
    }
    
    /// Prove multiple leaf indices
    pub fn prove(&self, queries: &[usize]) -> BatchedMerkleProof {
        batch::prove_batch(self, queries)
    }
}

/// Verify a batched Merkle proof
pub fn verify(
    root: &MerkleRoot,
    proof: &BatchedMerkleProof,
    depth: usize,
    leaves: &[impl Pod],
    leaf_indices: &[usize],
) -> bool {
    batch::verify_batch(root, proof, depth, leaves, leaf_indices)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_merkle_tree() {
        let leaves: Vec<[u16; 4]> = (0..16)
            .map(|i| [i, i+1, i+2, i+3])
            .collect();
        
        let tree = build_merkle_tree(&leaves);
        let root = tree.get_root();
        
        assert!(root.root.is_some());
        assert_eq!(tree.get_depth(), 4); // log2(16)
    }
    
    #[test]
    fn test_batch_proof() {
        let leaves: Vec<u64> = (0..16).collect();
        let tree = build_merkle_tree(&leaves);
        let root = tree.get_root();

        // Use 0-based indices
        let queries = vec![0, 2, 6, 9];
        let proof = tree.prove(&queries);

        let queried_leaves: Vec<u64> = queries.iter()
            .map(|&i| leaves[i])
            .collect();

        assert!(verify(
            &root,
            &proof,
            tree.get_depth(),
            &queried_leaves,
            &queries
        ));
    }
}
