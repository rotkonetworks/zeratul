//! ligerito prover integration
//!
//! Uses SHA256 transcript for browser WASM verification compatibility.
//! The transcript choice affects proof verification - verifier must use same transcript.
//!
//! PUBLIC OUTPUTS: The proof commits to specific values that the client can verify:
//! - tip_hash: final block hash (client checks against Zanchor checkpoint)
//! - tip_prev_hash: for chain continuity verification
//! - cumulative_difficulty: total chain work
//! - final_commitment: running hash chain result (proves internal consistency)
//! - final_state_commitment: state root chain (for NOMT verification)

use crate::error::{Result, ZidecarError};
use crate::header_chain::HeaderChainTrace;
use ligerito::{prove_with_transcript, ProverConfig, data_structures::FinalizedLigeritoProof};
use ligerito::transcript::FiatShamir;
use ligerito_binary_fields::{BinaryElem32, BinaryElem128, BinaryFieldElement};
use serde::{Serialize, Deserialize};
use tracing::{info, debug};
use std::time::Instant;

/// Public outputs that the proof commits to
/// These are values the verifier can check against external sources (like Zanchor)
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ProofPublicOutputs {
    /// Starting block height
    pub start_height: u32,
    /// Ending block height (tip)
    pub end_height: u32,
    /// Starting block hash (public input - known checkpoint)
    pub start_hash: [u8; 32],
    /// Previous hash of starting block - for chain continuity with previous proof
    /// This links to the previous proof's tip_hash
    pub start_prev_hash: [u8; 32],
    /// Ending block hash (tip) - verify against Zanchor checkpoint
    pub tip_hash: [u8; 32],
    /// Previous hash of tip block - for chain continuity
    pub tip_prev_hash: [u8; 32],
    /// Cumulative difficulty (total chain work)
    pub cumulative_difficulty: u64,
    /// Final running commitment (proves hash chain consistency)
    pub final_commitment: [u8; 32],
    /// Final state commitment (for NOMT/state root verification)
    pub final_state_commitment: [u8; 32],
    /// Number of headers in proof
    pub num_headers: u32,
}

/// header chain proof with public outputs
pub struct HeaderChainProof {
    /// serialized ligerito proof
    pub proof_bytes: Vec<u8>,
    /// public outputs the proof commits to
    pub public_outputs: ProofPublicOutputs,
    /// log2 of trace size (needed for verifier config)
    pub trace_log_size: u32,
}

