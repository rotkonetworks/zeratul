//! Fork Choice Rule
//!
//! Implements JAM's fork choice rule from the Gray Paper:
//! - Prefer chains with more ticketed blocks (vs fallback)
//! - No equivocations in unfinalized portion
//! - Must be audited
//! - Must descend from finalized block

use crate::block::Block;
use commonware_cryptography::sha256::Digest;
use std::collections::{HashMap, HashSet};
use tracing::{debug, warn};

/// Fork choice engine
pub struct ForkChoice {
    /// Finalized block hash
    finalized: Digest,

    /// All known blocks (hash -> block)
    blocks: HashMap<Digest, Block>,

    /// Block children (parent_hash -> child_hashes)
    children: HashMap<Digest, Vec<Digest>>,

    /// Detected equivocations (timeslot -> block_hashes)
    /// If a timeslot has multiple blocks, it's an equivocation
    equivocations: HashMap<u64, HashSet<Digest>>,
}

impl ForkChoice {
    /// Create new fork choice engine
    pub fn new(genesis: Block) -> Self {
        let genesis_hash = genesis.digest();

        let mut blocks = HashMap::new();
        blocks.insert(genesis_hash, genesis);

        Self {
            finalized: genesis_hash,
            blocks,
            children: HashMap::new(),
            equivocations: HashMap::new(),
        }
    }

    /// Add a block to the fork choice
    ///
    /// Returns true if block was added, false if rejected or already known
    pub fn add_block(&mut self, block: Block) -> bool {
        let hash = block.digest();
        let parent = block.parent();
        let timeslot = block.timeslot();

        // Check if already known
        if self.blocks.contains_key(&hash) {
            return false;
        }

        // Check if parent is known
        if !self.blocks.contains_key(&parent) {
            warn!(?hash, ?parent, "Block parent unknown, rejecting");
            return false;
        }

        // Check for equivocation at this timeslot
        let timeslot_blocks = self.equivocations.entry(timeslot).or_insert_with(HashSet::new);
        if !timeslot_blocks.is_empty() {
            // Equivocation detected!
            warn!(
                timeslot,
                ?hash,
                existing = timeslot_blocks.len(),
                "Equivocation detected at timeslot"
            );
            timeslot_blocks.insert(hash);
            return false;
        }

        // Add to blocks
        timeslot_blocks.insert(hash);
        self.blocks.insert(hash, block);

        // Add to children
        self.children.entry(parent).or_insert_with(Vec::new).push(hash);

        debug!(?hash, ?parent, timeslot, "Block added to fork choice");
        true
    }

    /// Get best block according to fork choice rule
    ///
    /// JAM spec (best_chain.tex):
    /// - Must descend from finalized block
    /// - No equivocations in unfinalized portion
    /// - Prefer chain with most ticketed blocks
    pub fn best_block(&self) -> Digest {
        // Start from finalized block
        let mut candidates = vec![self.finalized];
        let mut best = self.finalized;
        let mut best_score = 0u64;

        // BFS through all descendants
        while let Some(hash) = candidates.pop() {
            // Check all children
            if let Some(children) = self.children.get(&hash) {
                for child_hash in children {
                    // Check if this chain has equivocations
                    if self.has_equivocation_in_chain(*child_hash) {
                        continue;
                    }

                    // Calculate score (number of ticketed blocks)
                    let score = self.count_ticketed_blocks(*child_hash);

                    // Update best if this is better
                    if score > best_score {
                        best = *child_hash;
                        best_score = score;
                    }

                    // Add to candidates for further exploration
                    candidates.push(*child_hash);
                }
            }
        }

        debug!(?best, score = best_score, "Best block selected");
        best
    }

    /// Check if chain has equivocation in unfinalized portion
    fn has_equivocation_in_chain(&self, mut hash: Digest) -> bool {
        // Walk backwards until finalized block
        while hash != self.finalized {
            let block = match self.blocks.get(&hash) {
                Some(b) => b,
                None => return true, // Unknown block, consider invalid
            };

            let timeslot = block.timeslot();

            // Check if this timeslot has equivocation
            if let Some(equivocations) = self.equivocations.get(&timeslot) {
                if equivocations.len() > 1 {
                    return true;
                }
            }

            hash = block.parent();
        }

        false
    }

