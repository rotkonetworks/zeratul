// src/lib.rs
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
    pub layers: Vec<Vec<Hash>>,
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

/// Check if a number is a power of two
pub fn is_power_of_two(n: usize) -> bool {
    n > 0 && (n & (n - 1)) == 0
}

/// Hash a leaf value
pub fn hash_leaf<T: Pod>(leaf: &T) -> Hash {
    let bytes = bytemuck::bytes_of(leaf);
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    hasher.finalize().into()
}

/// Hash two sibling nodes
pub fn hash_siblings(left: &Hash, right: &Hash) -> Hash {
    let mut hasher = Sha256::new();
    hasher.update(left);
    hasher.update(right);
    hasher.finalize().into()
}

/// Build a complete Merkle tree from leaves
pub fn build_merkle_tree<T: Pod>(leaves: &[T]) -> CompleteMerkleTree {
    // Allow empty trees (matching Julia)
    if leaves.is_empty() {
        return CompleteMerkleTree { layers: vec![] };
    }
    
    assert!(is_power_of_two(leaves.len()), "Number of leaves must be a power of 2");

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

    /// Prove multiple leaf indices (1-based, matching Julia)
    pub fn prove(&self, queries: &[usize]) -> BatchedMerkleProof {
        batch::prove_batch(self, queries)
    }

    /// Prove multiple leaf indices (0-based, Rust convention)
    pub fn prove_0_based(&self, queries: &[usize]) -> BatchedMerkleProof {
        let queries_1_based: Vec<usize> = queries.iter()
            .map(|&q| q + 1)
            .collect();
        batch::prove_batch(self, &queries_1_based)
    }
}

/// Verify a batched Merkle proof (1-based indices, matching Julia)
pub fn verify<T: Pod>(
    root: &MerkleRoot,
    proof: &BatchedMerkleProof,
    depth: usize,
    leaves: &[T],
    leaf_indices: &[usize],
) -> bool {
    batch::verify_batch(root, proof, depth, leaves, leaf_indices)
}

/// Verify a batched Merkle proof (0-based indices, Rust convention)
pub fn verify_0_based<T: Pod>(
    root: &MerkleRoot,
    proof: &BatchedMerkleProof,
    depth: usize,
    leaves: &[T],
    leaf_indices: &[usize],
) -> bool {
    let indices_1_based: Vec<usize> = leaf_indices.iter()
        .map(|&i| i + 1)
        .collect();
    batch::verify_batch(root, proof, depth, leaves, &indices_1_based)
}

#[cfg(test)]
mod tests {
    use super::*;
    use rand::{thread_rng, seq::SliceRandom};

    #[test]
    fn test_empty_tree() {
        let leaves: Vec<u64> = vec![];
        let tree = build_merkle_tree(&leaves);
        let root = tree.get_root();
        
        assert!(root.root.is_none());
        assert_eq!(tree.get_depth(), 0);
    }

    #[test]
    fn test_single_leaf() {
        let leaves = vec![42u64];
        let tree = build_merkle_tree(&leaves);
        let root = tree.get_root();
        
        assert!(root.root.is_some());
        assert_eq!(tree.get_depth(), 0);
    }

    #[test]
    fn test_merkle_tree_basic() {
        let leaves: Vec<[u16; 4]> = (0..16)
            .map(|i| [i, i+1, i+2, i+3])
            .collect();

        let tree = build_merkle_tree(&leaves);
        let root = tree.get_root();

        assert!(root.root.is_some());
        assert_eq!(tree.get_depth(), 4); // log2(16)
    }

    #[test]
    fn test_batch_proof_1_based() {
        let leaves: Vec<u64> = (0..16).collect();
        let tree = build_merkle_tree(&leaves);
        let root = tree.get_root();

        // Use 1-based indices (Julia convention)
        let queries = vec![1, 3, 7, 10];
        let proof = tree.prove(&queries);

        let queried_leaves: Vec<u64> = queries.iter()
            .map(|&i| leaves[i - 1]) // Convert to 0-based for array access
            .collect();

        assert!(verify(
            &root,
            &proof,
            tree.get_depth(),
            &queried_leaves,
            &queries
        ));
    }

