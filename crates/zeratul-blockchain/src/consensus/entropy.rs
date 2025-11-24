//! Entropy Accumulation
//!
//! Accumulates randomness from VRF outputs for protocol use.

use anyhow::Result;
use serde::{Deserialize, Serialize};

/// Hash type (32 bytes)
pub type Hash = [u8; 32];

/// Entropy accumulator
///
/// Accumulates VRF outputs to generate high-quality randomness.
/// Used for:
/// - Ticket verification (prevents bias)
/// - Fallback key selection
/// - General protocol randomness
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct EntropyAccumulator {
    /// Current accumulator value
    pub current: Hash,

    /// Entropy at end of last epoch (epoch N-1)
    pub epoch_1: Hash,

    /// Entropy at end of 2 epochs ago (epoch N-2)
    pub epoch_2: Hash,

    /// Entropy at end of 3 epochs ago (epoch N-3)
    pub epoch_3: Hash,
}

impl EntropyAccumulator {
    /// Create new accumulator with genesis entropy
    pub fn new(genesis_entropy: Hash) -> Self {
        Self {
            current: genesis_entropy,
            epoch_1: genesis_entropy,
            epoch_2: genesis_entropy,
            epoch_3: genesis_entropy,
        }
    }

    /// Accumulate VRF output
    ///
    /// Each block adds VRF output to accumulator via Blake3 hash
    pub fn accumulate(&mut self, vrf_output: &Hash) {
        let mut data = Vec::with_capacity(64);
        data.extend_from_slice(&self.current);
        data.extend_from_slice(vrf_output);

        let hash = blake3::hash(&data);
        self.current = *hash.as_bytes();
    }

    /// Rotate entropy on epoch change
    ///
    /// When new epoch starts, rotate history:
    /// - current → epoch_1
    /// - epoch_1 → epoch_2
    /// - epoch_2 → epoch_3
    pub fn rotate_epoch(&mut self) {
        self.epoch_3 = self.epoch_2;
        self.epoch_2 = self.epoch_1;
        self.epoch_1 = self.current;
    }

    /// Get entropy for ticket verification (epoch_2)
    pub fn ticket_entropy(&self) -> &Hash {
        &self.epoch_2
    }

    /// Get entropy for fallback keys (epoch_2)
    pub fn fallback_entropy(&self) -> &Hash {
        &self.epoch_2
    }

    /// Get entropy for seal verification (epoch_3)
    pub fn seal_entropy(&self) -> &Hash {
        &self.epoch_3
    }

    /// Get current entropy
    pub fn current_entropy(&self) -> &Hash {
        &self.current
    }

    /// Get epoch entropy by index
    /// - 0: current
    /// - 1: epoch_1 (last epoch)
    /// - 2: epoch_2 (2 epochs ago)
    /// - 3: epoch_3 (3 epochs ago)
    pub fn epoch_entropy(&self, index: usize) -> &Hash {
        match index {
            0 => &self.current,
            1 => &self.epoch_1,
            2 => &self.epoch_2,
            3 => &self.epoch_3,
            _ => &self.epoch_3, // Default to oldest
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_accumulate() {
        let genesis = [0u8; 32];
        let mut acc = EntropyAccumulator::new(genesis);

        let vrf_output = [1u8; 32];
        acc.accumulate(&vrf_output);

        // Should have changed
        assert_ne!(acc.current, genesis);

        // Epoch history unchanged
        assert_eq!(acc.epoch_1, genesis);
        assert_eq!(acc.epoch_2, genesis);
        assert_eq!(acc.epoch_3, genesis);
    }

    #[test]
    fn test_rotate_epoch() {
        let genesis = [0u8; 32];
        let mut acc = EntropyAccumulator::new(genesis);

        // Accumulate some entropy
        acc.accumulate(&[1u8; 32]);
        let after_accumulate = acc.current;

        // Rotate epoch
        acc.rotate_epoch();

        // Current moved to epoch_1
        assert_eq!(acc.epoch_1, after_accumulate);
        assert_eq!(acc.epoch_2, genesis);
        assert_eq!(acc.epoch_3, genesis);

        // Accumulate more and rotate again
        acc.accumulate(&[2u8; 32]);
        let after_second = acc.current;
        acc.rotate_epoch();

        // Check rotation
        assert_eq!(acc.epoch_1, after_second);
        assert_eq!(acc.epoch_2, after_accumulate);
        assert_eq!(acc.epoch_3, genesis);
    }

    #[test]
    fn test_deterministic() {
        let genesis = [42u8; 32];
        let mut acc1 = EntropyAccumulator::new(genesis);
        let mut acc2 = EntropyAccumulator::new(genesis);

        // Same inputs → same outputs
        for i in 0..10 {
            let vrf = [i as u8; 32];
            acc1.accumulate(&vrf);
            acc2.accumulate(&vrf);
        }

        assert_eq!(acc1.current, acc2.current);
    }
}
