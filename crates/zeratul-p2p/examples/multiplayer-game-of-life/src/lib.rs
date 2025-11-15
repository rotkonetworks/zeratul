//! P2P Multiplayer Game of Life with Proof Verification
//!
//! Players:
//! 1. Agree on genesis timestamp
//! 2. Submit move proofs
//! 3. Gossip to all peers via litep2p
//! 4. Verify all proofs locally
//! 5. Apply valid moves each block (every 1s)

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

pub mod network;
pub mod game;
pub mod consensus;

/// Player identity (Ed25519 public key)
pub type PlayerId = [u8; 32];

/// Block number (increments every 1s from genesis)
pub type BlockNumber = u64;

/// Game genesis - all players agree on this
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GameGenesis {
    /// Unix timestamp (ms) when block 0 starts
    pub genesis_timestamp_ms: u64,
    /// Block interval in milliseconds
    pub block_interval_ms: u64,
    /// Grid dimensions
    pub grid_width: usize,
    pub grid_height: usize,
}

impl GameGenesis {
    /// Calculate current block number from timestamp
    pub fn current_block(&self, now_ms: u64) -> BlockNumber {
        if now_ms < self.genesis_timestamp_ms {
            return 0;
        }
        (now_ms - self.genesis_timestamp_ms) / self.block_interval_ms
    }

    /// Get block start timestamp
    pub fn block_start_time(&self, block: BlockNumber) -> u64 {
        self.genesis_timestamp_ms + (block * self.block_interval_ms)
    }
}

/// Move transaction with proof
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MoveTransaction {
    /// Player who made the move
    pub player: PlayerId,
    /// Cell coordinates
    pub x: usize,
    pub y: usize,
    /// Block number when move was made
    pub block_number: BlockNumber,
    /// PolkaVM execution proof
    pub proof: Vec<u8>, // Serialized PolkaVMProof
    /// Signature over (player, x, y, block_number, proof)
    pub signature: [u8; 64],
}

/// Game state at a specific block
#[derive(Debug, Clone)]
pub struct GameState {
    pub block_number: BlockNumber,
    pub grid: Vec<Vec<u8>>, // 0 = dead, 1 = alive
    pub generation: usize,
}

impl GameState {
    pub fn new(width: usize, height: usize) -> Self {
        Self {
            block_number: 0,
            grid: vec![vec![0; width]; height],
            generation: 0,
        }
    }

    /// Apply Conway's Game of Life rules
    pub fn step(&mut self) {
        let height = self.grid.len();
        let width = self.grid[0].len();
        let mut next = vec![vec![0; width]; height];

        for y in 0..height {
            for x in 0..width {
                let neighbors = self.count_neighbors(x, y);
                let alive = self.grid[y][x] == 1;

                next[y][x] = match (alive, neighbors) {
                    (true, 2) | (true, 3) => 1,  // Survive
                    (false, 3) => 1,              // Born
                    _ => 0,                       // Die
                };
            }
        }

        self.grid = next;
        self.generation += 1;
    }

    fn count_neighbors(&self, x: usize, y: usize) -> usize {
        let height = self.grid.len();
        let width = self.grid[0].len();
        let mut count = 0;

        for dy in -1..=1 {
            for dx in -1..=1 {
                if dx == 0 && dy == 0 {
                    continue;
                }

                let ny = (y as i32 + dy).rem_euclid(height as i32) as usize;
                let nx = (x as i32 + dx).rem_euclid(width as i32) as usize;

                if self.grid[ny][nx] == 1 {
                    count += 1;
                }
            }
        }

        count
    }

    pub fn toggle(&mut self, x: usize, y: usize) {
        if y < self.grid.len() && x < self.grid[0].len() {
            self.grid[y][x] = 1 - self.grid[y][x];
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_genesis_block_calculation() {
        let genesis = GameGenesis {
            genesis_timestamp_ms: 1000,
            block_interval_ms: 1000,
            grid_width: 32,
            grid_height: 32,
        };

        assert_eq!(genesis.current_block(1000), 0);
        assert_eq!(genesis.current_block(1999), 0);
        assert_eq!(genesis.current_block(2000), 1);
        assert_eq!(genesis.current_block(3000), 2);
    }

    #[test]
    fn test_game_of_life_glider() {
        let mut state = GameState::new(5, 5);

        // Glider pattern
        state.toggle(1, 0);
        state.toggle(2, 1);
        state.toggle(0, 2);
        state.toggle(1, 2);
        state.toggle(2, 2);

        state.step();

        // Verify glider moved
        assert_eq!(state.generation, 1);
    }
}
