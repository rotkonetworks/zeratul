//! ligerito proof verification with continuity checking
//!
//! wire format (combined proof):
//!   [giga_full_size: u32][giga_full][tip_full]
//! where each full proof is:
//!   [public_outputs_len: u32][public_outputs (bincode)][log_size: u8][ligerito_proof (bincode)]

use anyhow::Result;
use ligerito::{FinalizedLigeritoProof, verify_with_transcript, transcript::FiatShamir};
use ligerito_binary_fields::{BinaryElem32, BinaryElem128};
use serde::{Serialize, Deserialize};
use crate::verifier_config_for_log_size;

#[cfg(not(target_arch = "wasm32"))]
use std::thread;

/// public outputs embedded in each proof
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ProofPublicOutputs {
    pub start_height: u32,
    pub end_height: u32,
    pub start_hash: [u8; 32],
    pub start_prev_hash: [u8; 32],
    pub tip_hash: [u8; 32],
    pub tip_prev_hash: [u8; 32],
    pub cumulative_difficulty: u64,
    pub final_commitment: [u8; 32],
    pub final_state_commitment: [u8; 32],
    pub num_headers: u32,
}

/// result of proof verification
#[derive(Clone, Debug)]
pub struct VerifyResult {
    pub gigaproof_valid: bool,
    pub tip_valid: bool,
    pub continuous: bool,
    pub giga_outputs: ProofPublicOutputs,
    pub tip_outputs: Option<ProofPublicOutputs>,
}

/// split a full proof into (public_outputs, raw_proof_bytes)
fn split_full_proof(full: &[u8]) -> Result<(ProofPublicOutputs, Vec<u8>)> {
    if full.len() < 4 {
        anyhow::bail!("proof too short");
    }
    let public_len = u32::from_le_bytes([full[0], full[1], full[2], full[3]]) as usize;
    if full.len() < 4 + public_len + 1 {
        anyhow::bail!("proof truncated");
    }
    let outputs: ProofPublicOutputs = bincode::deserialize(&full[4..4 + public_len])
        .map_err(|e| anyhow::anyhow!("deserialize public outputs: {}", e))?;
    let raw = full[4 + public_len..].to_vec();
    Ok((outputs, raw))
}

/// deserialize raw proof: [log_size: u8][proof_bytes...]
fn deserialize_proof(bytes: &[u8]) -> Result<(FinalizedLigeritoProof<BinaryElem32, BinaryElem128>, u8)> {
    if bytes.is_empty() {
        anyhow::bail!("empty proof bytes");
    }
    let log_size = bytes[0];
    let proof = bincode::deserialize(&bytes[1..])
        .map_err(|e| anyhow::anyhow!("failed to deserialize proof: {}", e))?;
    Ok((proof, log_size))
}

/// verify a single raw proof (sha256 transcript to match prover)
fn verify_single(proof_bytes: &[u8]) -> Result<bool> {
    let (proof, log_size) = deserialize_proof(proof_bytes)?;
    let config = verifier_config_for_log_size(log_size as u32);
    let transcript = FiatShamir::new_sha256(0);
    verify_with_transcript(&config, &proof, transcript)
        .map_err(|e| anyhow::anyhow!("verification error: {}", e))
}

/// verify combined gigaproof + tip proof with continuity checking
///
/// format: [giga_full_size: u32][giga_full][tip_full]
/// each full proof: [public_outputs_len: u32][public_outputs][log_size: u8][proof]
///
/// checks:
/// 1. both proofs verify cryptographically
/// 2. tip_proof.start_prev_hash == gigaproof.tip_hash (chain continuity)
#[cfg(not(target_arch = "wasm32"))]
pub fn verify_proofs(combined_proof: &[u8]) -> Result<(bool, bool)> {
    let result = verify_proofs_full(combined_proof)?;
    Ok((result.gigaproof_valid, result.tip_valid && result.continuous))
}

