//! ligerito proof verification
//!
//! supports both native (with rayon parallelism) and wasm targets

use anyhow::Result;
use ligerito::{FinalizedLigeritoProof, verify};
use ligerito_binary_fields::{BinaryElem32, BinaryElem128};
use crate::verifier_config_for_log_size;

#[cfg(not(target_arch = "wasm32"))]
use std::thread;

/// deserialize proof with config prefix
/// format: [log_size: u8][proof_bytes...]
fn deserialize_proof(bytes: &[u8]) -> Result<(FinalizedLigeritoProof<BinaryElem32, BinaryElem128>, u8)> {
    if bytes.is_empty() {
        anyhow::bail!("empty proof bytes");
    }
    let log_size = bytes[0];
    let proof = bincode::deserialize(&bytes[1..])
        .map_err(|e| anyhow::anyhow!("failed to deserialize proof: {}", e))?;
    Ok((proof, log_size))
}

/// verify a single proof
fn verify_single(proof_bytes: &[u8]) -> Result<bool> {
    let (proof, log_size) = deserialize_proof(proof_bytes)?;
    let config = verifier_config_for_log_size(log_size as u32);
    verify(&config, &proof).map_err(|e| anyhow::anyhow!("verification error: {}", e))
}

/// verify combined gigaproof + tip proof
/// format: [gigaproof_size: u32][gigaproof_bytes][tip_bytes]
/// returns (gigaproof_valid, tip_valid)
#[cfg(not(target_arch = "wasm32"))]
pub fn verify_proofs(combined_proof: &[u8]) -> Result<(bool, bool)> {
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

    let gigaproof_bytes = combined_proof[4..4 + gigaproof_size].to_vec();
    let tip_bytes = combined_proof[4 + gigaproof_size..].to_vec();

    // verify both proofs in parallel using native threads
    let giga_handle = thread::spawn(move || verify_single(&gigaproof_bytes));
    let tip_handle = thread::spawn(move || verify_single(&tip_bytes));

    let gigaproof_valid = giga_handle.join()
        .map_err(|_| anyhow::anyhow!("gigaproof thread panicked"))??;
    let tip_valid = tip_handle.join()
        .map_err(|_| anyhow::anyhow!("tip thread panicked"))??;

    Ok((gigaproof_valid, tip_valid))
}

/// verify combined gigaproof + tip proof (wasm - sequential)
#[cfg(target_arch = "wasm32")]
pub fn verify_proofs(combined_proof: &[u8]) -> Result<(bool, bool)> {
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

    // wasm: verify sequentially (web workers would need different setup)
    let gigaproof_valid = verify_single(gigaproof_bytes)?;
    let tip_valid = verify_single(tip_bytes)?;

    Ok((gigaproof_valid, tip_valid))
}

/// verify just tip proof (for incremental sync)
pub fn verify_tip(tip_proof: &[u8]) -> Result<bool> {
    verify_single(tip_proof)
}

/// verify multiple proofs in parallel (native only)
#[cfg(not(target_arch = "wasm32"))]
pub fn verify_batch(proofs: Vec<Vec<u8>>) -> Result<Vec<bool>> {
    use std::sync::mpsc;

    let (tx, rx) = mpsc::channel();
    let num_proofs = proofs.len();

    for (i, proof) in proofs.into_iter().enumerate() {
        let tx = tx.clone();
        thread::spawn(move || {
            let result = verify_single(&proof);
            let _ = tx.send((i, result));
        });
    }
    drop(tx);

    let mut results = vec![false; num_proofs];
    for _ in 0..num_proofs {
        let (i, result) = rx.recv()
            .map_err(|_| anyhow::anyhow!("channel error"))?;
        results[i] = result?;
    }

    Ok(results)
}

/// verify multiple proofs (wasm - sequential)
#[cfg(target_arch = "wasm32")]
pub fn verify_batch(proofs: Vec<Vec<u8>>) -> Result<Vec<bool>> {
    proofs.iter().map(|p| verify_single(p)).collect()
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

    #[test]
    fn test_invalid_format_fails() {
        // says gigaproof is 1000 bytes but we only have 10
        let mut data = vec![0xe8, 0x03, 0x00, 0x00]; // 1000 in little endian
        data.extend_from_slice(&[0u8; 10]);
        let result = verify_proofs(&data);
        assert!(result.is_err());
    }
}
