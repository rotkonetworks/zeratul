//! proof verification using ligerito

use anyhow::Result;
use ligerito::{VerifierConfig, FinalizedLigeritoProof, verify};
use ligerito_binary_fields::{BinaryElem32, BinaryElem128};
use tracing::{info, debug};
use std::time::Instant;

pub struct ProofVerifier {
    gigaproof_config: VerifierConfig,
    tip_config: VerifierConfig,
}

impl ProofVerifier {
    pub fn new() -> Self {
        Self {
            gigaproof_config: zync_core::gigaproof_verifier_config(),
            tip_config: zync_core::tip_verifier_config(),
        }
    }

    /// verify combined gigaproof + tip proof
    /// returns (gigaproof_valid, tip_valid)
    pub fn verify_proofs(&self, combined_proof: &[u8]) -> Result<(bool, bool)> {
        info!("verifying proof ({} bytes)", combined_proof.len());
        let start = Instant::now();

        // split combined proof into gigaproof + tip
        // format: [gigaproof_size: u32][gigaproof_bytes][tip_bytes]
        if combined_proof.len() < 4 {
            anyhow::bail!("proof too small");
        }

        let gigaproof_size = u32::from_le_bytes([
            combined_proof[0],
            combined_proof[1],
            combined_proof[2],
            combined_proof[3],
        ]) as usize;

        if combined_proof.len() < 4 + gigaproof_size {
            anyhow::bail!("invalid proof format");
        }

        let gigaproof_bytes = &combined_proof[4..4 + gigaproof_size];
        let tip_bytes = &combined_proof[4 + gigaproof_size..];

        debug!("gigaproof: {} bytes, tip: {} bytes", gigaproof_bytes.len(), tip_bytes.len());

        // deserialize proofs
        let gigaproof: FinalizedLigeritoProof<BinaryElem32, BinaryElem128> =
            bincode::deserialize(gigaproof_bytes)
                .map_err(|e| anyhow::anyhow!("failed to deserialize gigaproof: {}", e))?;

        let tip_proof: FinalizedLigeritoProof<BinaryElem32, BinaryElem128> =
            bincode::deserialize(tip_bytes)
                .map_err(|e| anyhow::anyhow!("failed to deserialize tip proof: {}", e))?;

        // verify gigaproof
        let gigaproof_start = Instant::now();
        let gigaproof_valid = verify(&self.gigaproof_config, &gigaproof)
            .map_err(|e| anyhow::anyhow!("gigaproof verification error: {}", e))?;
        debug!("gigaproof verification took {:?}", gigaproof_start.elapsed());

        // verify tip proof
        let tip_start = Instant::now();
        let tip_valid = verify(&self.tip_config, &tip_proof)
            .map_err(|e| anyhow::anyhow!("tip verification error: {}", e))?;
        debug!("tip verification took {:?}", tip_start.elapsed());

        let elapsed = start.elapsed();
        info!(
            "verification complete ({:?}): gigaproof={}, tip={}",
            elapsed, gigaproof_valid, tip_valid
        );

        Ok((gigaproof_valid, tip_valid))
    }

    /// verify just tip proof (for incremental sync)
    pub fn verify_tip(&self, tip_proof: &[u8]) -> Result<bool> {
        info!("verifying tip proof ({} bytes)", tip_proof.len());
        let start = Instant::now();

        // deserialize tip proof
        let proof: FinalizedLigeritoProof<BinaryElem32, BinaryElem128> =
            bincode::deserialize(tip_proof)
                .map_err(|e| anyhow::anyhow!("failed to deserialize tip proof: {}", e))?;

        // verify
        let valid = verify(&self.tip_config, &proof)
            .map_err(|e| anyhow::anyhow!("tip verification error: {}", e))?;

        let elapsed = start.elapsed();
        info!("tip verification took {:?}: valid={}", elapsed, valid);

        Ok(valid)
    }
}

impl Default for ProofVerifier {
    fn default() -> Self {
        Self::new()
    }
}
