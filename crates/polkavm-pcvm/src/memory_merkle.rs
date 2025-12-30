//! Binary Field Merkle Tree for Memory Authentication
//!
//! # ⚠️ DEPRECATED - CRYPTOGRAPHICALLY INSECURE ⚠️
//!
//! This module uses the insecure [`crate::poseidon`] hash function which has
//! critical security flaws (see that module's documentation).
//!
//! ## Use Instead
//!
//! Use [`crate::merkle128`] which provides:
//! - Rescue-Prime hash with proper x^(-1) S-box
//! - 128-bit field (64-bit collision resistance)
//! - Verified MDS matrix
//!
//! This module is kept only for backwards compatibility.
//!
//! ---
//!
//! This module implements a Merkle tree over GF(2^32) for cryptographically
//! authenticating memory contents in PolkaVM execution traces.
//!
//! # Why Merkle Trees for Memory?
//!
//! In zkVM proofs, we need to prove:
//! 1. **Load correctness**: memory[addr] = value
//! 2. **Store updates**: memory' = update(memory, addr, value)
//! 3. **Memory consistency**: state carries between steps
//!
//! Merkle trees give us O(log N) proofs instead of O(N) full memory.
//! 4. Fast in hardware (CLMUL)
//!
//! Unlike SHA-256 (which requires 256-bit fields), Poseidon works
//! directly in our proof system's native field.

use ligerito_binary_fields::{BinaryElem32, BinaryFieldElement};
use super::poseidon::PoseidonHash;

#[cfg(not(feature = "std"))]
use alloc::vec::Vec;

/// Merkle tree over memory (word-addressed)
///
/// Memory is treated as an array of u32 words.
/// Tree structure:
/// ```text
///           root
///          /    \
///        h0      h1
///       / \     / \
///     w0  w1  w2  w3  ← memory words
/// ```
#[derive(Debug, Clone)]
pub struct MemoryMerkleTree {
    /// Memory contents (leaves of the tree)
    memory: Vec<u32>,

    /// Internal nodes of the tree (level by level, bottom-up)
    /// nodes[0] = leaf hashes
    /// nodes[1] = level 1 internal nodes
    /// ...
    /// nodes[height-1] = root (single element)
    nodes: Vec<Vec<BinaryElem32>>,

    /// Tree height (log2 of memory size)
    height: usize,

    /// Root hash
    root: BinaryElem32,
}

impl MemoryMerkleTree {
    /// Create a new Merkle tree from memory contents
    ///
    /// Memory size must be a power of 2 for perfect binary tree.
    pub fn new(memory: Vec<u32>) -> Result<Self, &'static str> {
        if memory.is_empty() {
            return Err("Memory cannot be empty");
        }

        // Check power of 2
        if !memory.len().is_power_of_two() {
            return Err("Memory size must be power of 2");
        }

        let height = memory.len().trailing_zeros() as usize;
        let mut tree = Self {
            memory: memory.clone(),
            nodes: Vec::new(),
            height,
            root: BinaryElem32::zero(),
        };

