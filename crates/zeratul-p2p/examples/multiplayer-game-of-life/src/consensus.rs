//! Consensus: verify proofs and apply moves

use crate::{MoveTransaction, GameState, GameGenesis, BlockNumber, PlayerId};
use std::collections::HashMap;
use tracing::{info, warn};

/// Consensus engine - verifies proofs and applies valid moves
pub struct Consensus {
    genesis: GameGenesis,
    current_state: GameState,
    pending_moves: HashMap<BlockNumber, Vec<MoveTransaction>>,
}

impl Consensus {
    pub fn new(genesis: GameGenesis) -> Self {
        let state = GameState::new(genesis.grid_width, genesis.grid_height);

        Self {
            genesis,
            current_state: state,
            pending_moves: HashMap::new(),
        }
    }

    /// Add move transaction (will be verified)
    pub fn add_move(&mut self, tx: MoveTransaction) -> Result<(), ConsensusError> {
        // Basic validation
        if tx.x >= self.genesis.grid_width || tx.y >= self.genesis.grid_height {
            return Err(ConsensusError::OutOfBounds);
        }

        // TODO: Verify proof
        // For MVP: accept all moves (proof verification added later)
        info!("Received move from player {:?} at ({}, {}) for block {}",
            &tx.player[..8], tx.x, tx.y, tx.block_number);

        self.pending_moves
            .entry(tx.block_number)
            .or_insert_with(Vec::new)
            .push(tx);

        Ok(())
    }

    /// Finalize block: apply all valid moves and evolve grid
    pub fn finalize_block(&mut self, block: BlockNumber) {
        info!("Finalizing block {}", block);

        // Get moves for this block
        if let Some(moves) = self.pending_moves.remove(&block) {
            info!("Applying {} moves", moves.len());

            // Sort by player ID for deterministic ordering
            let mut sorted_moves = moves;
            sorted_moves.sort_by_key(|m| m.player);

            // Apply moves
            for mv in sorted_moves {
                self.current_state.toggle(mv.x, mv.y);
            }
        }

        // Evolve grid (Conway's rules)
        self.current_state.step();
        self.current_state.block_number = block;

        info!("Block {} finalized, generation {}", block, self.current_state.generation);
    }

    /// Get current game state
    pub fn current_state(&self) -> &GameState {
        &self.current_state
    }

    /// Get current block number based on time
    pub fn current_block(&self, now_ms: u64) -> BlockNumber {
        self.genesis.current_block(now_ms)
    }
}

#[derive(Debug, thiserror::Error)]
pub enum ConsensusError {
    #[error("Move out of bounds")]
    OutOfBounds,

    #[error("Invalid proof")]
    InvalidProof,

    #[error("Wrong block number")]
    WrongBlock,
}