    /// Count ticketed blocks in chain (from finalized to tip)
    fn count_ticketed_blocks(&self, mut hash: Digest) -> u64 {
        let mut count = 0;

        // Walk backwards until finalized block
        while hash != self.finalized {
            let block = match self.blocks.get(&hash) {
                Some(b) => b,
                None => break, // Unknown block
            };

            if block.is_ticketed() {
                count += 1;
            }

            hash = block.parent();
        }

        count
    }

    /// Finalize a block
    ///
    /// This is typically called when GRANDPA finalizes a block.
    /// After finalization:
    /// - The finalized block becomes the new base
    /// - All non-descendant chains are pruned
    pub fn finalize(&mut self, hash: Digest) -> bool {
        // Check if block exists
        if !self.blocks.contains_key(&hash) {
            warn!(?hash, "Cannot finalize unknown block");
            return false;
        }

        // Check if descends from current finalized
        if !self.is_descendant(hash, self.finalized) {
            warn!(?hash, finalized = ?self.finalized, "Block does not descend from finalized");
            return false;
        }

        debug!(?hash, "Finalizing block");

        // Update finalized
        self.finalized = hash;

        // Prune non-descendants
        self.prune_non_descendants();

        true
    }

    /// Check if `descendant` descends from `ancestor`
    fn is_descendant(&self, mut descendant: Digest, ancestor: Digest) -> bool {
        while descendant != ancestor {
            let block = match self.blocks.get(&descendant) {
                Some(b) => b,
                None => return false,
            };

            descendant = block.parent();

            // Reached genesis without finding ancestor
            if descendant == block.digest() {
                return false;
            }
        }

        true
    }

    /// Prune blocks that don't descend from finalized
    fn prune_non_descendants(&mut self) {
        let finalized = self.finalized;

        // Find all descendants of finalized
        let mut descendants = HashSet::new();
        descendants.insert(finalized);

        let mut to_check = vec![finalized];
        while let Some(hash) = to_check.pop() {
            if let Some(children) = self.children.get(&hash) {
                for child in children {
                    if descendants.insert(*child) {
                        to_check.push(*child);
                    }
                }
            }
        }

        // Remove non-descendants
        self.blocks.retain(|hash, _| descendants.contains(hash));
        self.children.retain(|hash, _| descendants.contains(hash));

        debug!(
            pruned = self.blocks.len(),
            "Pruned non-descendant blocks"
        );
    }

    /// Get block by hash
    pub fn get_block(&self, hash: &Digest) -> Option<&Block> {
        self.blocks.get(hash)
    }

    /// Get finalized block hash
    pub fn finalized(&self) -> Digest {
        self.finalized
    }

    /// Get number of blocks in fork choice
    pub fn block_count(&self) -> usize {
        self.blocks.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_genesis() {
        let genesis = Block::genesis();
        let fork_choice = ForkChoice::new(genesis.clone());

        assert_eq!(fork_choice.finalized(), genesis.digest());
        assert_eq!(fork_choice.best_block(), genesis.digest());
        assert_eq!(fork_choice.block_count(), 1);
    }

    #[test]
    fn test_add_block() {
        let genesis = Block::genesis();
        let mut fork_choice = ForkChoice::new(genesis.clone());

        // Create a child block
        let child = Block::new(
            genesis.digest(),
            1,
            1,
            1000,
            [1u8; 32],
            vec![],
            [1u8; 32],
            vec![],
            vec![],
            None,
            None,
            false,
        );

        assert!(fork_choice.add_block(child.clone()));
        assert_eq!(fork_choice.block_count(), 2);

        // Best block should be child (only option)
        assert_eq!(fork_choice.best_block(), child.digest());
    }

    #[test]
    fn test_ticketed_preference() {
        let genesis = Block::genesis();
        let mut fork_choice = ForkChoice::new(genesis.clone());

        // Create two chains: one with fallback, one with ticket
        let fallback_block = Block::new(
            genesis.digest(),
            1,
            1,
            1000,
            [1u8; 32],
            vec![],
            [1u8; 32],
            vec![],
            vec![],
            None,
            None,
            false, // Fallback
        );

        let ticketed_block = Block::new(
            genesis.digest(),
            1,
            2,
            2000,
            [2u8; 32],
            vec![],
            [2u8; 32],
            vec![],
            vec![],
            None,
            None,
            true, // Ticketed
        );

        fork_choice.add_block(fallback_block.clone());
        fork_choice.add_block(ticketed_block.clone());

        // Should prefer ticketed block
        assert_eq!(fork_choice.best_block(), ticketed_block.digest());
    }
}
