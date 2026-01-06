//! Unified Memory Model for PolkaVM
//!
//! # ⚠️ DEPRECATED - CRYPTOGRAPHICALLY INSECURE ⚠️
//!
//! This module uses the insecure [`crate::memory_merkle`] which in turn uses
//! the insecure [`crate::poseidon`] hash function. See those modules' documentation
//! for details on the security flaws.
//!
//! ## Use Instead
//!
//! Use [`crate::unified_memory128`] which provides:
//! - 128-bit merkle tree with Rescue-Prime hash
//! - 64-bit collision resistance (vs ~16-bit here)
//! - Proper cryptographic security
//!
//! This module is kept only for backwards compatibility.
//!
//! ---
//!
//! Program code and data share the same address space with a single merkle root.
//! This simplifies instruction fetch proofs - they're just memory reads.
//!
//! # Memory Layout
//!
//! ```text
//! Address Range          | Content         | Access
//! -----------------------|-----------------|--------
//! 0x0000_0000 - CODE_END | Program code    | Read-only
//! CODE_END - HEAP_START  | (reserved)      | None
//! HEAP_START - STACK_END | Heap + Stack    | Read-write
//! ```
//!
//! # Why Unified?
//!
//! 1. **Single commitment**: One merkle root for entire state
//! 2. **Reuse constraints**: Instruction fetch = memory read
//! 3. **Matches hardware**: RISC-V has unified address space
//! 4. **Simpler proofs**: No separate "program lookup" machinery

use crate::memory_merkle::{MemoryMerkleTree, MerkleProof as MemoryMerkleProof};
use ligerito_binary_fields::{BinaryElem32, BinaryFieldElement};

#[cfg(not(feature = "std"))]
use alloc::vec::Vec;

/// Memory layout constants
pub const CODE_START: u32 = 0x0000_0000;
pub const CODE_MAX_SIZE: u32 = 0x0010_0000;  // 1MB for code
pub const HEAP_START: u32 = 0x0010_0000;
pub const STACK_START: u32 = 0x8000_0000;

/// Unified memory with program code and data
#[derive(Debug, Clone)]
pub struct UnifiedMemory {
    /// Underlying merkle tree
    tree: MemoryMerkleTree,

    /// Program size in bytes (for bounds checking)
    program_size: u32,

    /// Total memory size (power of 2)
    memory_size: u32,
}

impl UnifiedMemory {
    /// Create unified memory with program loaded at address 0
    ///
    /// Program bytes are loaded starting at CODE_START.
    /// Rest of memory is initialized to zero.
    pub fn with_program(program_bytes: &[u8], memory_size: u32) -> Result<Self, &'static str> {
        if !memory_size.is_power_of_two() {
            return Err("memory size must be power of 2");
        }

        if program_bytes.len() as u32 > CODE_MAX_SIZE {
            return Err("program too large");
        }

        // Convert to u32 words (little-endian)
        let mut words = vec![0u32; memory_size as usize];

        // Load program bytes as u32 words
        for (i, chunk) in program_bytes.chunks(4).enumerate() {
            let mut word_bytes = [0u8; 4];
            word_bytes[..chunk.len()].copy_from_slice(chunk);
            words[i] = u32::from_le_bytes(word_bytes);
        }

        let tree = MemoryMerkleTree::new(words)?;

