//! State Transition Circuit using Ligerito PCS
//!
//! This module implements a commitment-based state transition system:
//! - Commitments hide account balances (privacy)
//! - Zero-knowledge proofs prove valid state transitions
//! - Fast verification for stateless servers
//!
//! ## Architecture
//!
//! The system proves knowledge of a valid state transition:
//! - Old commitments exist in NOMT
//! - New commitments are correctly computed
//! - Transaction constraints are satisfied (balance checks, nonce updates)
//!
//! ## How It Works
//!
//! 1. **Client**: Generates proof of valid transfer (expensive, ~seconds)
//! 2. **Server**: Verifies proof (fast, ~milliseconds) and updates NOMT
//! 3. **Storage**: NOMT stores commitments (not plaintext balances)
//!
//! ## ZK Proof System (Ligerito-based)
//!
//! New modules provide actual zero-knowledge proving:
//! - `constraint`: binius64-style constraint system (AND/XOR/MUL constraints)
//! - `witness_poly`: witness encoding as multilinear polynomial
//! - `zkproof`: full prove/verify using ligerito pcs
//!
//! Unlike `accidental_computer` which leaks witness data in da shards,
//! `zkproof` ensures verifier only sees polynomial commitment + proofs.

use anyhow::Result;

pub mod prover;
pub mod verifier;
pub mod accidental_computer;
pub mod note;
pub mod note_trace;
pub mod note_state;
pub mod privacy;

// new zk proof modules
pub mod constraint;
pub mod witness_poly;
pub mod zkproof;
pub mod spend_circuit;
pub mod poseidon;
pub mod poker;

pub use prover::{prove_transfer, StateTransitionProof};
pub use verifier::{verify_transfer, verify_and_extract_commitments, VerifiedTransition};
pub use accidental_computer::{
    prove_with_accidental_computer,
    verify_accidental_computer,
    AccidentalComputerConfig,
    AccidentalComputerProof,
};
pub use note::{
    Note, NoteCommitment, Nullifier, NullifierKey, Value, AssetId,
    Address, Rseed, Spend, Output, Transaction, TransactionPublic,
    MerkleProof, Position,
};
pub use note_state::{
    NoteProof, StateUpdate, NoteStateConfig, StateReader,
    commitment_key, nullifier_key, generate_nomt_updates,
    verify_proof_against_state, validate_proof_config,
};
pub use note_trace::{
    TransactionTrace, TransactionProofPublic,
    generate_trace, trace_to_polynomial, verify_trace,
};
pub use privacy::{
    SpendCircuit, PrivacyParams, SpendProof,
    bytes_to_field, field_to_bytes,
};

// zk proof exports
pub use constraint::{
    WireId, Operand, ShiftOp, Constraint, CircuitBuilder, Circuit, Witness,
};
pub use witness_poly::{
    WitnessPolynomial, ConstraintPolynomial, LigeritoInstance,
};
pub use zkproof::{
    ZkProof, ZkVerifier,
};
#[cfg(feature = "prover")]
pub use zkproof::{ZkProver, prove_and_verify};

// spend circuit exports (uses zeratul constraint system for real zk proofs)
pub use spend_circuit::{
    SpendCircuit as NoteSpendCircuit, SpendWires as NoteSpendWires,
    OutputCircuit as NoteOutputCircuit, OutputWires as NoteOutputWires,
    BalanceCircuit as NoteBalanceCircuit,
};

// poker settlement exports
pub use poker::{
    CooperativeWithdrawal, PlayerSignature,
    ShowdownCommitment,
    WinnerCircuit, WinnerWires,
    PotWithdrawalCircuit, PotWithdrawalWires,
    CooperativeWithdrawalRequest, DisputeWithdrawalRequest,
};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

/// A commitment to account state
/// commitment = Hash(account_id || balance || nonce || salt)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AccountCommitment {
    pub commitment: [u8; 32],
    pub encrypted_data: Vec<u8>, // Encrypted (balance, nonce) for transparency
}

/// Account data (private witness)
#[derive(Debug, Clone)]
pub struct AccountData {
    pub id: u64,
    pub balance: u64,
    pub nonce: u64,
    pub salt: [u8; 32],
}

impl AccountData {
    /// Compute commitment to this account data
    pub fn commit(&self) -> [u8; 32] {
        let mut hasher = Sha256::new();
        hasher.update(self.id.to_le_bytes());
        hasher.update(self.balance.to_le_bytes());
        hasher.update(self.nonce.to_le_bytes());
        hasher.update(self.salt);
        hasher.finalize().into()
    }

    /// Create new account data after transfer (sender)
    pub fn after_send(&self, amount: u64, new_salt: [u8; 32]) -> Result<Self> {
        if self.balance < amount {
            anyhow::bail!("Insufficient balance");
        }
        Ok(Self {
            id: self.id,
            balance: self.balance - amount,
            nonce: self.nonce + 1,
            salt: new_salt,
        })
    }

