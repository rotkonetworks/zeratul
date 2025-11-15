// src/lib.rs
//! Merkle tree with batch opening support
//! Matches the Julia BatchedMerkleTree implementation

#![cfg_attr(not(feature = "std"), no_std)]

#[cfg(not(feature = "std"))]
extern crate alloc;

#[cfg(not(feature = "std"))]
use alloc::vec::Vec;

#[cfg(not(feature = "std"))]
#[macro_use]
extern crate alloc as alloc_crate;

pub mod batch;
pub use batch::{BatchedMerkleProof, prove_batch, verify_batch};

use sha2::{Sha256, Digest};
use bytemuck::Pod;

#[cfg(feature = "parallel")]
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

pub fn build_merkle_tree<T: Pod + Send + Sync>(leaves: &[T]) -> CompleteMerkleTree {
    if leaves.is_empty() {
        return CompleteMerkleTree { layers: vec![] };
    }

    if !is_power_of_two(leaves.len()) {
        panic!("Number of leaves must be a power of 2");
    }

    // parallelize initial leaf hashing for large leaf sets (when parallel feature is enabled)
    let mut current_layer: Vec<Hash> = {
        #[cfg(feature = "parallel")]
        {
            if leaves.len() >= 64 {
                leaves.par_iter()
                    .map(|leaf| hash_leaf(leaf))
                    .collect()
            } else {
                leaves.iter()
                    .map(|leaf| hash_leaf(leaf))
                    .collect()
            }
        }
        #[cfg(not(feature = "parallel"))]
        {
            leaves.iter()
                .map(|leaf| hash_leaf(leaf))
                .collect()
        }
    };

    let mut layers = vec![current_layer.clone()];

    while current_layer.len() > 1 {
        // parallelize sibling hashing for larger layers only to avoid thread overhead (when parallel feature is enabled)
        let next_layer: Vec<Hash> = {
            #[cfg(feature = "parallel")]
            {
                if current_layer.len() >= 64 {
                    current_layer
                        .par_chunks_exact(2)
                        .map(|chunk| hash_siblings(&chunk[0], &chunk[1]))
                        .collect()
                } else {
                    current_layer
                        .chunks_exact(2)
                        .map(|chunk| hash_siblings(&chunk[0], &chunk[1]))
                        .collect()
                }
            }
            #[cfg(not(feature = "parallel"))]
            {
                current_layer
                    .chunks_exact(2)
                    .map(|chunk| hash_siblings(&chunk[0], &chunk[1]))
                    .collect()
            }
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

    /// Generate trace for a leaf at given index
    ///
    /// Returns each opposite node from top to bottom as the tree is navigated
    /// to arrive at the leaf. This follows the graypaper specification for
    /// creating justifications of data inclusion.
    ///
    /// From graypaper Section "Merklization":
    /// > We also define the trace function T, which returns each opposite node
    /// > from top to bottom as the tree is navigated to arrive at some leaf
    /// > corresponding to the item of a given index into the sequence.
    ///
    /// # Arguments
    /// * `index` - 0-based index of the leaf
    ///
    /// # Returns
    /// Vector of sibling hashes from root to leaf (top to bottom)
    ///
    /// # Example
    /// ```ignore
    /// let leaves: Vec<u64> = (0..8).collect();
    /// let tree = build_merkle_tree(&leaves);
    /// let trace = tree.trace(3);  // Get trace for leaf at index 3
    /// // trace[0] = sibling at top level
    /// // trace[1] = sibling at middle level
    /// // trace[2] = sibling at leaf level
    /// ```
    pub fn trace(&self, index: usize) -> Vec<Hash> {
        if self.layers.is_empty() {
            return vec![];
        }

        let num_leaves = self.layers[0].len();
        if index >= num_leaves {
            panic!("Index {} out of range (tree has {} leaves)", index, num_leaves);
        }

        if num_leaves == 1 {
            // Single leaf - no siblings
            return vec![];
        }

        let mut trace = Vec::new();
        let mut current_index = index;

        // Iterate from leaf level up to root (but we collect top-to-bottom)
        // So we'll reverse at the end
        for layer_idx in 0..(self.layers.len() - 1) {
            let layer = &self.layers[layer_idx];

            // Find sibling index (if index is even, sibling is index+1, else index-1)
            let sibling_index = if current_index % 2 == 0 {
                current_index + 1
            } else {
                current_index - 1
            };

            // Add sibling hash to trace
            if sibling_index < layer.len() {
                trace.push(layer[sibling_index]);
            }

            // Move to parent index for next layer
            current_index /= 2;
        }

        // Reverse to get top-to-bottom order (as per graypaper spec)
        trace.reverse();
        trace
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

    #[test]
    fn test_trace_basic() {
        // Create a tree with 8 leaves
        let leaves: Vec<u64> = (0..8).collect();
        let tree = build_merkle_tree(&leaves);

        // For index 3 (binary: 011):
        // - At leaf level: sibling is index 2 (pair [2,3])
        // - At middle level: sibling is pair [0,1] (we're in [2,3])
        // - At top level: sibling is entire right subtree [4,5,6,7]
        let trace = tree.trace(3);

        // Should have depth nodes in trace (from top to bottom)
        assert_eq!(trace.len(), tree.get_depth());
        println!("Trace for index 3: {} hashes", trace.len());

        // Verify all hashes are non-zero
        for (i, hash) in trace.iter().enumerate() {
            assert_ne!(*hash, [0u8; 32], "Hash at level {} should not be zero", i);
        }
    }

    #[test]
    fn test_trace_verification() {
        // Test that trace can be used to verify a leaf
        let leaves: Vec<u64> = (0..8).collect();
        let tree = build_merkle_tree(&leaves);

        let index = 5;
        let trace = tree.trace(index);
        let leaf_hash = hash_leaf(&leaves[index]);

        // Reconstruct root from leaf + trace
        let mut current_hash = leaf_hash;
        let mut current_index = index;

        // Work backwards through trace (bottom to top)
        for sibling_hash in trace.iter().rev() {
            if current_index % 2 == 0 {
                // Current is left child
                current_hash = hash_siblings(&current_hash, sibling_hash);
            } else {
                // Current is right child
                current_hash = hash_siblings(sibling_hash, &current_hash);
            }
            current_index /= 2;
        }

        // Should match root
        assert_eq!(current_hash, tree.get_root().root.unwrap());
    }

    #[test]
    fn test_trace_all_leaves() {
        // Verify trace works for every leaf in the tree
        let leaves: Vec<u64> = (0..16).collect();
        let tree = build_merkle_tree(&leaves);
        let root = tree.get_root().root.unwrap();

        for index in 0..leaves.len() {
            let trace = tree.trace(index);
            let leaf_hash = hash_leaf(&leaves[index]);

            // Reconstruct root
            let mut current_hash = leaf_hash;
            let mut current_index = index;

            for sibling_hash in trace.iter().rev() {
                if current_index % 2 == 0 {
                    current_hash = hash_siblings(&current_hash, sibling_hash);
                } else {
                    current_hash = hash_siblings(sibling_hash, &current_hash);
                }
                current_index /= 2;
            }

            assert_eq!(current_hash, root, "Trace verification failed for index {}", index);
        }
    }

    #[test]
    fn test_trace_empty_tree() {
        let leaves: Vec<u64> = vec![];
        let tree = build_merkle_tree(&leaves);
        let trace = tree.trace(0);
        assert_eq!(trace.len(), 0);
    }

    #[test]
    fn test_trace_single_leaf() {
        let leaves = vec![42u64];
        let tree = build_merkle_tree(&leaves);
        let trace = tree.trace(0);
        // Single leaf has no siblings
        assert_eq!(trace.len(), 0);
    }

    #[test]
    #[should_panic(expected = "out of range")]
    fn test_trace_invalid_index() {
        let leaves: Vec<u64> = (0..8).collect();
        let tree = build_merkle_tree(&leaves);
        let _ = tree.trace(8); // Index 8 is out of bounds
    }

    #[test]
    fn test_trace_matches_graypaper_definition() {
        // Test the graypaper definition:
        // T returns opposite nodes from TOP to BOTTOM
        let leaves: Vec<u64> = (0..4).collect();
        let tree = build_merkle_tree(&leaves);

        // For index 2 in 4-leaf tree:
        // Tree structure:
        //       root
        //      /    \
        //    h01    h23  <- top level sibling
        //    / \    / \
        //   h0 h1  h2 h3 <- leaf level sibling
        //
        // Navigating to index 2 (leaf h2):
        // - From root: go right, opposite is h01 (left subtree)
        // - From h23: go left, opposite is h3 (right leaf)

        let trace = tree.trace(2);

        // Should have 2 siblings (depth = 2)
        assert_eq!(trace.len(), 2);

        // trace[0] should be top level (h01)
        // trace[1] should be leaf level (h3)

        // Verify by reconstructing root
        let leaf_hash = hash_leaf(&leaves[2]); // h2
        let h23 = hash_siblings(&leaf_hash, &trace[1]); // h2 + h3
        let root = hash_siblings(&trace[0], &h23); // h01 + h23

        assert_eq!(root, tree.get_root().root.unwrap());
    }
}
