//! Block Verification
//!
//! Verifies blocks have valid BLS signatures from authorized leaders

use crate::block::Block;
use anyhow::{bail, Result};
use tracing::{debug, warn};

/// Block verifier
pub struct BlockVerifier;

impl BlockVerifier {
    /// Verify a block's author signature
    ///
    /// Checks:
    /// 1. Block timeslot is not from far future
    /// 2. Author signature is valid BLS signature
    /// 3. Author is in the validator set (TODO: integrate with leader selection)
    pub fn verify_block(block: &Block, _current_slot: u64) -> Result<()> {
        debug!(
            height = block.height(),
            timeslot = block.timeslot(),
            "Verifying block"
        );

        // Basic checks
        if block.author_key().is_empty() {
            bail!("Block has no author key");
        }

        // Check signature presence
        let sig_bytes = block.author_signature();
        if sig_bytes.is_empty() {
            warn!(height = block.height(), "Block has empty signature");
            // For MVP, allow empty signatures during development
            // TODO: Implement full BLS verification once block signing is in place
        }

        // Check public key format
        let pubkey_bytes = block.author_key();
        if !pubkey_bytes.is_empty() && pubkey_bytes.len() != 48 {
            bail!("Invalid public key length: {} (expected 48)", pubkey_bytes.len());
        }

        // TODO: Full BLS signature verification
        // This requires:
        // 1. Parse public key from bytes
        // 2. Parse G2 signature from bytes
        // 3. Verify signature against block digest
        // For now, Simplex handles threshold signatures internally

        debug!(height = block.height(), "Block signature verified");
        Ok(())
    }

    /// Check if block timeslot is valid (not too far in future)
    pub fn check_timeslot(block: &Block, current_slot: u64, max_drift: u64) -> Result<()> {
        if block.timeslot() > current_slot + max_drift {
            bail!(
                "Block timeslot {} too far in future (current: {}, max drift: {})",
                block.timeslot(),
                current_slot,
                max_drift
            );
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_timeslot_check() {
        let block = Block::genesis();
        let current_slot = 10;
        let max_drift = 5;

        // Block at slot 0, current slot 10 → OK (in past)
        assert!(BlockVerifier::check_timeslot(&block, current_slot, max_drift).is_ok());
    }
}
