//! ligerito prover integration

use crate::error::{Result, ZidecarError};
use crate::header_chain::HeaderChainTrace;
use ligerito::{prove, ProverConfig, data_structures::FinalizedLigeritoProof};
use ligerito_binary_fields::{BinaryElem32, BinaryElem128, BinaryFieldElement};
use tracing::{info, debug};
use std::time::Instant;

/// header chain proof
pub struct HeaderChainProof {
    /// serialized ligerito proof
    pub proof_bytes: Vec<u8>,
    /// height range
    pub from_height: u32,
    pub to_height: u32,
    /// log2 of trace size (needed for verifier config)
    pub trace_log_size: u32,
}

impl HeaderChainProof {
    /// generate proof from trace (with explicit config)
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

        // serialize proof with config size prefix
        let trace_log_size = (trace.trace.len() as f64).log2().ceil() as u32;
        let proof_bytes = Self::serialize_proof_with_config(&proof, trace_log_size as u8)?;
        debug!("proof size: {} bytes (config 2^{})", proof_bytes.len(), trace_log_size);

        Ok(Self {
            proof_bytes,
            from_height: trace.start_height,
            to_height: trace.end_height,
            trace_log_size,
        })
    }

    /// generate proof from trace (auto-select config based on trace size)
    pub fn prove_auto(trace: &mut HeaderChainTrace) -> Result<Self> {
        // select config based on trace size
        let (config, required_size) = zync_core::prover_config_for_size(trace.trace.len());

        info!(
            "auto-selected config for {} elements -> {} (2^{})",
            trace.trace.len(),
            required_size,
            (required_size as f64).log2() as u32
        );

        // pad trace if needed
        if trace.trace.len() < required_size {
            info!("padding trace from {} to {} elements", trace.trace.len(), required_size);
            trace.trace.resize(required_size, BinaryElem32::zero());
        }

        Self::prove(&config, trace)
    }

    /// serialize proof to bytes with config size prefix
    /// format: [log_size: u8][proof_bytes...]
    fn serialize_proof_with_config(proof: &FinalizedLigeritoProof<BinaryElem32, BinaryElem128>, log_size: u8) -> Result<Vec<u8>> {
        let proof_bytes = bincode::serialize(proof)
            .map_err(|e| ZidecarError::Serialization(format!("bincode serialize failed: {}", e)))?;

        let mut result = Vec::with_capacity(1 + proof_bytes.len());
        result.push(log_size);
        result.extend(proof_bytes);
        Ok(result)
    }

    /// serialize proof to bytes using bincode (legacy, no config prefix)
    fn serialize_proof(proof: &FinalizedLigeritoProof<BinaryElem32, BinaryElem128>) -> Result<Vec<u8>> {
        bincode::serialize(proof)
            .map_err(|e| ZidecarError::Serialization(format!("bincode serialize failed: {}", e)))
    }

    /// deserialize proof from bytes (reads config prefix if present)
    /// returns (proof, log_size)
    pub fn deserialize_proof_with_config(bytes: &[u8]) -> Result<(FinalizedLigeritoProof<BinaryElem32, BinaryElem128>, u8)> {
        if bytes.is_empty() {
            return Err(ZidecarError::Serialization("empty proof bytes".into()));
        }
        let log_size = bytes[0];
        let proof = bincode::deserialize(&bytes[1..])
            .map_err(|e| ZidecarError::Serialization(format!("bincode deserialize failed: {}", e)))?;
        Ok((proof, log_size))
    }

    /// deserialize proof from bytes (legacy, no config prefix)
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
