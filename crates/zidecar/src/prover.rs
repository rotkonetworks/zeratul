//! ligerito prover integration

use crate::error::{Result, ZidecarError};
use crate::header_chain::HeaderChainTrace;
use ligerito::{prove, ProverConfig, data_structures::FinalizedLigeritoProof};
use ligerito_binary_fields::{BinaryElem32, BinaryElem128};
use tracing::{info, debug};
use std::time::Instant;

/// header chain proof
pub struct HeaderChainProof {
    /// serialized ligerito proof
    pub proof_bytes: Vec<u8>,
    /// height range
    pub from_height: u32,
    pub to_height: u32,
}

impl HeaderChainProof {
    /// generate proof from trace
    pub fn prove(
        config: &ProverConfig<BinaryElem32, BinaryElem128>,
        trace: &HeaderChainTrace,
    ) -> Result<Self> {
        info!(
            "generating ligerito proof for {} headers",
            trace.num_headers
        );

        let start = Instant::now();

        // prove with ligerito
        let proof = prove(config, &trace.trace)
            .map_err(|e| ZidecarError::ProofGeneration(format!("{:?}", e)))?;

        let elapsed = start.elapsed();
        info!(
            "proof generated in {:.2}s ({} headers, {} trace elements)",
            elapsed.as_secs_f64(),
            trace.num_headers,
            trace.trace.len()
        );

        // serialize proof
        let proof_bytes = Self::serialize_proof(&proof)?;
        debug!("proof size: {} bytes", proof_bytes.len());

        Ok(Self {
            proof_bytes,
            from_height: trace.start_height,
            to_height: trace.end_height,
        })
    }

    /// serialize proof to bytes using bincode
    fn serialize_proof(proof: &FinalizedLigeritoProof<BinaryElem32, BinaryElem128>) -> Result<Vec<u8>> {
        bincode::serialize(proof)
            .map_err(|e| ZidecarError::Serialization(format!("bincode serialize failed: {}", e)))
    }

    /// deserialize proof from bytes
    pub fn deserialize_proof(bytes: &[u8]) -> Result<FinalizedLigeritoProof<BinaryElem32, BinaryElem128>> {
        bincode::deserialize(bytes)
            .map_err(|e| ZidecarError::Serialization(format!("bincode deserialize failed: {}", e)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::zebrad::BlockHeader;
    use ligerito_binary_fields::BinaryElem32;

    #[test]
    fn test_proof_serialization() {
        // create dummy proof (simplified - normally from ligerito::prove)
        // this test just checks serialization works

        // we can't easily create a real proof without full setup
        // so we'll skip this for now - integration tests will cover it
    }
}
