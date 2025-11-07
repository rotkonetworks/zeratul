// src/lib.rs
//! Merkle tree with batch opening support
//! Matches the Julia BatchedMerkleTree implementation

pub mod batch;
pub use batch::{BatchedMerkleProof, prove_batch, verify_batch};

use sha2::{Sha256, Digest};
use bytemuck::Pod;
use rayon::prelude::*;

pub type Hash = [u8; 32];

pub struct CompleteMerkleTree {
    pub layers: Vec<Vec<Hash>>,
}

#[derive(Clone, Debug)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct MerkleRoot {
    pub root: Option<Hash>,
}

impl MerkleRoot {
    pub fn size_of(&self) -> usize {
        self.root.map_or(0, |_| 32)
    }
}

pub fn is_power_of_two(n: usize) -> bool {
    n > 0 && (n & (n - 1)) == 0
}

pub fn hash_leaf<T: Pod>(leaf: &T) -> Hash {
    let bytes = bytemuck::bytes_of(leaf);
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    hasher.finalize().into()
}

pub fn hash_siblings(left: &Hash, right: &Hash) -> Hash {
    let mut hasher = Sha256::new();
    hasher.update(left);
    hasher.update(right);
    hasher.finalize().into()
}

pub fn build_merkle_tree<T: Pod>(leaves: &[T]) -> CompleteMerkleTree {
    if leaves.is_empty() {
        return CompleteMerkleTree { layers: vec![] };
    }

    if !is_power_of_two(leaves.len()) {
        panic!("Number of leaves must be a power of 2");
    }

    let mut current_layer: Vec<Hash> = leaves.iter()
        .map(|leaf| hash_leaf(leaf))
        .collect();

    let mut layers = vec![current_layer.clone()];

    while current_layer.len() > 1 {
        // Parallelize sibling hashing when layer is large enough (prover optimization)
        let next_layer: Vec<Hash> = if current_layer.len() >= 128 {
            current_layer
                .par_chunks_exact(2)
                .map(|chunk| hash_siblings(&chunk[0], &chunk[1]))
                .collect()
        } else {
            current_layer
                .chunks_exact(2)
                .map(|chunk| hash_siblings(&chunk[0], &chunk[1]))
                .collect()
        };

        layers.push(next_layer.clone());
        current_layer = next_layer;
    }

    CompleteMerkleTree { layers }
}

impl CompleteMerkleTree {
    pub fn get_root(&self) -> MerkleRoot {
        MerkleRoot {
            root: self.layers.last()
                .and_then(|layer| layer.first())
                .copied(),
        }
    }

    pub fn get_depth(&self) -> usize {
        if self.layers.is_empty() {
            0
        } else {
            self.layers.len() - 1
        }
    }

    pub fn prove(&self, queries: &[usize]) -> BatchedMerkleProof {
        prove_batch(self, queries)
    }
}

/// Verify a batched Merkle proof (0-based indices)
pub fn verify<T: Pod + Send + Sync>(
    root: &MerkleRoot,
    proof: &BatchedMerkleProof,
    depth: usize,
    leaves: &[T],
    leaf_indices: &[usize],
) -> bool {
    verify_batch(root, proof, depth, leaves, leaf_indices)
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

    #[test]
    fn test_invalid_proof() {
        let leaves: Vec<u64> = (0..16).collect();
        let tree = build_merkle_tree(&leaves);
        let root = tree.get_root();

        let queries = vec![0, 2, 6, 9];
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

        // Generate random queries (0-based)
        let mut rng = thread_rng();
        let mut queries: Vec<usize> = (0..num_leaves).collect();
        queries.shuffle(&mut rng);
        queries.truncate(num_queries);
        queries.sort_unstable();

        let proof = tree.prove(&queries);

        let queried_leaves: Vec<[u16; 4]> = queries.iter()
            .map(|&q| leaves[q])
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

        let queries = vec![0, 2, 6, 9];
        let proof = tree.prove(&queries);

        println!("Proof size: {} siblings", proof.siblings.len());

        let queried_leaves: Vec<u64> = queries.iter()
            .map(|&i| leaves[i])
            .collect();

        println!("Verifying with:");
        println!("  Queries (0-based): {:?}", queries);
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
        let queries = vec![0]; // First leaf (0-based)
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
        let queries = vec![0, 2]; // First and third leaves (0-based)
        let proof = tree.prove(&queries);

        let queried_leaves: Vec<u64> = queries.iter()
            .map(|&i| leaves[i])
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