        Ok(Self {
            tree,
            program_size: program_bytes.len() as u32,
            memory_size,
        })
    }

    /// Get the state root (single commitment for code + data)
    pub fn root(&self) -> BinaryElem32 {
        self.tree.root()
    }

    /// Get access to the underlying merkle tree (for generating proofs)
    pub fn tree(&self) -> &MemoryMerkleTree {
        &self.tree
    }

    /// Get root as bytes
    pub fn root_bytes(&self) -> [u8; 32] {
        let root = self.tree.root();
        let mut bytes = [0u8; 32];
        bytes[0..4].copy_from_slice(&root.poly().value().to_le_bytes());
        bytes
    }

    /// Fetch instruction at PC (returns bytes + merkle proof)
    ///
    /// This is the key function: instruction fetch is just a memory read.
    pub fn fetch_instruction(&self, pc: u32) -> Result<InstructionFetch, &'static str> {
        // PC must be in code region
        if pc >= self.program_size {
            return Err("PC out of program bounds");
        }

        // Word-aligned address
        let word_addr = pc / 4;

        // Get the word containing instruction start
        let proof = self.tree.prove_read(word_addr)?;

        // For multi-word instructions, we may need second word
        // PVM instructions are variable length, but we handle 4-byte case for now
        let instruction_bytes = proof.value.to_le_bytes();

        Ok(InstructionFetch {
            pc,
            instruction_word: proof.value,
            merkle_proof: proof,
        })
    }

    /// Read data memory (heap/stack)
    pub fn read_data(&self, address: u32) -> Result<(u32, MemoryMerkleProof), &'static str> {
        if address < HEAP_START {
            return Err("data read in code region");
        }

        let word_addr = address / 4;
        let proof = self.tree.prove_read(word_addr)?;
        Ok((proof.value, proof))
    }

    /// Write data memory (heap/stack)
    pub fn write_data(&mut self, address: u32, value: u32) -> Result<MemoryMerkleProof, &'static str> {
        if address < HEAP_START {
            return Err("cannot write to code region");
        }

        let word_addr = address / 4;

        // Get proof before write
        let proof_before = self.tree.prove_read(word_addr)?;

        // Perform write
        self.tree.write(word_addr, value)?;

        Ok(proof_before)
    }
}

/// Instruction fetch result with merkle proof
#[derive(Debug, Clone)]
pub struct InstructionFetch {
    /// Program counter
    pub pc: u32,

    /// The u32 word containing instruction (or start of instruction)
    pub instruction_word: u32,

    /// Merkle proof that this word is at address pc/4 in committed memory
    pub merkle_proof: MemoryMerkleProof,
}

impl InstructionFetch {
    /// Verify this fetch against a state root
    pub fn verify(&self, expected_root: BinaryElem32) -> bool {
        if self.merkle_proof.root != expected_root {
            return false;
        }
        MemoryMerkleTree::verify_proof(&self.merkle_proof)
    }

    /// Convert to constraint-friendly form
    pub fn to_constraint_elements(&self) -> Vec<BinaryElem32> {
        let mut elements = Vec::new();

        // PC
        elements.push(BinaryElem32::from(self.pc));

        // Instruction word
        elements.push(BinaryElem32::from(self.instruction_word));

        // Merkle siblings
        for sibling in &self.merkle_proof.siblings {
            elements.push(*sibling);
        }

        elements
    }
}

/// Constraint for instruction fetch verification
///
/// Given a trace polynomial, verify that the instruction claimed at step i
/// matches the committed program.
#[derive(Debug, Clone)]
pub struct InstructionFetchConstraint {
    /// PC value for this step
    pub pc: BinaryElem32,

    /// Expected instruction word at pc/4
    pub instruction_word: BinaryElem32,

    /// Merkle siblings for verification
    pub merkle_siblings: Vec<BinaryElem32>,

    /// Expected root (from initial state commitment)
    pub program_root: BinaryElem32,
}

impl InstructionFetchConstraint {
    /// Generate constraint that evaluates to 0 if fetch is valid
    ///
    /// The constraint verifies:
    /// 1. Merkle path from instruction_word to program_root is valid
    /// 2. The path position matches pc/4
    pub fn evaluate(&self) -> BinaryElem32 {
        // Reconstruct root from instruction_word and siblings
        let mut current = hash_leaf_elem(self.instruction_word);
        let mut index = self.pc.poly().value() / 4;

        for sibling in &self.merkle_siblings {
            if index % 2 == 0 {
                current = hash_pair_elem(current, *sibling);
            } else {
                current = hash_pair_elem(*sibling, current);
            }
            index /= 2;
        }

        // Constraint: computed_root XOR expected_root = 0
        current.add(&self.program_root)
    }
}