        tree.rebuild();
        Ok(tree)
    }

    /// Create tree with power-of-2 size (padding with zeros)
    pub fn with_size(size: usize) -> Result<Self, &'static str> {
        if !size.is_power_of_two() {
            return Err("Size must be power of 2");
        }
        Self::new(vec![0u32; size])
    }

    /// Rebuild the entire tree (after modifications)
    fn rebuild(&mut self) {
        let n = self.memory.len();

        // Level 0: Hash individual memory words
        let mut current_level: Vec<BinaryElem32> = self.memory.iter()
            .map(|&word| hash_leaf(word))
            .collect();

        let mut all_levels = vec![current_level.clone()];

        // Build tree bottom-up
        while current_level.len() > 1 {
            let mut next_level = Vec::with_capacity(current_level.len() / 2);

            for chunk in current_level.chunks(2) {
                let left = chunk[0];
                let right = chunk.get(1).copied().unwrap_or(left);
                let parent = hash_pair(left, right);
                next_level.push(parent);
            }

            current_level = next_level.clone();
            all_levels.push(next_level);
        }

        self.root = current_level[0];
        self.nodes = all_levels;
    }

    /// Get the root hash
    pub fn root(&self) -> BinaryElem32 {
        self.root
    }

    /// Read a word from memory
    pub fn read(&self, address: u32) -> Option<u32> {
        self.memory.get(address as usize).copied()
    }

    /// Update memory and recompute affected hashes
    ///
    /// This is more efficient than full rebuild - only O(log N) updates.
    pub fn write(&mut self, address: u32, value: u32) -> Result<(), &'static str> {
        let addr = address as usize;
        if addr >= self.memory.len() {
            return Err("Address out of bounds");
        }

        // Update memory
        self.memory[addr] = value;

        // Update leaf hash
        self.nodes[0][addr] = hash_leaf(value);

        // Propagate changes up the tree
        let mut index = addr;
        for level in 1..self.nodes.len() {
            // Parent index in next level
            index = index / 2;

            // Sibling index in current level
            let sibling_idx = if index * 2 + 1 < self.nodes[level - 1].len() {
                index * 2 + 1
            } else {
                index * 2  // If no sibling, use self
            };

            let left = self.nodes[level - 1][index * 2];
            let right = self.nodes[level - 1].get(sibling_idx).copied().unwrap_or(left);

            self.nodes[level][index] = hash_pair(left, right);
        }

        // Update root
        self.root = self.nodes[self.nodes.len() - 1][0];

        Ok(())
    }

    /// Generate a Merkle proof for a memory read
    ///
    /// Proof format: [sibling_0, sibling_1, ..., sibling_height]
    /// where sibling_i is the sibling at level i needed to recompute the root.
    pub fn prove_read(&self, address: u32) -> Result<MerkleProof, &'static str> {
        let addr = address as usize;
        if addr >= self.memory.len() {
            return Err("Address out of bounds");
        }

        let value = self.memory[addr];
        let mut proof_siblings = Vec::new();
        let mut index = addr;

        // Collect siblings from each level
        for level in 0..self.height {
            let sibling_idx = if index % 2 == 0 {
                // We're left child, sibling is right
                index + 1
            } else {
                // We're right child, sibling is left
                index - 1
            };

            // Get sibling hash (or duplicate if at boundary)
            let sibling = self.nodes[level].get(sibling_idx)
                .copied()
                .unwrap_or(self.nodes[level][index]);

            proof_siblings.push(sibling);

            index = index / 2;  // Move to parent in next level
        }

        Ok(MerkleProof {
            address,
            value,
            siblings: proof_siblings,
            root: self.root,
        })
    }

    /// Verify a Merkle proof
    pub fn verify_proof(proof: &MerkleProof) -> bool {
        let mut current = hash_leaf(proof.value);
        let mut index = proof.address as usize;

        for sibling in &proof.siblings {
            if index % 2 == 0 {
                // We're left child
                current = hash_pair(current, *sibling);
            } else {
                // We're right child
                current = hash_pair(*sibling, current);
            }

            index = index / 2;
        }

        current == proof.root
    }
}

/// Merkle proof for a memory access
///
/// This proves that memory[address] = value under root hash.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MerkleProof {
    /// Address being accessed
    pub address: u32,

    /// Value at that address
    pub value: u32,

    /// Sibling hashes for path to root
    pub siblings: Vec<BinaryElem32>,

    /// Expected root hash
    pub root: BinaryElem32,
}

impl MerkleProof {
    /// Verify this proof
    pub fn verify(&self) -> bool {
        MemoryMerkleTree::verify_proof(self)
    }

    /// Size of the proof (number of siblings)
    pub fn size(&self) -> usize {
        self.siblings.len()
    }
}

/// Hash a single memory word (leaf node)
fn hash_leaf(word: u32) -> BinaryElem32 {
    // Use Poseidon to hash the word with a domain separator
    let elements = vec![
        BinaryElem32::from(0xDEADBEEF),  // Domain separator for leaves
        BinaryElem32::from(word),
    ];
    PoseidonHash::hash_elements(&elements)
}

