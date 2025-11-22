//! Note-based State Management
//!
//! Integrates the transparent note model with NOMT for authenticated state storage.
//! Handles nullifier tracking and note commitment management.
//!
//! ## State Layout in NOMT
//!
//! - Key: `commit/{commitment_hash}` → Value: empty (presence proves inclusion)
//! - Key: `nullifier/{nullifier_hash}` → Value: empty (presence proves spent)
//!
//! ## Merkle Proofs
//!
//! NOMT provides witness proofs for both inclusion and non-inclusion,
//! which we use for:
//! - Proving a note commitment exists (for spending)
//! - Proving a nullifier doesn't exist (not double-spent)

use crate::note::Transaction;
use crate::note_trace::{generate_trace, verify_trace, TransactionProofPublic};
use ligerito_binary_fields::BinaryElem128;
use serde::{Deserialize, Serialize};

/// Proof of valid note transaction
///
/// Contains the transaction data and Ligerito proof of correct execution.
/// In Phase 2, this will be wrapped with Groth16 for privacy.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NoteProof {
    /// Public inputs (nullifiers, commitments, fee)
    pub public: TransactionProofPublic,

    /// The full transaction (transparent - visible to verifier)
    /// In Phase 2, this becomes private witness
    pub transaction: Transaction,

    /// Batching challenge used for constraint accumulation
    pub batching_challenge: [u8; 16],
}

impl NoteProof {
    /// Create a new note proof from a transaction
    pub fn new(tx: Transaction, challenge: BinaryElem128) -> Result<Self, &'static str> {
        // Generate and verify trace
        let trace = generate_trace(&tx, challenge)?;
        if !verify_trace(&trace) {
            return Err("Transaction constraints not satisfied");
        }

        // Extract public inputs
        let public = TransactionProofPublic::from_transaction(&tx);

        // Convert challenge to bytes using bytemuck
        let challenge_bytes: [u8; 16] = bytemuck::bytes_of(&challenge).try_into()
            .expect("BinaryElem128 should be 16 bytes");

        Ok(Self {
            public,
            transaction: tx,
            batching_challenge: challenge_bytes,
        })
    }

    /// Verify the proof is internally consistent
    pub fn verify(&self) -> bool {
        // Reconstruct challenge from bytes
        let challenge: BinaryElem128 = *bytemuck::from_bytes(&self.batching_challenge);

        // Verify trace
        match generate_trace(&self.transaction, challenge) {
            Ok(trace) => verify_trace(&trace),
            Err(_) => false,
        }
    }

    /// Get all nullifiers from this proof
    pub fn nullifiers(&self) -> &[[u8; 32]] {
        &self.public.nullifiers
    }

    /// Get all new commitments from this proof
    pub fn commitments(&self) -> &[[u8; 32]] {
        &self.public.commitments
    }

    /// Get the fee
    pub fn fee(&self) -> u64 {
        self.public.fee
    }

    /// Get the anchor (state root at time of transaction)
    pub fn anchor(&self) -> [u8; 32] {
        self.public.anchor
    }
}

/// State update from a verified note proof
#[derive(Debug, Clone)]
pub struct StateUpdate {
    /// Nullifiers to mark as spent
    pub nullifiers: Vec<[u8; 32]>,
    /// New commitments to add
    pub commitments: Vec<[u8; 32]>,
    /// Fee collected
    pub fee: u64,
}

impl From<&NoteProof> for StateUpdate {
    fn from(proof: &NoteProof) -> Self {
        Self {
            nullifiers: proof.public.nullifiers.clone(),
            commitments: proof.public.commitments.clone(),
            fee: proof.public.fee,
        }
    }
}

/// Key prefix for note commitments in NOMT
pub const COMMITMENT_PREFIX: &[u8] = b"commit/";

/// Key prefix for nullifiers in NOMT
pub const NULLIFIER_PREFIX: &[u8] = b"nullifier/";

/// Create NOMT key for a commitment
pub fn commitment_key(commitment: &[u8; 32]) -> [u8; 32] {
    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    hasher.update(COMMITMENT_PREFIX);
    hasher.update(commitment);
    hasher.finalize().into()
}

/// Create NOMT key for a nullifier
pub fn nullifier_key(nullifier: &[u8; 32]) -> [u8; 32] {
    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    hasher.update(NULLIFIER_PREFIX);
    hasher.update(nullifier);
    hasher.finalize().into()
}