/// full verification with detailed result
#[cfg(not(target_arch = "wasm32"))]
pub fn verify_proofs_full(combined_proof: &[u8]) -> Result<VerifyResult> {
    if combined_proof.len() < 4 {
        anyhow::bail!("proof too small");
    }

    let giga_full_size = u32::from_le_bytes([
        combined_proof[0], combined_proof[1],
        combined_proof[2], combined_proof[3],
    ]) as usize;

    if combined_proof.len() < 4 + giga_full_size {
        anyhow::bail!("invalid proof format");
    }

    let giga_full = &combined_proof[4..4 + giga_full_size];
    let tip_full = &combined_proof[4 + giga_full_size..];

    // parse public outputs from both proofs
    let (giga_outputs, giga_raw) = split_full_proof(giga_full)?;
    let (tip_outputs, tip_raw) = if !tip_full.is_empty() {
        let (o, r) = split_full_proof(tip_full)?;
        (Some(o), r)
    } else {
        (None, vec![])
    };

    // verify both proofs in parallel
    let giga_raw_clone = giga_raw;
    let tip_raw_clone = tip_raw;
    let giga_handle = thread::spawn(move || verify_single(&giga_raw_clone));
    let tip_handle = if !tip_raw_clone.is_empty() {
        Some(thread::spawn(move || verify_single(&tip_raw_clone)))
    } else {
        None
    };

    let gigaproof_valid = giga_handle.join()
        .map_err(|_| anyhow::anyhow!("gigaproof thread panicked"))??;
    let tip_valid = match tip_handle {
        Some(h) => h.join().map_err(|_| anyhow::anyhow!("tip thread panicked"))??,
        None => true,
    };

    // check continuity: tip starts where gigaproof ends
    let continuous = match &tip_outputs {
        Some(tip) => tip.start_prev_hash == giga_outputs.tip_hash,
        None => true, // no tip = gigaproof covers everything
    };

    Ok(VerifyResult {
        gigaproof_valid,
        tip_valid,
        continuous,
        giga_outputs,
        tip_outputs,
    })
}

/// wasm variant
#[cfg(target_arch = "wasm32")]
pub fn verify_proofs(combined_proof: &[u8]) -> Result<(bool, bool)> {
    let result = verify_proofs_full(combined_proof)?;
    Ok((result.gigaproof_valid, result.tip_valid && result.continuous))
}

#[cfg(target_arch = "wasm32")]
pub fn verify_proofs_full(combined_proof: &[u8]) -> Result<VerifyResult> {
    if combined_proof.len() < 4 {
        anyhow::bail!("proof too small");
    }

    let giga_full_size = u32::from_le_bytes([
        combined_proof[0], combined_proof[1],
        combined_proof[2], combined_proof[3],
    ]) as usize;

    if combined_proof.len() < 4 + giga_full_size {
        anyhow::bail!("invalid proof format");
    }

    let giga_full = &combined_proof[4..4 + giga_full_size];
    let tip_full = &combined_proof[4 + giga_full_size..];

    let (giga_outputs, giga_raw) = split_full_proof(giga_full)?;
    let (tip_outputs, tip_raw) = if !tip_full.is_empty() {
        let (o, r) = split_full_proof(tip_full)?;
        (Some(o), r)
    } else {
        (None, vec![])
    };

    let gigaproof_valid = verify_single(&giga_raw)?;
    let tip_valid = if !tip_raw.is_empty() {
        verify_single(&tip_raw)?
    } else {
        true
    };

    let continuous = match &tip_outputs {
        Some(tip) => tip.start_prev_hash == giga_outputs.tip_hash,
        None => true,
    };

    Ok(VerifyResult {
        gigaproof_valid,
        tip_valid,
        continuous,
        giga_outputs,
        tip_outputs,
    })
}

/// verify just tip proof (for incremental sync)
pub fn verify_tip(tip_proof: &[u8]) -> Result<bool> {
    verify_single(tip_proof)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_empty_proof_fails() {
        let result = verify_proofs(&[]);
        assert!(result.is_err());
    }

    #[test]
    fn test_too_small_proof_fails() {
        let result = verify_proofs(&[1, 2, 3]);
        assert!(result.is_err());
    }
}