/// Hash a pair of internal nodes
fn hash_pair(left: BinaryElem32, right: BinaryElem32) -> BinaryElem32 {
    let elements = vec![
        BinaryElem32::from(0xCAFEBABE),  // Domain separator for internal nodes
        left,
        right,
    ];
    PoseidonHash::hash_elements(&elements)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_create_merkle_tree() {
        let memory = vec![1, 2, 3, 4, 5, 6, 7, 8];
        let tree = MemoryMerkleTree::new(memory).expect("Failed to create tree");

        assert_eq!(tree.height, 3);  // log2(8) = 3
        assert_ne!(tree.root(), BinaryElem32::zero());
    }

    #[test]
    fn test_merkle_read() {
        let memory = vec![10, 20, 30, 40];
        let tree = MemoryMerkleTree::new(memory).expect("Failed to create tree");

        assert_eq!(tree.read(0), Some(10));
        assert_eq!(tree.read(1), Some(20));
        assert_eq!(tree.read(2), Some(30));
        assert_eq!(tree.read(3), Some(40));
        assert_eq!(tree.read(4), None);
    }

    #[test]
    fn test_merkle_write() {
        let memory = vec![0, 0, 0, 0];
        let mut tree = MemoryMerkleTree::new(memory).expect("Failed to create tree");

        let root_before = tree.root();

        tree.write(2, 42).expect("Failed to write");

        let root_after = tree.root();

        // Root should change
        assert_ne!(root_before, root_after);

        // Value should be updated
        assert_eq!(tree.read(2), Some(42));
    }

    #[test]
    fn test_merkle_proof_valid() {
        let memory = vec![10, 20, 30, 40, 50, 60, 70, 80];
        let tree = MemoryMerkleTree::new(memory).expect("Failed to create tree");

        // Generate proof for address 5 (value = 60)
        let proof = tree.prove_read(5).expect("Failed to generate proof");

        assert_eq!(proof.address, 5);
        assert_eq!(proof.value, 60);
        assert_eq!(proof.root, tree.root());

        // Proof should verify
        assert!(proof.verify());
    }

    #[test]
    fn test_merkle_proof_all_addresses() {
        let memory = vec![1, 2, 3, 4, 5, 6, 7, 8];
        let tree = MemoryMerkleTree::new(memory).expect("Failed to create tree");

        // All proofs should verify
        for addr in 0..8 {
            let proof = tree.prove_read(addr).expect("Failed to generate proof");
            assert!(proof.verify(), "Proof failed for address {}", addr);
        }
    }

    #[test]
    fn test_merkle_proof_after_write() {
        let memory = vec![0, 0, 0, 0];
        let mut tree = MemoryMerkleTree::new(memory).expect("Failed to create tree");

        // Write to address 2
        tree.write(2, 999).expect("Failed to write");

        // Generate proof for the write
        let proof = tree.prove_read(2).expect("Failed to generate proof");

        assert_eq!(proof.value, 999);
        assert!(proof.verify());
    }

    #[test]
    fn test_merkle_proof_tampered_value() {
        let memory = vec![10, 20, 30, 40];
        let tree = MemoryMerkleTree::new(memory).expect("Failed to create tree");

        let mut proof = tree.prove_read(1).expect("Failed to generate proof");

        // Tamper with the value
        proof.value = 999;

        // Should NOT verify
        assert!(!proof.verify());
    }

    #[test]
    fn test_merkle_proof_tampered_root() {
        let memory = vec![10, 20, 30, 40];
        let tree = MemoryMerkleTree::new(memory).expect("Failed to create tree");

        let mut proof = tree.prove_read(1).expect("Failed to generate proof");

        // Tamper with the root
        proof.root = BinaryElem32::from(0xDEADBEEF);

        // Should NOT verify
        assert!(!proof.verify());
    }

    #[test]
    fn test_merkle_deterministic() {
        let memory = vec![1, 2, 3, 4, 5, 6, 7, 8];

        let tree1 = MemoryMerkleTree::new(memory.clone()).expect("Failed to create tree");
        let tree2 = MemoryMerkleTree::new(memory.clone()).expect("Failed to create tree");

        // Same memory should give same root
        assert_eq!(tree1.root(), tree2.root());
    }

    #[test]
    fn test_merkle_different_memory() {
        let mem1 = vec![1, 2, 3, 4];
        let mem2 = vec![1, 2, 3, 5];  // Last element different

        let tree1 = MemoryMerkleTree::new(mem1).expect("Failed to create tree");
        let tree2 = MemoryMerkleTree::new(mem2).expect("Failed to create tree");

        // Different memory should give different root
        assert_ne!(tree1.root(), tree2.root());
    }

    #[test]
    fn test_power_of_two_requirement() {
        // Non-power-of-2 should fail
        assert!(MemoryMerkleTree::new(vec![1, 2, 3]).is_err());
        assert!(MemoryMerkleTree::new(vec![1, 2, 3, 4, 5]).is_err());

        // Power-of-2 should succeed
        assert!(MemoryMerkleTree::new(vec![1, 2]).is_ok());
        assert!(MemoryMerkleTree::new(vec![1, 2, 3, 4]).is_ok());
        assert!(MemoryMerkleTree::new(vec![0; 16]).is_ok());
    }

    #[test]
    fn test_proof_size() {
        // Proof size should be log2(memory_size)
        let tree4 = MemoryMerkleTree::new(vec![0; 4]).expect("Failed");
        let proof4 = tree4.prove_read(0).expect("Failed");
        assert_eq!(proof4.size(), 2);  // log2(4) = 2

        let tree16 = MemoryMerkleTree::new(vec![0; 16]).expect("Failed");
        let proof16 = tree16.prove_read(0).expect("Failed");
        assert_eq!(proof16.size(), 4);  // log2(16) = 4

        let tree256 = MemoryMerkleTree::new(vec![0; 256]).expect("Failed");
        let proof256 = tree256.prove_read(0).expect("Failed");
        assert_eq!(proof256.size(), 8);  // log2(256) = 8
    }

    #[test]
    fn test_large_memory() {
        // 64KB memory = 16384 words
        let tree = MemoryMerkleTree::with_size(16384).expect("Failed to create tree");

        // Write and read
        let mut tree = tree;
        tree.write(1000, 0xDEADBEEF).expect("Failed to write");

        let proof = tree.prove_read(1000).expect("Failed to generate proof");
        assert_eq!(proof.value, 0xDEADBEEF);
        assert!(proof.verify());

        // Proof size for 64KB should be 14 siblings
        assert_eq!(proof.size(), 14);  // log2(16384) = 14
    }
}
