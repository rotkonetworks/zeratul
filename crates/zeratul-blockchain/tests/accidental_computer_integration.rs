//! Integration tests for AccidentalComputer proof generation and verification
//!
//! Tests the complete flow:
//! 1. Client generates proof
//! 2. Proof is verified
//! 3. State is updated

use anyhow::Result;
use zeratul_circuit::{
    prove_with_accidental_computer, verify_accidental_computer, AccountData,
    AccidentalComputerConfig, TransferInstance,
};

/// Helper to create deterministic salt
fn test_salt(seed: u8) -> [u8; 32] {
    let mut salt = [0u8; 32];
    for i in 0..32 {
        salt[i] = (i as u8).wrapping_mul(seed).wrapping_add(13);
    }
    salt
}

#[test]
fn test_generate_and_verify_proof() -> Result<()> {
    // Create sender and receiver accounts
    let sender = AccountData {
        id: 1,
        balance: 1000,
        nonce: 0,
        salt: test_salt(1),
    };

    let receiver = AccountData {
        id: 2,
        balance: 500,
        nonce: 0,
        salt: test_salt(2),
    };

    // Create transfer instance
    let instance = TransferInstance::new(
        sender.clone(),
        test_salt(3),
        receiver.clone(),
        test_salt(4),
        100, // amount
    )?;

    // Generate proof using AccidentalComputer
    let config = AccidentalComputerConfig::default();
    let proof = prove_with_accidental_computer(&config, &instance)?;

    // Verify proof commitments match
    assert_eq!(proof.sender_commitment_old, instance.sender_commitment_old);
    assert_eq!(proof.sender_commitment_new, instance.sender_commitment_new);
    assert_eq!(proof.receiver_commitment_old, instance.receiver_commitment_old);
    assert_eq!(proof.receiver_commitment_new, instance.receiver_commitment_new);

    // Verify proof using AccidentalComputer
    let valid = verify_accidental_computer(&config, &proof)?;
    assert!(valid, "Proof should be valid");

    // Verify ZODA commitment exists
    assert!(!proof.zoda_commitment.is_empty());
    assert!(!proof.shards.is_empty());

    Ok(())
}

#[test]
fn test_invalid_proof_fails() -> Result<()> {
    // Create a valid proof first
    let sender = AccountData {
        id: 1,
        balance: 1000,
        nonce: 0,
        salt: test_salt(1),
    };

    let receiver = AccountData {
        id: 2,
        balance: 500,
        nonce: 0,
        salt: test_salt(2),
    };

    let instance = TransferInstance::new(
        sender,
        test_salt(3),
        receiver,
        test_salt(4),
        100,
    )?;

    let config = AccidentalComputerConfig::default();
    let mut proof = prove_with_accidental_computer(&config, &instance)?;

    // Tamper with the proof (corrupt a commitment)
    proof.sender_commitment_new[0] ^= 0xFF;

    // Verification should fail
    let valid = verify_accidental_computer(&config, &proof)?;
    assert!(!valid, "Tampered proof should be invalid");

    Ok(())
}

#[test]
fn test_insufficient_balance_fails() {
    let sender = AccountData {
        id: 1,
        balance: 50, // Not enough for 100 transfer
        nonce: 0,
        salt: test_salt(1),
    };

    let receiver = AccountData {
        id: 2,
        balance: 500,
        nonce: 0,
        salt: test_salt(2),
    };

    // Should fail to create instance
    let result = TransferInstance::new(
        sender,
        test_salt(3),
        receiver,
        test_salt(4),
        100, // amount > balance
    );

    assert!(result.is_err(), "Should fail with insufficient balance");
}

#[test]
fn test_commitments_hide_balances() -> Result<()> {
    // Create two accounts with different balances
    let account1 = AccountData {
        id: 1,
        balance: 1000,
        nonce: 0,
        salt: test_salt(1),
    };

    let account2 = AccountData {
        id: 1, // Same ID
        balance: 2000, // Different balance
        nonce: 0,
        salt: test_salt(1), // Same salt
    };

    // Commitments should be different (balance is part of commitment)
    let commitment1 = account1.commit();
    let commitment2 = account2.commit();

    assert_ne!(
        commitment1, commitment2,
        "Different balances should produce different commitments"
    );

    // But you can't reverse engineer the balance from the commitment
    // (This property is ensured by the hash function)

    Ok(())
}

#[test]
fn test_multiple_transfers_same_accounts() -> Result<()> {
    // Initial state
    let mut sender = AccountData {
        id: 1,
        balance: 1000,
        nonce: 0,
        salt: test_salt(1),
    };

    let mut receiver = AccountData {
        id: 2,
        balance: 500,
        nonce: 0,
        salt: test_salt(2),
    };

    let config = AccidentalComputerConfig::default();

    // First transfer: 100
    let instance1 = TransferInstance::new(
        sender.clone(),
        test_salt(3),
        receiver.clone(),
        test_salt(4),
        100,
    )?;

    let proof1 = prove_with_accidental_computer(&config, &instance1)?;
    assert!(verify_accidental_computer(&config, &proof1)?);

    // Update sender and receiver for next transfer
    sender = sender.after_send(100, test_salt(3))?;
    receiver = receiver.after_receive(100, test_salt(4));

    // Second transfer: 200
    let instance2 = TransferInstance::new(
        sender.clone(),
        test_salt(5),
        receiver.clone(),
        test_salt(6),
        200,
    )?;

    let proof2 = prove_with_accidental_computer(&config, &instance2)?;
    assert!(verify_accidental_computer(&config, &proof2)?);

    // Verify final balances via commitments
    let expected_sender_balance = 1000 - 100 - 200; // 700
    let expected_receiver_balance = 500 + 100 + 200; // 800

    let final_sender = AccountData {
        id: 1,
        balance: expected_sender_balance,
        nonce: 2, // Incremented twice
        salt: test_salt(5),
    };

    let final_receiver = AccountData {
        id: 2,
        balance: expected_receiver_balance,
        nonce: 0, // Receiver nonce doesn't change
        salt: test_salt(6),
    };

    assert_eq!(proof2.sender_commitment_new, final_sender.commit());
    assert_eq!(proof2.receiver_commitment_new, final_receiver.commit());

    Ok(())
}
