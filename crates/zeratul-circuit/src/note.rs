//! Transparent Note Model
//!
//! Follows Penumbra's note structure but transparent (no privacy yet).
//! This allows building the infrastructure first, then adding privacy via Groth16.
//!
//! ## Design Rationale
//!
//! Penumbra was built transparent first and added encryption in later testnets.
//! We follow the same approach:
//! 1. Build with transparent notes (all values visible)
//! 2. Prove state transitions with Ligerito
//! 3. Add Groth16 for privacy (Phase 2)
//!
//! ## Note vs Account Model
//!
//! Notes are UTXO-like: you spend entire notes and create new ones.
//! This enables:
//! - Nullifiers (prevent double-spend without revealing which note)
//! - Better parallelization
//! - Easier privacy later (just encrypt note values)

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

/// Asset identifier (32 bytes)
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct AssetId(pub [u8; 32]);

impl AssetId {
    /// Native token asset ID
    pub fn native() -> Self {
        let mut hasher = Sha256::new();
        hasher.update(b"zeratul/native");
        Self(hasher.finalize().into())
    }
}

/// Value: amount + asset type
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Value {
    pub amount: u64,
    pub asset_id: AssetId,
}

impl Value {
    pub fn new(amount: u64, asset_id: AssetId) -> Self {
        Self { amount, asset_id }
    }

    pub fn native(amount: u64) -> Self {
        Self::new(amount, AssetId::native())
    }
}

/// Address that can receive notes
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Address {
    /// Diversifier for multiple addresses from same key
    pub diversifier: [u8; 16],
    /// Transmission key (public key for encryption)
    pub transmission_key: [u8; 32],
}

impl Address {
    /// Create address from raw bytes (simplified for transparent MVP)
    pub fn from_bytes(diversifier: [u8; 16], transmission_key: [u8; 32]) -> Self {
        Self {
            diversifier,
            transmission_key,
        }
    }

    /// Derive diversified generator (simplified)
    pub fn diversified_generator(&self) -> [u8; 32] {
        let mut hasher = Sha256::new();
        hasher.update(b"zeratul/diversified_generator");
        hasher.update(&self.diversifier);
        hasher.finalize().into()
    }
}

/// Random seed for deriving note blinding and ephemeral keys
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Rseed(pub [u8; 32]);

impl Rseed {
    /// Generate random rseed
    pub fn random() -> Self {
        use rand::RngCore;
        let mut bytes = [0u8; 32];
        rand::thread_rng().fill_bytes(&mut bytes);
        Self(bytes)
    }

    /// Derive note blinding factor
    pub fn derive_note_blinding(&self) -> [u8; 32] {
        let mut hasher = Sha256::new();
        hasher.update(b"zeratul/note_blinding");
        hasher.update(&self.0);
        hasher.finalize().into()
    }

    /// Derive ephemeral secret key (for encryption in Phase 2)
    pub fn derive_esk(&self) -> [u8; 32] {
        let mut hasher = Sha256::new();
        hasher.update(b"zeratul/esk");
        hasher.update(&self.0);
        hasher.finalize().into()
    }
}

/// A note representing value owned by an address
///
/// Transparent version - all fields visible.
/// In Phase 2, this will be encrypted for privacy.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Note {
    /// Value (amount + asset)
    pub value: Value,
    /// Random seed for blinding
    pub rseed: Rseed,
    /// Owner address
    pub address: Address,
}

impl Note {
    /// Create a new note
    pub fn new(value: Value, rseed: Rseed, address: Address) -> Self {
        Self {
            value,
            rseed,
            address,
        }
    }

    /// Compute note commitment
    ///
    /// commitment = Hash(blinding || amount || asset_id || diversified_gen || transmission_key)
    pub fn commit(&self) -> NoteCommitment {
        let blinding = self.rseed.derive_note_blinding();
        let div_gen = self.address.diversified_generator();

        let mut hasher = Sha256::new();
        hasher.update(b"zeratul/note_commitment");
        hasher.update(&blinding);
        hasher.update(&self.value.amount.to_le_bytes());
        hasher.update(&self.value.asset_id.0);
        hasher.update(&div_gen);
        hasher.update(&self.address.transmission_key);

        NoteCommitment(hasher.finalize().into())
    }

