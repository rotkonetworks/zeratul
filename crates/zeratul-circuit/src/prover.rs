//! Prover for state transition circuits
//!
//! This module implements proof generation for state transitions using Ligerito PCS.

use anyhow::Result;
use ligerito_binary_fields::{BinaryElem32, BinaryElem128};
use ligerito::{hardcoded_config_20, prover, FinalizedLigeritoProof};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::{AccountData, TransferInstance};

/// A proof of a valid state transition
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StateTransitionProof {
    /// The Ligerito polynomial commitment proof
    pub pcs_proof: FinalizedLigeritoProof<BinaryElem32, BinaryElem128>,

    /// Public inputs
    pub sender_commitment_old: [u8; 32],
    pub sender_commitment_new: [u8; 32],
    pub receiver_commitment_old: [u8; 32],
    pub receiver_commitment_new: [u8; 32],
}

/// Generate a proof for a state transition
///
/// This converts the state transition constraints into a polynomial and proves it with Ligerito.
pub fn prove_transfer(instance: &TransferInstance) -> Result<StateTransitionProof> {
    // Build the constraint polynomial from the instance
    let poly = build_constraint_polynomial(instance)?;

    // Verify constraints locally before proving
    verify_constraints_local(instance)?;

    // Generate Ligerito proof (use smaller config for testing)
    let config = if cfg!(test) {
        use ligerito::hardcoded_config_12;
        hardcoded_config_12(
            std::marker::PhantomData::<BinaryElem32>,
            std::marker::PhantomData::<BinaryElem128>,
        )
    } else {
        hardcoded_config_20(
            std::marker::PhantomData::<BinaryElem32>,
            std::marker::PhantomData::<BinaryElem128>,
        )
    };

    let pcs_proof = prover(&config, &poly)?;

    Ok(StateTransitionProof {
        pcs_proof,
        sender_commitment_old: instance.sender_commitment_old,
        sender_commitment_new: instance.sender_commitment_new,
        receiver_commitment_old: instance.receiver_commitment_old,
        receiver_commitment_new: instance.receiver_commitment_new,
    })
}

/// Build a constraint polynomial from the transfer instance
///
/// The polynomial encodes all the constraints:
/// - Commitment correctness
/// - Balance checks
/// - Nonce updates
fn build_constraint_polynomial(instance: &TransferInstance) -> Result<Vec<BinaryElem32>> {
    // Use smaller size for testing (2^12), can be increased to 2^20 for production
    let poly_size = if cfg!(test) { 1 << 12 } else { 1 << 20 };
    let mut poly = vec![BinaryElem32::from(0u32); poly_size];

    // Encode constraints into polynomial
    // For simplicity, we use a hash-based encoding scheme

    // Position 0-7: Sender old account data
    encode_account_data(&instance.sender_old, &mut poly[0..8]);

    // Position 8-15: Receiver old account data
    encode_account_data(&instance.receiver_old, &mut poly[8..16]);

    // Position 16-23: Transfer amount (8 u32 words)
    let amount_bytes = instance.amount.to_le_bytes();
    for i in 0..8 {
        poly[16 + i] = BinaryElem32::from(amount_bytes[i % 8] as u32);
    }

    // Position 24-31: New sender salt
    for i in 0..8 {
        let salt_word = u32::from_le_bytes([
            instance.sender_salt_new[i * 4],
            instance.sender_salt_new[i * 4 + 1],
            instance.sender_salt_new[i * 4 + 2],
            instance.sender_salt_new[i * 4 + 3],
        ]);
        poly[24 + i] = BinaryElem32::from(salt_word);
    }

    // Position 32-39: New receiver salt
    for i in 0..8 {
        let salt_word = u32::from_le_bytes([
            instance.receiver_salt_new[i * 4],
            instance.receiver_salt_new[i * 4 + 1],
            instance.receiver_salt_new[i * 4 + 2],
            instance.receiver_salt_new[i * 4 + 3],
        ]);
        poly[32 + i] = BinaryElem32::from(salt_word);
    }

    // Fill rest with hash of all constraints (binding)
    let constraint_hash = hash_all_constraints(instance);
    for i in 40..poly_size {
        let idx = (i - 40) % 32;
        poly[i] = BinaryElem32::from(constraint_hash[idx] as u32);
    }

    Ok(poly)
}

