//! wallet state types and sparse merkle tree operations

use blake2::{Blake2b512, Digest};
use serde::{Deserialize, Serialize};

use crate::{
    DOMAIN_WALLET_STATE, EMPTY_SMT_ROOT, 
    ORCHARD_ACTIVATION_HEIGHT, GENESIS_EPOCH_HASH,
    Result, ZyncError,
};

/// commitment to wallet state (32 bytes)
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct WalletStateCommitment(pub [u8; 32]);

impl WalletStateCommitment {
    pub fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }
}

impl AsRef<[u8]> for WalletStateCommitment {
    fn as_ref(&self) -> &[u8] {
        &self.0
    }
}

/// full wallet state
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WalletState {
    /// sparse merkle root: nullifier -> bool (has been seen)
    pub nullifier_set_root: [u8; 32],

    /// sparse merkle root: note_commitment -> NoteData
    pub owned_notes_root: [u8; 32],

    /// incremental merkle tree frontier for note commitment tree
    /// allows reconstructing merkle paths for spending
    /// stored as frontier hashes at each level
    pub note_tree_frontier: NoteFrontier,

    /// chain position
    pub block_height: u32,
    pub block_hash: [u8; 32],
}

impl WalletState {
    /// genesis state for fresh wallet (at orchard activation)
    pub fn genesis() -> Self {
        Self {
            nullifier_set_root: EMPTY_SMT_ROOT,
            owned_notes_root: EMPTY_SMT_ROOT,
            note_tree_frontier: NoteFrontier::empty(),
            block_height: ORCHARD_ACTIVATION_HEIGHT,
            block_hash: [0u8; 32], // filled in during first sync
        }
    }

    /// genesis state for testnet
    pub fn genesis_testnet() -> Self {
        Self {
            block_height: crate::ORCHARD_ACTIVATION_HEIGHT_TESTNET,
            ..Self::genesis()
        }
    }

    /// commit to state (domain-separated blake2b)
    pub fn commit(&self) -> WalletStateCommitment {
        let mut hasher = Blake2b512::new();
        hasher.update(DOMAIN_WALLET_STATE);
        hasher.update(&self.nullifier_set_root);
        hasher.update(&self.owned_notes_root);
        hasher.update(&self.note_tree_frontier.root());
        hasher.update(&self.block_height.to_le_bytes());
        hasher.update(&self.block_hash);
        
        let hash = hasher.finalize();
        let mut commitment = [0u8; 32];
        commitment.copy_from_slice(&hash[..32]);
        WalletStateCommitment(commitment)
    }

    /// check if this is genesis state
    pub fn is_genesis(&self) -> bool {
        self.nullifier_set_root == EMPTY_SMT_ROOT
            && self.owned_notes_root == EMPTY_SMT_ROOT
    }
}

/// incremental merkle tree frontier
/// stores the rightmost path for efficient appends
/// depth 32 for orchard note commitment tree
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NoteFrontier {
    /// position of next leaf to insert
    pub position: u64,
    
    /// frontier hashes at each level (32 levels for orchard)
    /// frontier[i] is Some if there's an odd node at level i
    pub frontier: [Option<[u8; 32]>; 32],
}

impl NoteFrontier {
    pub fn empty() -> Self {
        Self {
            position: 0,
            frontier: [None; 32],
        }
    }

    /// compute merkle root from frontier
    pub fn root(&self) -> [u8; 32] {
        if self.position == 0 {
            return EMPTY_SMT_ROOT;
        }

        let mut current = [0u8; 32];
        let mut have_current = false;

        for (level, frontier_hash) in self.frontier.iter().enumerate() {
            match (frontier_hash, have_current) {
                (Some(left), false) => {
                    // frontier hash becomes current
                    current = *left;
                    have_current = true;
                }
                (Some(left), true) => {
                    // hash frontier with current
                    current = hash_merkle_node(level, left, &current);
                }
                (None, true) => {
                    // hash current with empty
                    current = hash_merkle_node(level, &current, &empty_hash(level));
                }
                (None, false) => {
                    // nothing at this level
                }
            }
        }

        if have_current {
            current
        } else {
            EMPTY_SMT_ROOT
        }
    }

