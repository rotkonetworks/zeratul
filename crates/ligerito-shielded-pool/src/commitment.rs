//! state commitment tree
//!
//! merkle tree of note commitments

#[cfg(not(feature = "std"))]
use alloc::vec::Vec;

use crate::note::NoteCommitment;
use crate::nullifier::Position;

/// merkle root of state commitment tree
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct StateRoot(pub [u8; 32]);

impl StateRoot {
    pub fn to_bytes(&self) -> [u8; 32] {
        self.0
    }

    pub fn from_bytes(bytes: [u8; 32]) -> Self {
        Self(bytes)
    }

    /// empty tree root
    pub fn empty() -> Self {
        Self([0u8; 32])
    }
}

/// merkle inclusion proof
#[derive(Clone, Debug)]
pub struct MerkleProof {
    /// sibling hashes from leaf to root
    pub siblings: Vec<[u8; 32]>,
    /// position of the leaf
    pub position: Position,
}

impl MerkleProof {
    /// verify that commitment is in tree with given root
    pub fn verify(&self, commitment: &NoteCommitment, root: &StateRoot) -> bool {
        let mut current = commitment.0;
        let mut pos = self.position.0;

        for sibling in &self.siblings {
            let (left, right) = if pos & 1 == 0 {
                (&current, sibling)
            } else {
                (sibling, &current)
            };

            let mut hasher = blake3::Hasher::new();
            hasher.update(b"ligerito.merkle.v1");
            hasher.update(left);
            hasher.update(right);
            current = *hasher.finalize().as_bytes();
            pos >>= 1;
        }

        current == root.0
    }
}

/// state commitment tree
/// stores note commitments in a merkle tree
#[cfg(feature = "std")]
pub struct StateCommitmentTree {
    /// all commitments (leaves)
    leaves: Vec<NoteCommitment>,
    /// cached tree nodes (for efficient root computation)
    nodes: Vec<Vec<[u8; 32]>>,
    /// tree depth
    depth: usize,
}

#[cfg(feature = "std")]
impl StateCommitmentTree {
    /// create empty tree with given depth
    pub fn new(depth: usize) -> Self {
        Self {
            leaves: Vec::new(),
            nodes: vec![Vec::new(); depth + 1],
            depth,
        }
    }

    /// current root
    pub fn root(&self) -> StateRoot {
        if self.leaves.is_empty() {
            return StateRoot::empty();
        }

        self.compute_root()
    }

    /// insert a note commitment, returns its position
    pub fn insert(&mut self, commitment: NoteCommitment) -> Position {
        let pos = Position::new(self.leaves.len() as u64);
        self.leaves.push(commitment);
        // invalidate cache (in production, do incremental updates)
        self.nodes = vec![Vec::new(); self.depth + 1];
        pos
    }

    /// get merkle proof for a position
    pub fn prove(&self, position: Position) -> Option<MerkleProof> {
        let pos = position.0 as usize;
        if pos >= self.leaves.len() {
            return None;
        }

        let mut siblings = Vec::with_capacity(self.depth);
        let mut level = self.leaves.iter().map(|c| c.0).collect::<Vec<_>>();
        let mut current_pos = pos;

        for _ in 0..self.depth {
            // pad to even length
            if level.len() % 2 == 1 {
                level.push([0u8; 32]);
            }

            // get sibling
            let sibling_pos = if current_pos % 2 == 0 {
                current_pos + 1
            } else {
                current_pos - 1
            };

            if sibling_pos < level.len() {
                siblings.push(level[sibling_pos]);
            } else {
                siblings.push([0u8; 32]);
            }

            // compute next level
            let mut next_level = Vec::with_capacity(level.len() / 2);
            for chunk in level.chunks(2) {
                let mut hasher = blake3::Hasher::new();
                hasher.update(b"ligerito.merkle.v1");
                hasher.update(&chunk[0]);
                hasher.update(chunk.get(1).unwrap_or(&[0u8; 32]));
                next_level.push(*hasher.finalize().as_bytes());
            }
            level = next_level;
            current_pos /= 2;
        }

        Some(MerkleProof {
            siblings,
            position,
        })
    }

    fn compute_root(&self) -> StateRoot {
        if self.leaves.is_empty() {
            return StateRoot::empty();
        }

        let mut level = self.leaves.iter().map(|c| c.0).collect::<Vec<_>>();

        for _ in 0..self.depth {
            if level.len() % 2 == 1 {
                level.push([0u8; 32]);
            }

            let mut next_level = Vec::with_capacity(level.len() / 2);
            for chunk in level.chunks(2) {
                let mut hasher = blake3::Hasher::new();
                hasher.update(b"ligerito.merkle.v1");
                hasher.update(&chunk[0]);
                hasher.update(chunk.get(1).unwrap_or(&[0u8; 32]));
                next_level.push(*hasher.finalize().as_bytes());
            }
            level = next_level;
        }

        StateRoot(level.get(0).copied().unwrap_or([0u8; 32]))
    }

    /// number of notes in tree
    pub fn len(&self) -> usize {
        self.leaves.len()
    }

    pub fn is_empty(&self) -> bool {
        self.leaves.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_merkle_tree() {
        let mut tree = StateCommitmentTree::new(16);

        // empty tree
        assert_eq!(tree.root(), StateRoot::empty());

        // insert some commitments
        let c1 = NoteCommitment([1u8; 32]);
        let c2 = NoteCommitment([2u8; 32]);
        let c3 = NoteCommitment([3u8; 32]);

        let p1 = tree.insert(c1);
        let root1 = tree.root();

        let p2 = tree.insert(c2);
        let root2 = tree.root();

        let p3 = tree.insert(c3);
        let root3 = tree.root();

        // roots should change
        assert_ne!(root1, root2);
        assert_ne!(root2, root3);

        // proofs should verify
        let proof1 = tree.prove(p1).unwrap();
        assert!(proof1.verify(&c1, &root3));

        let proof2 = tree.prove(p2).unwrap();
        assert!(proof2.verify(&c2, &root3));

        let proof3 = tree.prove(p3).unwrap();
        assert!(proof3.verify(&c3, &root3));

        // wrong commitment should fail
        assert!(!proof1.verify(&c2, &root3));
    }
}