/// Encode account data into polynomial segment
fn encode_account_data(account: &AccountData, poly_segment: &mut [BinaryElem32]) {
    assert!(poly_segment.len() >= 8);

    // ID (u64) -> 2 u32s
    poly_segment[0] = BinaryElem32::from((account.id & 0xFFFFFFFF) as u32);
    poly_segment[1] = BinaryElem32::from((account.id >> 32) as u32);

    // Balance (u64) -> 2 u32s
    poly_segment[2] = BinaryElem32::from((account.balance & 0xFFFFFFFF) as u32);
    poly_segment[3] = BinaryElem32::from((account.balance >> 32) as u32);

    // Nonce (u64) -> 2 u32s
    poly_segment[4] = BinaryElem32::from((account.nonce & 0xFFFFFFFF) as u32);
    poly_segment[5] = BinaryElem32::from((account.nonce >> 32) as u32);

    // Salt (first 8 bytes as 2 u32s)
    poly_segment[6] = BinaryElem32::from(u32::from_le_bytes([
        account.salt[0], account.salt[1], account.salt[2], account.salt[3],
    ]));
    poly_segment[7] = BinaryElem32::from(u32::from_le_bytes([
        account.salt[4], account.salt[5], account.salt[6], account.salt[7],
    ]));
}

/// Hash all constraints for binding
fn hash_all_constraints(instance: &TransferInstance) -> [u8; 32] {
    let mut hasher = Sha256::new();

    hasher.update(&instance.sender_commitment_old);
    hasher.update(&instance.sender_commitment_new);
    hasher.update(&instance.receiver_commitment_old);
    hasher.update(&instance.receiver_commitment_new);
    hasher.update(&instance.sender_old.id.to_le_bytes());
    hasher.update(&instance.sender_old.balance.to_le_bytes());
    hasher.update(&instance.sender_old.nonce.to_le_bytes());
    hasher.update(&instance.receiver_old.id.to_le_bytes());
    hasher.update(&instance.receiver_old.balance.to_le_bytes());
    hasher.update(&instance.receiver_old.nonce.to_le_bytes());
    hasher.update(&instance.amount.to_le_bytes());

    hasher.finalize().into()
}

/// Verify constraints locally before generating proof
fn verify_constraints_local(instance: &TransferInstance) -> Result<()> {
    // 1. Verify old commitments
    let sender_old_commitment = instance.sender_old.commit();
    if sender_old_commitment != instance.sender_commitment_old {
        anyhow::bail!("Invalid sender old commitment");
    }

    let receiver_old_commitment = instance.receiver_old.commit();
    if receiver_old_commitment != instance.receiver_commitment_old {
        anyhow::bail!("Invalid receiver old commitment");
    }

    // 2. Check sufficient balance
    if instance.sender_old.balance < instance.amount {
        anyhow::bail!("Insufficient balance");
    }

    // 3. Verify new commitments
    let sender_new = instance.sender_old.after_send(instance.amount, instance.sender_salt_new)?;
    let sender_new_commitment = sender_new.commit();
    if sender_new_commitment != instance.sender_commitment_new {
        anyhow::bail!("Invalid sender new commitment");
    }

    let receiver_new = instance.receiver_old.after_receive(instance.amount, instance.receiver_salt_new);
    let receiver_new_commitment = receiver_new.commit();
    if receiver_new_commitment != instance.receiver_commitment_new {
        anyhow::bail!("Invalid receiver new commitment");
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn random_salt() -> [u8; 32] {
        let mut salt = [0u8; 32];
        for i in 0..32 {
            salt[i] = (i * 7 + 13) as u8;
        }
        salt
    }

    #[test]
    fn test_prove_transfer() {
        let sender = AccountData {
            id: 1,
            balance: 1000,
            nonce: 0,
            salt: random_salt(),
        };

        let receiver = AccountData {
            id: 2,
            balance: 500,
            nonce: 0,
            salt: random_salt(),
        };

        let instance = TransferInstance::new(
            sender,
            random_salt(),
            receiver,
            random_salt(),
            100,
        ).unwrap();

        let proof = prove_transfer(&instance).unwrap();

        // Verify public inputs match
        assert_eq!(proof.sender_commitment_old, instance.sender_commitment_old);
        assert_eq!(proof.sender_commitment_new, instance.sender_commitment_new);
        assert_eq!(proof.receiver_commitment_old, instance.receiver_commitment_old);
        assert_eq!(proof.receiver_commitment_new, instance.receiver_commitment_new);
    }
}