    #[test]
    fn test_batch_proof_0_based() {
        let leaves: Vec<u64> = (0..16).collect();
        let tree = build_merkle_tree(&leaves);
        let root = tree.get_root();

        // Use 0-based indices (Rust convention)
        let queries = vec![0, 2, 6, 9];
        let proof = tree.prove_0_based(&queries);

        let queried_leaves: Vec<u64> = queries.iter()
            .map(|&i| leaves[i])
            .collect();

        assert!(verify_0_based(
            &root,
            &proof,
            tree.get_depth(),
            &queried_leaves,
            &queries
        ));
    }

    #[test]
    fn test_invalid_proof() {
        let leaves: Vec<u64> = (0..16).collect();
        let tree = build_merkle_tree(&leaves);
        let root = tree.get_root();

        let queries = vec![1, 3, 7, 10];
        let proof = tree.prove(&queries);

        // Use wrong leaves
        let wrong_leaves: Vec<u64> = vec![100, 200, 300, 400];

        assert!(!verify(
            &root,
            &proof,
            tree.get_depth(),
            &wrong_leaves,
            &queries
        ));
    }

    #[test]
    fn test_large_random_subset() {
        // Similar to Julia test but smaller for unit testing
        let n = 10; // 2^10 = 1024 leaves
        let num_leaves = 1 << n;
        let num_queries = 100;

        // Generate random leaves
        let leaves: Vec<[u16; 4]> = (0..num_leaves)
            .map(|_| {
                let val = rand::random::<u16>();
                [val; 4]
            })
            .collect();

        let tree = build_merkle_tree(&leaves);
        let root = tree.get_root();

        // Generate random queries (1-based)
        let mut rng = thread_rng();
        let mut queries: Vec<usize> = (1..=num_leaves).collect();
        queries.shuffle(&mut rng);
        queries.truncate(num_queries);
        queries.sort_unstable();

        let proof = tree.prove(&queries);

        let queried_leaves: Vec<[u16; 4]> = queries.iter()
            .map(|&q| leaves[q - 1])
            .collect();

        assert!(verify(
            &root,
            &proof,
            tree.get_depth(),
            &queried_leaves,
            &queries
        ));
    }

    #[test]
    #[should_panic(expected = "Number of leaves must be a power of 2")]
    fn test_non_power_of_two_panics() {
        let leaves: Vec<u64> = (0..15).collect(); // 15 is not a power of 2
        let _ = build_merkle_tree(&leaves);
    }

    #[test]
    fn test_debug_batch_proof() {
        let leaves: Vec<u64> = (0..16).collect();
        let tree = build_merkle_tree(&leaves);
        
        println!("Tree layers:");
        for (i, layer) in tree.layers.iter().enumerate() {
            println!("  Layer {}: {} nodes", i, layer.len());
        }
        
        let queries = vec![1, 3, 7, 10];
        let proof = tree.prove(&queries);
        
        println!("Proof size: {} siblings", proof.siblings.len());
        
        let queried_leaves: Vec<u64> = queries.iter()
            .map(|&i| leaves[i - 1])
            .collect();
            
        println!("Verifying with:");
        println!("  Queries (1-based): {:?}", queries);
        println!("  Leaves: {:?}", queried_leaves);
        println!("  Depth: {}", tree.get_depth());
            
        let is_valid = verify(
            &tree.get_root(),
            &proof,
            tree.get_depth(),
            &queried_leaves,
            &queries
        );
        
        println!("Verification result: {}", is_valid);
        assert!(is_valid);
    }

    #[test]
    fn test_simple_proof() {
        // Test with just 4 leaves for easier debugging
        let leaves: Vec<u64> = vec![0, 1, 2, 3];
        let tree = build_merkle_tree(&leaves);
        
        // Test single query first
        let queries = vec![1]; // First leaf (1-based)
        let proof = tree.prove(&queries);
        
        let queried_leaves = vec![leaves[0]];
        
        let is_valid = verify(
            &tree.get_root(),
            &proof,
            tree.get_depth(),
            &queried_leaves,
            &queries
        );
        
        assert!(is_valid, "Single query verification failed");
        
        // Test multiple queries
        let queries = vec![1, 3]; // First and third leaves (1-based)
        let proof = tree.prove(&queries);
        
        let queried_leaves: Vec<u64> = queries.iter()
            .map(|&i| leaves[i - 1])
            .collect();
        
        let is_valid = verify(
            &tree.get_root(),
            &proof,
            tree.get_depth(),
            &queried_leaves,
            &queries
        );
        
        assert!(is_valid, "Multiple query verification failed");
    }
}
