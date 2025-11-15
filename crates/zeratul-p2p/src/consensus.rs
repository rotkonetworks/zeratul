//! Consensus primitives for low-latency applications
//!
//! Key properties for trading:
//! - Sub-millisecond proof verification (512μs)
//! - QUIC transport (UDP, ~10-50ms latency)
//! - Deterministic ordering via block numbers
//! - No centralized sequencer needed

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use thiserror::Error;

/// Block number (increments every N milliseconds from genesis)
pub type BlockNumber = u64;

/// Transaction with proof
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProvenTransaction<T> {
    /// Transaction data
    pub data: T,
    /// Block number when submitted
    pub block_number: BlockNumber,
    /// Execution proof (PolkaVM or other)
    pub proof: Vec<u8>,
    /// Submitter signature
    pub signature: Vec<u8>,
}

/// Genesis configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Genesis {
    /// Unix timestamp (ms) when block 0 starts
    pub genesis_timestamp_ms: u64,
    /// Block interval in milliseconds
    pub block_interval_ms: u64,
}

impl Genesis {
    /// Create genesis for low-latency trading (100ms blocks)
    pub fn trading_100ms(start_time_ms: u64) -> Self {
        Self {
            genesis_timestamp_ms: start_time_ms,
            block_interval_ms: 100, // 10 blocks/second
        }
    }

    /// Create genesis for standard applications (1s blocks)
    pub fn standard_1s(start_time_ms: u64) -> Self {
        Self {
            genesis_timestamp_ms: start_time_ms,
            block_interval_ms: 1000, // 1 block/second
        }
    }

    /// Calculate current block from timestamp
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

/// Consensus state machine
pub struct ConsensusEngine<T, S> {
    genesis: Genesis,
    current_state: S,
    pending_txs: HashMap<BlockNumber, Vec<ProvenTransaction<T>>>,
    verifier: Box<dyn Fn(&ProvenTransaction<T>) -> bool + Send + Sync>,
    state_transition: Box<dyn Fn(&mut S, &T) + Send + Sync>,
}

impl<T, S> ConsensusEngine<T, S>
where
    T: Clone + Send + Sync,
    S: Clone + Send + Sync,
{
    pub fn new(
        genesis: Genesis,
        initial_state: S,
        verifier: impl Fn(&ProvenTransaction<T>) -> bool + Send + Sync + 'static,
        state_transition: impl Fn(&mut S, &T) + Send + Sync + 'static,
    ) -> Self {
        Self {
            genesis,
            current_state: initial_state,
            pending_txs: HashMap::new(),
            verifier: Box::new(verifier),
            state_transition: Box::new(state_transition),
        }
    }

    /// Add transaction (will be verified)
    pub fn add_transaction(&mut self, tx: ProvenTransaction<T>) -> Result<(), ConsensusError> {
        // Verify proof
        if !(self.verifier)(&tx) {
            return Err(ConsensusError::InvalidProof);
        }

        // Add to pending
        self.pending_txs
            .entry(tx.block_number)
            .or_insert_with(Vec::new)
            .push(tx);

        Ok(())
    }

    /// Finalize block: apply all valid transactions
    pub fn finalize_block(&mut self, block: BlockNumber) {
        if let Some(txs) = self.pending_txs.remove(&block) {
            // Deterministic ordering (by signature for now)
            let mut sorted_txs = txs;
            sorted_txs.sort_by(|a, b| a.signature.cmp(&b.signature));

            // Apply state transitions
            for tx in sorted_txs {
                (self.state_transition)(&mut self.current_state, &tx.data);
            }
        }
    }

    /// Get current state
    pub fn state(&self) -> &S {
        &self.current_state
    }

    /// Get current block
    pub fn current_block(&self, now_ms: u64) -> BlockNumber {
        self.genesis.current_block(now_ms)
    }
}

#[derive(Debug, Error)]
pub enum ConsensusError {
    #[error("Invalid proof")]
    InvalidProof,

    #[error("Wrong block number")]
    WrongBlock,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_100ms_blocks() {
        let genesis = Genesis::trading_100ms(1000);

        assert_eq!(genesis.current_block(1000), 0);
        assert_eq!(genesis.current_block(1099), 0);
        assert_eq!(genesis.current_block(1100), 1); // 100ms later
        assert_eq!(genesis.current_block(1200), 2);
        assert_eq!(genesis.current_block(2000), 10); // 1 second = 10 blocks
    }

    #[test]
    fn test_block_timestamps() {
        let genesis = Genesis::trading_100ms(1000);

        assert_eq!(genesis.block_start_time(0), 1000);
        assert_eq!(genesis.block_start_time(1), 1100);
        assert_eq!(genesis.block_start_time(10), 2000);
    }
}