/// Generate NOMT updates from state update
///
/// Returns list of (key, value) pairs for NOMT commit
pub fn generate_nomt_updates(update: &StateUpdate) -> Vec<([u8; 32], Option<Vec<u8>>)> {
    let mut updates = Vec::new();

    // Add nullifiers (presence indicates spent)
    for nullifier in &update.nullifiers {
        let key = nullifier_key(nullifier);
        updates.push((key, Some(vec![1]))); // marker value
    }

    // Add commitments (presence indicates unspent note exists)
    for commitment in &update.commitments {
        let key = commitment_key(commitment);
        updates.push((key, Some(vec![1]))); // marker value
    }

    updates
}

/// Verify that a proof's inputs exist in state and outputs are new
///
/// Returns:
/// - Ok(true) if proof is valid against current state
/// - Ok(false) if proof is invalid (double-spend, missing input, etc.)
/// - Err if state query failed
pub fn verify_proof_against_state<S: StateReader>(
    proof: &NoteProof,
    state: &S,
) -> Result<bool, &'static str> {
    // Check that all input commitments exist (notes to spend exist)
    for spend in &proof.transaction.spends {
        let commitment = spend.note.commit();
        let key = commitment_key(&commitment.0);
        if !state.exists(&key)? {
            return Ok(false); // Input note doesn't exist
        }
    }

    // Check that all nullifiers are new (not double-spent)
    for nullifier in &proof.public.nullifiers {
        let key = nullifier_key(nullifier);
        if state.exists(&key)? {
            return Ok(false); // Already spent
        }
    }

    // Check that output commitments are new
    for commitment in &proof.public.commitments {
        let key = commitment_key(commitment);
        if state.exists(&key)? {
            return Ok(false); // Commitment collision (extremely unlikely)
        }
    }

    Ok(true)
}

/// Trait for reading state (abstraction over NOMT)
pub trait StateReader {
    /// Check if a key exists in state
    fn exists(&self, key: &[u8; 32]) -> Result<bool, &'static str>;

    /// Get value at key (None if not present)
    fn get(&self, key: &[u8; 32]) -> Result<Option<Vec<u8>>, &'static str>;

    /// Get current state root
    fn root(&self) -> [u8; 32];
}

/// Configuration for note state management
#[derive(Debug, Clone)]
pub struct NoteStateConfig {
    /// Maximum notes per transaction
    pub max_notes_per_tx: usize,
    /// Maximum transaction fee
    pub max_fee: u64,
    /// Minimum transaction fee
    pub min_fee: u64,
}

impl Default for NoteStateConfig {
    fn default() -> Self {
        Self {
            max_notes_per_tx: 16,
            max_fee: 1_000_000_000, // 1 billion units
            min_fee: 1,
        }
    }
}

/// Validate a note proof against configuration
pub fn validate_proof_config(proof: &NoteProof, config: &NoteStateConfig) -> Result<(), &'static str> {
    // Check note counts
    if proof.transaction.spends.len() > config.max_notes_per_tx {
        return Err("Too many input notes");
    }
    if proof.transaction.outputs.len() > config.max_notes_per_tx {
        return Err("Too many output notes");
    }

    // Check fee bounds
    if proof.public.fee < config.min_fee {
        return Err("Fee too low");
    }
    if proof.public.fee > config.max_fee {
        return Err("Fee too high");
    }

    Ok(())
}

/// Mock state reader for testing
#[cfg(test)]
pub struct MockStateReader {
    pub data: std::collections::HashMap<[u8; 32], Vec<u8>>,
    pub state_root: [u8; 32],
}