    /// Serialize note to bytes (160 bytes like Penumbra)
    pub fn to_bytes(&self) -> [u8; 160] {
        let mut bytes = [0u8; 160];

        // Address (48 bytes: 16 diversifier + 32 transmission_key)
        bytes[0..16].copy_from_slice(&self.address.diversifier);
        bytes[16..48].copy_from_slice(&self.address.transmission_key);

        // Value (40 bytes: 8 amount + 32 asset_id)
        bytes[48..56].copy_from_slice(&self.value.amount.to_le_bytes());
        bytes[56..88].copy_from_slice(&self.value.asset_id.0);

        // Rseed (32 bytes)
        bytes[88..120].copy_from_slice(&self.rseed.0);

        // Padding (40 bytes for future use)
        // bytes[120..160] = 0

        bytes
    }

    /// Deserialize note from bytes
    pub fn from_bytes(bytes: &[u8; 160]) -> Self {
        let mut diversifier = [0u8; 16];
        diversifier.copy_from_slice(&bytes[0..16]);

        let mut transmission_key = [0u8; 32];
        transmission_key.copy_from_slice(&bytes[16..48]);

        let amount = u64::from_le_bytes(bytes[48..56].try_into().unwrap());

        let mut asset_id = [0u8; 32];
        asset_id.copy_from_slice(&bytes[56..88]);

        let mut rseed = [0u8; 32];
        rseed.copy_from_slice(&bytes[88..120]);

        Self {
            value: Value::new(amount, AssetId(asset_id)),
            rseed: Rseed(rseed),
            address: Address::from_bytes(diversifier, transmission_key),
        }
    }
}

/// Commitment to a note (hides note contents)
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct NoteCommitment(pub [u8; 32]);

/// Nullifier key (derived from spending key)
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct NullifierKey(pub [u8; 32]);

impl NullifierKey {
    /// Derive nullifier key from spending key
    pub fn derive(spending_key: &[u8; 32]) -> Self {
        let mut hasher = Sha256::new();
        hasher.update(b"zeratul/nullifier_key");
        hasher.update(spending_key);
        Self(hasher.finalize().into())
    }
}

/// Nullifier: reveals that a note was spent without revealing which note
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Nullifier(pub [u8; 32]);

impl Nullifier {
    /// Derive nullifier from note
    ///
    /// nullifier = Hash(nk || position || note_commitment)
    pub fn derive(nk: &NullifierKey, position: u64, commitment: &NoteCommitment) -> Self {
        let mut hasher = Sha256::new();
        hasher.update(b"zeratul/nullifier");
        hasher.update(&nk.0);
        hasher.update(&position.to_le_bytes());
        hasher.update(&commitment.0);
        Self(hasher.finalize().into())
    }
}

/// Position in the note commitment tree
pub type Position = u64;

/// Merkle proof for note inclusion
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MerkleProof {
    pub position: Position,
    pub path: Vec<[u8; 32]>,
}

/// Spend action: consume a note
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Spend {
    /// Nullifier (public - prevents double spend)
    pub nullifier: Nullifier,
    /// Merkle root at time of spend
    pub anchor: [u8; 32],
    /// Balance commitment (for Phase 2 privacy)
    pub balance_commitment: [u8; 32],

    // Witness (private in Phase 2, transparent now)
    /// The note being spent
    pub note: Note,
    /// Nullifier key
    pub nk: NullifierKey,
    /// Merkle proof
    pub merkle_proof: MerkleProof,
}

/// Output action: create a note
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Output {
    /// Note commitment (public)
    pub note_commitment: NoteCommitment,
    /// Balance commitment (for Phase 2 privacy)
    pub balance_commitment: [u8; 32],

    // Witness (private in Phase 2, transparent now)
    /// The note being created
    pub note: Note,
}