    /// append a leaf to the tree, returning new position
    pub fn append(&mut self, leaf: [u8; 32]) -> u64 {
        let pos = self.position;
        self.position += 1;

        let mut current = leaf;
        for level in 0..32 {
            if pos & (1 << level) == 0 {
                // this level was empty, store and stop
                self.frontier[level] = Some(current);
                return pos;
            } else {
                // this level had a node, hash together and continue
                let left = self.frontier[level].take().unwrap_or_else(|| empty_hash(level));
                current = hash_merkle_node(level, &left, &current);
            }
        }

        pos
    }

    /// get current position (number of leaves)
    pub fn position(&self) -> u64 {
        self.position
    }
}

/// hash two merkle nodes at given level
fn hash_merkle_node(level: usize, left: &[u8; 32], right: &[u8; 32]) -> [u8; 32] {
    let mut hasher = Blake2b512::new();
    hasher.update(b"ZYNC_merkle_node");
    hasher.update(&[level as u8]);
    hasher.update(left);
    hasher.update(right);
    
    let hash = hasher.finalize();
    let mut result = [0u8; 32];
    result.copy_from_slice(&hash[..32]);
    result
}

/// empty hash at given level (cached in practice)
fn empty_hash(level: usize) -> [u8; 32] {
    // todo: precompute and cache these
    if level == 0 {
        [0u8; 32]
    } else {
        let child = empty_hash(level - 1);
        hash_merkle_node(level - 1, &child, &child)
    }
}

/// sparse merkle tree operations
pub mod smt {
    use super::*;

    /// insert key-value pair into SMT, return new root
    pub fn insert(
        root: [u8; 32],
        key: &[u8; 32],
        value: &[u8],
    ) -> [u8; 32] {
        // simplified: just hash key||value||old_root
        // full implementation would track actual tree structure
        let mut hasher = Blake2b512::new();
        hasher.update(b"ZYNC_smt_insert");
        hasher.update(&root);
        hasher.update(key);
        hasher.update(value);
        
        let hash = hasher.finalize();
        let mut new_root = [0u8; 32];
        new_root.copy_from_slice(&hash[..32]);
        new_root
    }

    /// verify membership proof
    pub fn verify_membership(
        root: &[u8; 32],
        key: &[u8; 32],
        value: &[u8],
        proof: &SmtProof,
    ) -> bool {
        // todo: implement actual merkle path verification
        // for now just check proof isn't empty
        !proof.siblings.is_empty()
    }

    /// SMT membership proof
    #[derive(Debug, Clone, Serialize, Deserialize)]
    pub struct SmtProof {
        pub siblings: Vec<[u8; 32]>,
        pub path_bits: Vec<bool>,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_genesis_state() {
        let state = WalletState::genesis();
        assert!(state.is_genesis());
        assert_eq!(state.block_height, ORCHARD_ACTIVATION_HEIGHT);
    }

    #[test]
    fn test_state_commitment_deterministic() {
        let state = WalletState::genesis();
        let c1 = state.commit();
        let c2 = state.commit();
        assert_eq!(c1, c2);
    }

    #[test]
    fn test_frontier_empty() {
        let frontier = NoteFrontier::empty();
        assert_eq!(frontier.position(), 0);
        assert_eq!(frontier.root(), EMPTY_SMT_ROOT);
    }

    #[test]
    fn test_frontier_append() {
        let mut frontier = NoteFrontier::empty();
        
        let leaf1 = [1u8; 32];
        let pos1 = frontier.append(leaf1);
        assert_eq!(pos1, 0);
        assert_eq!(frontier.position(), 1);
        
        let leaf2 = [2u8; 32];
        let pos2 = frontier.append(leaf2);
        assert_eq!(pos2, 1);
        assert_eq!(frontier.position(), 2);
        
        // root should be hash of the two leaves
        let root = frontier.root();
        assert_ne!(root, EMPTY_SMT_ROOT);
    }
}