    /// Create new account data after transfer (receiver)
    pub fn after_receive(&self, amount: u64, new_salt: [u8; 32]) -> Self {
        Self {
            id: self.id,
            balance: self.balance + amount,
            nonce: self.nonce,
            salt: new_salt,
        }
    }
}

/// Circuit parameters (compile-time configuration)
#[derive(Debug, Clone)]
pub struct TransferCircuitParams {
    /// Maximum number of accounts that can be involved
    pub max_accounts: usize,
}

/// Instance data for a specific transfer proof
#[derive(Debug, Clone)]
pub struct TransferInstance {
    /// Private witness: sender account data
    pub sender_old: AccountData,
    pub sender_salt_new: [u8; 32],

    /// Private witness: receiver account data
    pub receiver_old: AccountData,
    pub receiver_salt_new: [u8; 32],

    /// Private witness: transfer amount
    pub amount: u64,

    /// Public inputs: commitments that will be stored on-chain
    pub sender_commitment_old: [u8; 32],
    pub sender_commitment_new: [u8; 32],
    pub receiver_commitment_old: [u8; 32],
    pub receiver_commitment_new: [u8; 32],
}

impl TransferInstance {
    /// Create a new transfer instance and compute commitments
    pub fn new(
        sender_old: AccountData,
        sender_salt_new: [u8; 32],
        receiver_old: AccountData,
        receiver_salt_new: [u8; 32],
        amount: u64,
    ) -> Result<Self> {
        let sender_new = sender_old.after_send(amount, sender_salt_new)?;
        let receiver_new = receiver_old.after_receive(amount, receiver_salt_new);

        Ok(Self {
            sender_commitment_old: sender_old.commit(),
            sender_commitment_new: sender_new.commit(),
            receiver_commitment_old: receiver_old.commit(),
            receiver_commitment_new: receiver_new.commit(),
            sender_old,
            sender_salt_new,
            receiver_old,
            receiver_salt_new,
            amount,
        })
    }

    /// Get public inputs (what goes on-chain)
    pub fn public_inputs(&self) -> Vec<[u8; 32]> {
        vec![
            self.sender_commitment_old,
            self.sender_commitment_new,
            self.receiver_commitment_old,
            self.receiver_commitment_new,
        ]
    }
}

/// Placeholder for actual Binius circuit
///
/// In a real implementation, this would use:
/// ```ignore
/// use binius_frontend::CircuitBuilder;
/// use binius_circuits::sha256::Sha256;
///
/// pub struct TransferCircuit {
///     // Wires for witness values
///     sender_balance: Wire,
///     sender_nonce: Wire,
///     receiver_balance: Wire,
///     receiver_nonce: Wire,
///     amount: Wire,
///
///     // Gadgets
///     hash_gadget: Sha256,
/// }
///
/// impl ExampleCircuit for TransferCircuit {
///     type Params = TransferCircuitParams;
///     type Instance = TransferInstance;
///
///     fn build(params: Self::Params, builder: &mut CircuitBuilder) -> Result<Self> {
///         // Add witnesses for private data
///         let sender_balance = builder.add_witness();
///         let sender_nonce = builder.add_witness();
///         let receiver_balance = builder.add_witness();
///         let receiver_nonce = builder.add_witness();
///         let amount = builder.add_witness();
///
///         // Add public inputs/outputs
///         let sender_commitment_old = builder.add_inout();
///         let sender_commitment_new = builder.add_inout();
///         let receiver_commitment_old = builder.add_inout();
///         let receiver_commitment_new = builder.add_inout();
///
///         // Build SHA256 gadget for commitment verification
///         let hash_gadget = Sha256::new(builder, max_len_bytes, digest_wires, message_wires);
///
///         // Constraint 1: Verify old sender commitment
///         // commitment_old == Hash(id || balance || nonce || salt)
///         let sender_preimage = concat_wires(
///             builder,
///             &[sender_id, sender_balance, sender_nonce, sender_salt_old]
///         );
///         hash_gadget.populate_message(sender_preimage);
///         builder.assert_eq("sender_old_valid", hash_gadget.digest(), sender_commitment_old);
///
///         // Constraint 2: Check sufficient balance
///         let has_funds = builder.icmp_uge(sender_balance, amount);
///         builder.assert_true("sufficient_balance", has_funds);
///
///         // Constraint 3: Verify new sender commitment
///         let new_sender_balance = builder.isub(sender_balance, amount);
///         let new_sender_nonce = builder.iadd(sender_nonce, builder.add_constant_64(1));
///         let sender_new_preimage = concat_wires(
///             builder,
///             &[sender_id, new_sender_balance, new_sender_nonce, sender_salt_new]
///         );
///         builder.assert_eq("sender_new_valid", hash_new_preimage, sender_commitment_new);
///
///         // Constraint 4: Verify old receiver commitment
///         let receiver_preimage = concat_wires(
///             builder,
///             &[receiver_id, receiver_balance, receiver_nonce, receiver_salt_old]
///         );
///         builder.assert_eq("receiver_old_valid", hash_preimage, receiver_commitment_old);
///
///         // Constraint 5: Verify new receiver commitment
///         let new_receiver_balance = builder.iadd(receiver_balance, amount);
///         let receiver_new_preimage = concat_wires(
///             builder,
///             &[receiver_id, new_receiver_balance, receiver_nonce, receiver_salt_new]
///         );
///         builder.assert_eq("receiver_new_valid", hash_new_preimage, receiver_commitment_new);
///
///         Ok(Self {
///             sender_balance,
///             sender_nonce,
///             receiver_balance,
///             receiver_nonce,
///             amount,
///             hash_gadget,
///         })
///     }
///
///     fn populate_witness(&self, instance: Self::Instance, w: &mut WitnessFiller) -> Result<()> {
///         // Populate witness values from instance
///         w[self.sender_balance] = Word(instance.sender_old.balance);
///         w[self.sender_nonce] = Word(instance.sender_old.nonce);
///         w[self.receiver_balance] = Word(instance.receiver_old.balance);
///         w[self.receiver_nonce] = Word(instance.receiver_old.nonce);
///         w[self.amount] = Word(instance.amount);
///
///         // Populate hash gadget inputs/outputs
///         self.hash_gadget.populate_message(w, &sender_preimage_bytes);
///         self.hash_gadget.populate_digest(w, &instance.sender_commitment_old);
///
///         Ok(())
///     }
/// }
/// ```
pub struct TransferCircuit;