impl HeaderChainProof {
    /// generate proof from trace (with explicit config)
    /// Uses SHA256 transcript for browser WASM verification
    pub fn prove(
        config: &ProverConfig<BinaryElem32, BinaryElem128>,
        trace: &HeaderChainTrace,
    ) -> Result<Self> {
        use crate::header_chain::FIELDS_PER_HEADER;

        info!(
            "generating ligerito proof for {} headers (SHA256 transcript)",
            trace.num_headers
        );

        let start = Instant::now();

        // Extract public outputs from trace BEFORE proving
        // These are the values the proof commits to
        let public_outputs = Self::extract_public_outputs(trace)?;

        info!(
            "public outputs: tip_hash={}, difficulty={}, num_headers={}",
            hex::encode(&public_outputs.tip_hash[..8]),
            public_outputs.cumulative_difficulty,
            public_outputs.num_headers
        );

        // use SHA256 transcript for browser WASM verification
        // (blake2b requires extra WASM feature, sha256 is always available)
        let transcript = FiatShamir::new_sha256(0);

        // prove with ligerito using SHA256 transcript
        let proof = prove_with_transcript(config, &trace.trace, transcript)
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
            public_outputs,
            trace_log_size,
        })
    }

    /// Extract public outputs from the trace
    /// These values are what the proof actually commits to
    fn extract_public_outputs(trace: &HeaderChainTrace) -> Result<ProofPublicOutputs> {
        use crate::header_chain::FIELDS_PER_HEADER;

        if trace.num_headers == 0 {
            return Err(ZidecarError::ProofGeneration("empty trace".into()));
        }

        // Extract first header's hash and prev_hash (start)
        let first_offset = 0;
        let mut start_hash = [0u8; 32];
        for j in 0..8 {
            let field_val = trace.trace[first_offset + 1 + j].poly().value();
            start_hash[j * 4..(j + 1) * 4].copy_from_slice(&field_val.to_le_bytes());
        }

        // Extract first header's prev_hash - links to previous proof's tip_hash
        let mut start_prev_hash = [0u8; 32];
        for j in 0..8 {
            let field_val = trace.trace[first_offset + 9 + j].poly().value();
            start_prev_hash[j * 4..(j + 1) * 4].copy_from_slice(&field_val.to_le_bytes());
        }

        // Extract last header's hash and prev_hash (tip)
        let last_offset = (trace.num_headers - 1) * FIELDS_PER_HEADER;

        let mut tip_hash = [0u8; 32];
        for j in 0..8 {
            let field_val = trace.trace[last_offset + 1 + j].poly().value();
            tip_hash[j * 4..(j + 1) * 4].copy_from_slice(&field_val.to_le_bytes());
        }

        let mut tip_prev_hash = [0u8; 32];
        for j in 0..8 {
            let field_val = trace.trace[last_offset + 9 + j].poly().value();
            tip_prev_hash[j * 4..(j + 1) * 4].copy_from_slice(&field_val.to_le_bytes());
        }

        Ok(ProofPublicOutputs {
            start_height: trace.start_height,
            end_height: trace.end_height,
            start_hash,
            start_prev_hash,
            tip_hash,
            tip_prev_hash,
            cumulative_difficulty: trace.cumulative_difficulty,
            final_commitment: trace.final_commitment,
            final_state_commitment: trace.final_state_commitment,
            num_headers: trace.num_headers as u32,
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

    /// Serialize the full proof with public outputs
    /// Format: [public_outputs_len: u32][public_outputs...][log_size: u8][ligerito_proof...]
    pub fn serialize_full(&self) -> Result<Vec<u8>> {
        let public_bytes = bincode::serialize(&self.public_outputs)
            .map_err(|e| ZidecarError::Serialization(format!("bincode public outputs: {}", e)))?;

        let mut result = Vec::with_capacity(4 + public_bytes.len() + self.proof_bytes.len());
        result.extend_from_slice(&(public_bytes.len() as u32).to_le_bytes());
        result.extend(public_bytes);
        result.extend(&self.proof_bytes);
        Ok(result)
    }

    /// Deserialize full proof with public outputs
    pub fn deserialize_full(bytes: &[u8]) -> Result<(ProofPublicOutputs, Vec<u8>, u8)> {
        if bytes.len() < 5 {
            return Err(ZidecarError::Serialization("proof too short".into()));
        }

        let public_len = u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]) as usize;
        if bytes.len() < 4 + public_len + 1 {
            return Err(ZidecarError::Serialization("proof truncated".into()));
        }

        let public_outputs: ProofPublicOutputs = bincode::deserialize(&bytes[4..4 + public_len])
            .map_err(|e| ZidecarError::Serialization(format!("bincode public outputs: {}", e)))?;

        let proof_bytes = bytes[4 + public_len..].to_vec();
        let log_size = if !proof_bytes.is_empty() { proof_bytes[0] } else { 0 };

        Ok((public_outputs, proof_bytes, log_size))
    }

    /// serialize proof to bytes with config size prefix (internal)
    /// format: [log_size: u8][proof_bytes...]
    fn serialize_proof_with_config(proof: &FinalizedLigeritoProof<BinaryElem32, BinaryElem128>, log_size: u8) -> Result<Vec<u8>> {
        let proof_bytes = bincode::serialize(proof)
            .map_err(|e| ZidecarError::Serialization(format!("bincode serialize failed: {}", e)))?;

        let mut result = Vec::with_capacity(1 + proof_bytes.len());
        result.push(log_size);
        result.extend(proof_bytes);
        Ok(result)
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
