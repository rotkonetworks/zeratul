//! Client-side proof generation example
//!
//! This example shows how a client generates an AccidentalComputer proof
//! for a transfer transaction and submits it to the blockchain.
//!
//! ## Usage
//!
//! ```bash
//! cargo run --example generate_proof
//! ```

use anyhow::Result;
use zeratul_circuit::{
    prove_with_accidental_computer, AccountData, AccidentalComputerConfig, TransferInstance,
};

fn main() -> Result<()> {
    println!("=== Zeratul Client: Proof Generation Example ===\n");

    // Step 1: Create account data (private witness)
    println!("Step 1: Creating account data...");

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

    println!("  Sender:   ID={}, Balance={}", sender.id, sender.balance);
    println!("  Receiver: ID={}, Balance={}", receiver.id, receiver.balance);

    // Step 2: Create transfer instance
    println!("\nStep 2: Creating transfer instance...");
    let amount = 100;
    let sender_salt_new = random_salt();
    let receiver_salt_new = random_salt();

    let instance = TransferInstance::new(
        sender.clone(),
        sender_salt_new,
        receiver.clone(),
        receiver_salt_new,
        amount,
    )?;

    println!("  Transfer amount: {}", amount);
    println!("  Sender commitment (old):  {:?}", hex_bytes(&instance.sender_commitment_old));
    println!("  Sender commitment (new):  {:?}", hex_bytes(&instance.sender_commitment_new));
    println!("  Receiver commitment (old): {:?}", hex_bytes(&instance.receiver_commitment_old));
    println!("  Receiver commitment (new): {:?}", hex_bytes(&instance.receiver_commitment_new));

    // Step 3: Generate AccidentalComputer proof
    println!("\nStep 3: Generating AccidentalComputer proof...");
    println!("  (This uses ZODA encoding as polynomial commitment)");

    let config = AccidentalComputerConfig::default();
    let proof = prove_with_accidental_computer(&config, &instance)?;

    println!("  ✓ Proof generated!");
    println!("  ZODA commitment: {} bytes", proof.zoda_commitment.len());
    println!("  Shard count: {}", proof.shards.len());
    println!("  Total proof size: {} bytes", estimate_proof_size(&proof));

    // Step 4: Serialize proof for transmission
    println!("\nStep 4: Serializing proof...");
    let serialized = serde_json::to_vec(&proof)?;
    println!("  Serialized size: {} bytes", serialized.len());

    // Step 5: Show what would happen next
    println!("\n=== Next Steps (Not Implemented in Example) ===");
    println!("1. Connect to validator node");
    println!("2. Submit proof via: application_mailbox.submit_proof(proof)");
    println!("3. Validator adds proof to mempool");
    println!("4. Proof included in next block");
    println!("5. Full nodes verify via verify_accidental_computer()");
    println!("6. NOMT state updated with new commitments");

    println!("\n✓ Proof generation complete!");

    Ok(())
}

/// Generate a random salt (for demonstration purposes)
fn random_salt() -> [u8; 32] {
    let mut salt = [0u8; 32];
    // In production, use: rand::thread_rng().fill(&mut salt);
    // For this example, use deterministic values
    for i in 0..32 {
        salt[i] = (i * 7 + 13) as u8;
    }
    salt
}

/// Estimate proof size (rough)
fn estimate_proof_size(proof: &zeratul_circuit::AccidentalComputerProof) -> usize {
    proof.zoda_commitment.len()
        + proof.shards.iter().map(|s| s.len()).sum::<usize>()
        + 32 * 4 // commitments
}

/// Format bytes as hex for display
fn hex_bytes(bytes: &[u8]) -> String {
    bytes.iter().take(8).map(|b| format!("{:02x}", b)).collect::<Vec<_>>().join("")
        + "..."
}
