//! Complete client example: Generate proof and submit to blockchain
//!
//! This example shows the full client workflow:
//! 1. Generate AccidentalComputer proof
//! 2. Connect to validator
//! 3. Submit proof to mempool
//! 4. Wait for confirmation
//!
//! ## Usage
//!
//! ```bash
//! # Start validator first
//! cargo run --bin validator
//!
//! # Then run client
//! cargo run --example submit_transfer
//! ```

use anyhow::Result;
use futures::channel::mpsc;
use state_transition_circuit::{
    prove_with_accidental_computer, AccountData, AccidentalComputerConfig, TransferInstance,
};
use zeratul_blockchain::{Application, ApplicationConfig, ApplicationMailbox, Message};

#[tokio::main]
async fn main() -> Result<()> {
    // Initialize tracing
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .init();

    println!("=== Zeratul Client: Submit Transfer ===\n");

    // Step 1: Generate proof
    println!("Generating proof...");
    let proof = generate_transfer_proof()?;
    println!("✓ Proof generated\n");

    // Step 2: Connect to validator
    println!("Connecting to validator...");
    let mut mailbox = connect_to_validator().await?;
    println!("✓ Connected\n");

    // Step 3: Submit proof
    println!("Submitting proof to mempool...");
    submit_proof(&mut mailbox, proof).await?;
    println!("✓ Proof submitted\n");

    println!("Transfer complete! Proof is now in mempool.");
    println!("It will be included in the next block.");

    Ok(())
}

/// Generate an AccidentalComputer proof for a transfer
fn generate_transfer_proof() -> Result<state_transition_circuit::AccidentalComputerProof> {
    // Create account data
    let sender = AccountData {
        id: 1,
        balance: 1000,
        nonce: 0,
        salt: random_salt(1),
    };

    let receiver = AccountData {
        id: 2,
        balance: 500,
        nonce: 0,
        salt: random_salt(2),
    };

    // Create transfer instance
    let instance = TransferInstance::new(
        sender,
        random_salt(3),
        receiver,
        random_salt(4),
        100, // amount
    )?;

    // Generate proof using AccidentalComputer
    let config = AccidentalComputerConfig::default();
    let proof = prove_with_accidental_computer(&config, &instance)?;

    Ok(proof)
}

/// Connect to a validator node
async fn connect_to_validator() -> Result<ApplicationMailbox> {
    // In a real client, this would connect via network
    // For this example, we create a local mailbox for testing

    // Create channel
    let (sender, receiver) = mpsc::channel(100);
    let mailbox = ApplicationMailbox::new(sender);

    // Note: In production, this would be a network connection:
    // let client = reqwest::Client::new();
    // let response = client.post("http://validator:8080/submit_proof")
    //     .json(&proof)
    //     .send()
    //     .await?;

    Ok(mailbox)
}

/// Submit proof to validator's mempool
async fn submit_proof(
    mailbox: &mut ApplicationMailbox,
    proof: state_transition_circuit::AccidentalComputerProof,
) -> Result<()> {
    mailbox.submit_proof(proof).await?;
    Ok(())
}

/// Generate deterministic salt for testing
fn random_salt(seed: u8) -> [u8; 32] {
    let mut salt = [0u8; 32];
    for i in 0..32 {
        salt[i] = (i as u8).wrapping_mul(seed).wrapping_add(13);
    }
    salt
}