impl TransferCircuit {
    /// Build the circuit (placeholder)
    pub fn build(_params: TransferCircuitParams) -> Result<Self> {
        // TODO: Implement actual circuit building with Binius CircuitBuilder
        Ok(Self)
    }

    /// Populate witness (placeholder)
    pub fn populate_witness(&self, _instance: &TransferInstance) -> Result<()> {
        // TODO: Implement actual witness population with Binius WitnessFiller
        Ok(())
    }

    /// Verify the constraints (placeholder for testing)
    pub fn verify_constraints(&self, instance: &TransferInstance) -> Result<()> {
        // Manually verify the constraints for testing
        // (In real implementation, this is done by the circuit)

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
        let sender_new = instance
            .sender_old
            .after_send(instance.amount, instance.sender_salt_new)?;
        let sender_new_commitment = sender_new.commit();
        if sender_new_commitment != instance.sender_commitment_new {
            anyhow::bail!("Invalid sender new commitment");
        }

        let receiver_new = instance
            .receiver_old
            .after_receive(instance.amount, instance.receiver_salt_new);
        let receiver_new_commitment = receiver_new.commit();
        if receiver_new_commitment != instance.receiver_commitment_new {
            anyhow::bail!("Invalid receiver new commitment");
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn random_salt() -> [u8; 32] {
        let mut salt = [0u8; 32];
        // In real implementation, use proper RNG
        for i in 0..32 {
            salt[i] = (i * 7 + 13) as u8;
        }
        salt
    }

    #[test]
    fn test_account_commitment() {
        let account = AccountData {
            id: 1,
            balance: 1000,
            nonce: 0,
            salt: random_salt(),
        };

        let commitment = account.commit();
        assert_eq!(commitment.len(), 32);

        // Same data should produce same commitment
        let commitment2 = account.commit();
        assert_eq!(commitment, commitment2);

        // Different balance should produce different commitment
        let account2 = AccountData {
            balance: 999,
            ..account
        };
        let commitment3 = account2.commit();
        assert_ne!(commitment, commitment3);
    }

    #[test]
    fn test_transfer_instance() {
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

        let amount = 100;
        let sender_salt_new = random_salt();
        let receiver_salt_new = random_salt();

        let instance = TransferInstance::new(
            sender.clone(),
            sender_salt_new,
            receiver.clone(),
            receiver_salt_new,
            amount,
        )
        .unwrap();

        // Verify commitments are correct
        assert_eq!(instance.sender_commitment_old, sender.commit());
        assert_eq!(instance.receiver_commitment_old, receiver.commit());

        let sender_new = sender.after_send(amount, sender_salt_new).unwrap();
        assert_eq!(instance.sender_commitment_new, sender_new.commit());

        let receiver_new = receiver.after_receive(amount, receiver_salt_new);
        assert_eq!(instance.receiver_commitment_new, receiver_new.commit());
    }

    #[test]
    fn test_circuit_verify_constraints() {
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
        )
        .unwrap();

        let circuit = TransferCircuit::build(TransferCircuitParams { max_accounts: 2 }).unwrap();

        // Should pass validation
        circuit.verify_constraints(&instance).unwrap();
    }

    #[test]
    fn test_insufficient_balance() {
        let sender = AccountData {
            id: 1,
            balance: 50, // Not enough
            nonce: 0,
            salt: random_salt(),
        };

        let receiver = AccountData {
            id: 2,
            balance: 500,
            nonce: 0,
            salt: random_salt(),
        };

        // Should fail to create instance
        let result = TransferInstance::new(sender, random_salt(), receiver, random_salt(), 100);

        assert!(result.is_err());
    }
}