/// Hash a leaf value for merkle tree
///
/// Must match memory_merkle::hash_leaf exactly
fn hash_leaf_elem(value: BinaryElem32) -> BinaryElem32 {
    use crate::poseidon::PoseidonHash;
    // Domain separator for leaves (matches memory_merkle.rs)
    let elements = vec![
        BinaryElem32::from(0xDEADBEEFu32),
        value,
    ];
    PoseidonHash::hash_elements(&elements)
}

/// Hash two children to get parent
///
/// Must match memory_merkle::hash_pair exactly
fn hash_pair_elem(left: BinaryElem32, right: BinaryElem32) -> BinaryElem32 {
    use crate::poseidon::PoseidonHash;
    // Domain separator for internal nodes (matches memory_merkle.rs)
    let elements = vec![
        BinaryElem32::from(0xCAFEBABEu32),
        left,
        right,
    ];
    PoseidonHash::hash_elements(&elements)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_unified_memory_creation() {
        // Simple 16-byte program
        let program = vec![
            0x13, 0x00, 0x00, 0x00,  // nop
            0x13, 0x00, 0x00, 0x00,  // nop
            0x13, 0x00, 0x00, 0x00,  // nop
            0x13, 0x00, 0x00, 0x00,  // nop
        ];

        let mem = UnifiedMemory::with_program(&program, 1024).unwrap();

        // Should have a root
        let root = mem.root();
        assert!(root != BinaryElem32::zero());
    }

    #[test]
    fn test_instruction_fetch() {
        let program = vec![
            0x13, 0x05, 0x10, 0x00,  // li a0, 1
            0x93, 0x05, 0x20, 0x00,  // li a1, 2
            0x33, 0x06, 0xB5, 0x00,  // add a2, a0, a1
            0x73, 0x00, 0x10, 0x00,  // ebreak
        ];

        let mem = UnifiedMemory::with_program(&program, 1024).unwrap();
        let root = mem.root();

        // Fetch instruction at PC=0
        let fetch = mem.fetch_instruction(0).unwrap();
        assert!(fetch.verify(root));

        // Fetch instruction at PC=4
        let fetch = mem.fetch_instruction(4).unwrap();
        assert!(fetch.verify(root));
    }

    #[test]
    fn test_instruction_fetch_constraint() {
        let program = vec![
            0x13, 0x05, 0x10, 0x00,  // li a0, 1
            0x93, 0x05, 0x20, 0x00,  // li a1, 2
        ];

        let mem = UnifiedMemory::with_program(&program, 1024).unwrap();
        let root = mem.root();

        let fetch = mem.fetch_instruction(0).unwrap();

        // Verify the fetch passes its own verification
        assert!(fetch.verify(root), "fetch.verify() should pass");

        let constraint = InstructionFetchConstraint {
            pc: BinaryElem32::from(0u32),
            instruction_word: BinaryElem32::from(fetch.instruction_word),
            merkle_siblings: fetch.merkle_proof.siblings.clone(),
            program_root: root,
        };

        // Constraint should evaluate to 0 for valid fetch
        let result = constraint.evaluate();
        assert_eq!(result, BinaryElem32::zero(), "valid fetch should give zero constraint");
    }

    #[test]
    fn test_invalid_instruction_fetch_constraint() {
        let program = vec![
            0x13, 0x05, 0x10, 0x00,  // li a0, 1
        ];

        let mem = UnifiedMemory::with_program(&program, 1024).unwrap();
        let root = mem.root();

        let fetch = mem.fetch_instruction(0).unwrap();

        // Create constraint with WRONG instruction word
        let constraint = InstructionFetchConstraint {
            pc: BinaryElem32::from(0u32),
            instruction_word: BinaryElem32::from(0xDEADBEEFu32),  // wrong!
            merkle_siblings: fetch.merkle_proof.siblings.clone(),
            program_root: root,
        };

        // Constraint should NOT evaluate to 0 for invalid fetch
        let result = constraint.evaluate();
        assert_ne!(result, BinaryElem32::zero(), "invalid fetch should give non-zero constraint");
    }
}