/// A transaction consuming and creating notes
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Transaction {
    /// Notes being spent
    pub spends: Vec<Spend>,
    /// Notes being created
    pub outputs: Vec<Output>,
    /// Fee (transparent)
    pub fee: u64,
}

impl Transaction {
    /// Verify balance: sum(inputs) = sum(outputs) + fee
    pub fn verify_balance(&self) -> bool {
        let input_sum: u64 = self.spends.iter().map(|s| s.note.value.amount).sum();
        let output_sum: u64 = self.outputs.iter().map(|o| o.note.value.amount).sum();

        input_sum == output_sum + self.fee
    }

    /// Verify all nullifiers are correctly derived
    pub fn verify_nullifiers(&self) -> bool {
        for spend in &self.spends {
            let commitment = spend.note.commit();
            let expected = Nullifier::derive(
                &spend.nk,
                spend.merkle_proof.position,
                &commitment,
            );
            if expected != spend.nullifier {
                return false;
            }
        }
        true
    }

    /// Verify all output commitments are correct
    pub fn verify_output_commitments(&self) -> bool {
        for output in &self.outputs {
            let expected = output.note.commit();
            if expected != output.note_commitment {
                return false;
            }
        }
        true
    }

    /// Get all public data for on-chain storage
    pub fn public_data(&self) -> TransactionPublic {
        TransactionPublic {
            nullifiers: self.spends.iter().map(|s| s.nullifier).collect(),
            commitments: self.outputs.iter().map(|o| o.note_commitment).collect(),
            fee: self.fee,
        }
    }
}

/// Public transaction data (what goes on-chain)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TransactionPublic {
    pub nullifiers: Vec<Nullifier>,
    pub commitments: Vec<NoteCommitment>,
    pub fee: u64,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_address() -> Address {
        Address::from_bytes([1u8; 16], [2u8; 32])
    }

    #[test]
    fn test_note_commitment() {
        let note = Note::new(
            Value::native(1000),
            Rseed::random(),
            test_address(),
        );

        let commitment = note.commit();

        // Same note should produce same commitment
        let commitment2 = note.commit();
        assert_eq!(commitment, commitment2);

        // Different amount should produce different commitment
        let note2 = Note::new(
            Value::native(999),
            note.rseed,
            note.address.clone(),
        );
        let commitment3 = note2.commit();
        assert_ne!(commitment, commitment3);
    }

    #[test]
    fn test_nullifier_derivation() {
        let spending_key = [42u8; 32];
        let nk = NullifierKey::derive(&spending_key);

        let note = Note::new(
            Value::native(1000),
            Rseed::random(),
            test_address(),
        );
        let commitment = note.commit();
        let position = 123u64;

        let nullifier = Nullifier::derive(&nk, position, &commitment);

        // Same inputs should produce same nullifier
        let nullifier2 = Nullifier::derive(&nk, position, &commitment);
        assert_eq!(nullifier, nullifier2);

        // Different position should produce different nullifier
        let nullifier3 = Nullifier::derive(&nk, 124, &commitment);
        assert_ne!(nullifier, nullifier3);
    }

    #[test]
    fn test_note_serialization() {
        let note = Note::new(
            Value::native(1000),
            Rseed::random(),
            test_address(),
        );

        let bytes = note.to_bytes();
        let note2 = Note::from_bytes(&bytes);

        assert_eq!(note, note2);
    }

    #[test]
    fn test_transaction_balance() {
        let spending_key = [42u8; 32];
        let nk = NullifierKey::derive(&spending_key);

        // Input note: 1000
        let input_note = Note::new(
            Value::native(1000),
            Rseed::random(),
            test_address(),
        );
        let input_commitment = input_note.commit();

        // Output notes: 700 + 290 = 990 (+ 10 fee = 1000)
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

        let tx = Transaction {
            spends: vec![Spend {
                nullifier: Nullifier::derive(&nk, 0, &input_commitment),
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
        };

        assert!(tx.verify_balance());
        assert!(tx.verify_nullifiers());
        assert!(tx.verify_output_commitments());
    }
}