#[cfg(test)]
impl StateReader for MockStateReader {
    fn exists(&self, key: &[u8; 32]) -> Result<bool, &'static str> {
        Ok(self.data.contains_key(key))
    }

    fn get(&self, key: &[u8; 32]) -> Result<Option<Vec<u8>>, &'static str> {
        Ok(self.data.get(key).cloned())
    }

    fn root(&self) -> [u8; 32] {
        self.state_root
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::note::{Note, Value, Address, Rseed, Spend, Output, NullifierKey, MerkleProof};

    fn test_address() -> Address {
        Address::from_bytes([1u8; 16], [2u8; 32])
    }

    fn create_test_transaction() -> Transaction {
        let spending_key = [42u8; 32];
        let nk = NullifierKey::derive(&spending_key);

        let input_note = Note::new(
            Value::native(1000),
            Rseed::random(),
            test_address(),
        );
        let input_commitment = input_note.commit();

        let output1 = Note::new(
            Value::native(700),
            Rseed::random(),
            test_address(),
        );
        let output2 = Note::new(
            Value::native(290),
            Rseed::random(),
            test_address(),
        );

        Transaction {
            spends: vec![Spend {
                nullifier: crate::note::Nullifier::derive(&nk, 0, &input_commitment),
                anchor: [0u8; 32],
                balance_commitment: [0u8; 32],
                note: input_note,
                nk,
                merkle_proof: MerkleProof {
                    position: 0,
                    path: vec![],
                },
            }],
            outputs: vec![
                Output {
                    note_commitment: output1.commit(),
                    balance_commitment: [0u8; 32],
                    note: output1,
                },
                Output {
                    note_commitment: output2.commit(),
                    balance_commitment: [0u8; 32],
                    note: output2,
                },
            ],
            fee: 10,
        }
    }

    #[test]
    fn test_note_proof_creation() {
        let tx = create_test_transaction();
        let challenge = BinaryElem128::from(0xDEADBEEFCAFEBABEu128);

        let proof = NoteProof::new(tx, challenge).unwrap();

        assert_eq!(proof.nullifiers().len(), 1);
        assert_eq!(proof.commitments().len(), 2);
        assert_eq!(proof.fee(), 10);
        assert!(proof.verify());
    }

    #[test]
    fn test_state_update() {
        let tx = create_test_transaction();
        let challenge = BinaryElem128::from(0xDEADBEEFCAFEBABEu128);

        let proof = NoteProof::new(tx, challenge).unwrap();
        let update = StateUpdate::from(&proof);

        assert_eq!(update.nullifiers.len(), 1);
        assert_eq!(update.commitments.len(), 2);
        assert_eq!(update.fee, 10);
    }

    #[test]
    fn test_nomt_updates() {
        let tx = create_test_transaction();
        let challenge = BinaryElem128::from(0xDEADBEEFCAFEBABEu128);

        let proof = NoteProof::new(tx, challenge).unwrap();
        let update = StateUpdate::from(&proof);
        let nomt_updates = generate_nomt_updates(&update);

        // 1 nullifier + 2 commitments = 3 updates
        assert_eq!(nomt_updates.len(), 3);
    }

    #[test]
    fn test_verify_against_state() {
        let tx = create_test_transaction();
        let challenge = BinaryElem128::from(0xDEADBEEFCAFEBABEu128);

        let proof = NoteProof::new(tx.clone(), challenge).unwrap();

        // Create state with the input commitment
        let mut state = MockStateReader {
            data: std::collections::HashMap::new(),
            state_root: [0u8; 32],
        };

        // Add input commitment to state
        let input_commitment = tx.spends[0].note.commit();
        let input_key = commitment_key(&input_commitment.0);
        state.data.insert(input_key, vec![1]);

        // Should verify successfully
        assert!(verify_proof_against_state(&proof, &state).unwrap());
    }

    #[test]
    fn test_double_spend_detection() {
        let tx = create_test_transaction();
        let challenge = BinaryElem128::from(0xDEADBEEFCAFEBABEu128);

        let proof = NoteProof::new(tx.clone(), challenge).unwrap();

        // Create state with input commitment AND nullifier (already spent)
        let mut state = MockStateReader {
            data: std::collections::HashMap::new(),
            state_root: [0u8; 32],
        };

        // Add input commitment
        let input_commitment = tx.spends[0].note.commit();
        let input_key = commitment_key(&input_commitment.0);
        state.data.insert(input_key, vec![1]);

        // Add nullifier (simulate already spent)
        let nullifier_k = nullifier_key(&tx.spends[0].nullifier.0);
        state.data.insert(nullifier_k, vec![1]);

        // Should fail - double spend
        assert!(!verify_proof_against_state(&proof, &state).unwrap());
    }

    #[test]
    fn test_missing_input_detection() {
        let tx = create_test_transaction();
        let challenge = BinaryElem128::from(0xDEADBEEFCAFEBABEu128);

        let proof = NoteProof::new(tx, challenge).unwrap();

        // Empty state - input commitment doesn't exist
        let state = MockStateReader {
            data: std::collections::HashMap::new(),
            state_root: [0u8; 32],
        };

        // Should fail - input doesn't exist
        assert!(!verify_proof_against_state(&proof, &state).unwrap());
    }

    #[test]
    fn test_validate_config() {
        let tx = create_test_transaction();
        let challenge = BinaryElem128::from(0xDEADBEEFCAFEBABEu128);

        let proof = NoteProof::new(tx, challenge).unwrap();
        let config = NoteStateConfig::default();

        assert!(validate_proof_config(&proof, &config).is_ok());
    }
}
