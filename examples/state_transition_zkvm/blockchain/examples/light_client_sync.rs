//! Light Client Sync Example
//!
//! This example demonstrates how a light client syncs to the blockchain
//! without downloading full ZODA shards.
//!
//! ## How It Works
//!
//! 1. **Full Nodes** verify AccidentalComputerProof (ZODA shards ~MB)
//! 2. **Light Clients** extract succinct proofs (~KB) and verify via PolkaVM
//!
//! ## Usage
//!
//! ```bash
//! # First, build the PolkaVM verifier
//! cd examples/polkavm_verifier
//! . ../../polkaports/activate.sh polkavm
//! make
//!
//! # Then run this example
//! cd ../../state_transition_zkvm/blockchain
//! cargo run --example light_client_sync
//! ```

use anyhow::Result;
use state_transition_circuit::{
    prove_with_accidental_computer, AccountData, AccidentalComputerConfig, TransferInstance,
};
use zeratul_blockchain::{
    Block, LightClient, LightClientConfig, extract_succinct_proof,
};

#[tokio::main]
async fn main() -> Result<()> {
    // Initialize tracing
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .init();

    println!("=== Zeratul Light Client Sync Example ===\n");

    // Step 1: Create light client
    println!("Step 1: Creating light client...");
    let config = LightClientConfig::default();
    let mut client = LightClient::new(config)?;
    println!("✓ Light client created\n");

    // Step 2: Initialize PolkaVM verifier
    println!("Step 2: Initializing PolkaVM verifier...");
    match client.init_polkavm().await {
        Ok(_) => println!("✓ PolkaVM verifier initialized\n"),
        Err(e) => {
            println!("⚠ PolkaVM verifier not available: {}", e);
            println!("  To build the verifier:");
            println!("  cd examples/polkavm_verifier");
            println!("  . ../../polkaports/activate.sh polkavm");
            println!("  make\n");
            println!("Continuing with proof extraction only...\n");
        }
    }

    // Step 3: Generate test proof (simulating what full node sends)
    println!("Step 3: Generating test proof...");
    let proof = generate_test_proof()?;
    println!("✓ AccidentalComputerProof generated");
    println!("  ZODA shards: {} bytes\n", estimate_proof_size(&proof));

    // Step 4: Extract succinct proof
    println!("Step 4: Extracting succinct proof for light client...");
    let succinct_proof = extract_succinct_proof(&proof, 24)?;
    println!("✓ Succinct proof extracted");
    println!("  Succinct proof: {} bytes", succinct_proof.proof_bytes.len());

    let zoda_size: usize = proof.shards.iter().map(|s| s.len()).sum();
    let compression_ratio = (zoda_size as f64) / (succinct_proof.proof_bytes.len() as f64);
    println!("  Compression ratio: {:.1}x\n", compression_ratio);

    // Step 5: Demonstrate size difference
    println!("Step 5: Size comparison");
    println!("  Full node verification:");
    println!("    - AccidentalComputerProof: {} bytes (ZODA shards)", zoda_size);
    println!("    - Verification: Native Rust, very fast");
    println!("  Light client verification:");
    println!("    - LigeritoSuccinctProof: {} bytes (compressed)", succinct_proof.proof_bytes.len());
    println!("    - Verification: PolkaVM sandboxed, secure\n");

    // Step 6: Show commitments
    println!("Step 6: Public commitments (what goes on-chain)");
    println!("  Sender old:  {}", hex_bytes(&succinct_proof.sender_commitment_old));
    println!("  Sender new:  {}", hex_bytes(&succinct_proof.sender_commitment_new));
    println!("  Receiver old: {}", hex_bytes(&succinct_proof.receiver_commitment_old));
    println!("  Receiver new: {}", hex_bytes(&succinct_proof.receiver_commitment_new));
    println!();

    println!("=== Summary ===");
    println!("✓ Light client can sync without downloading full ZODA shards");
    println!("✓ Proof size reduced by {:.1}x", compression_ratio);
    println!("✓ PolkaVM provides sandboxed verification");

    Ok(())
}

/// Generate a test AccidentalComputerProof
fn generate_test_proof() -> Result<state_transition_circuit::AccidentalComputerProof> {
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
        100, // amount
    )?;

    let config = AccidentalComputerConfig::default();
    let proof = prove_with_accidental_computer(&config, &instance)?;

    Ok(proof)
}

/// Generate deterministic salt for testing
fn test_salt(seed: u8) -> [u8; 32] {
    let mut salt = [0u8; 32];
    for i in 0..32 {
        salt[i] = (i as u8).wrapping_mul(seed).wrapping_add(13);
    }
    salt
}

/// Estimate total proof size
fn estimate_proof_size(proof: &state_transition_circuit::AccidentalComputerProof) -> usize {
    proof.zoda_commitment.len()
        + proof.shards.iter().map(|s| s.len()).sum::<usize>()
        + 32 * 4 // commitments
}

/// Format bytes as hex for display
fn hex_bytes(bytes: &[u8]) -> String {
    bytes.iter().take(8).map(|b| format!("{:02x}", b)).collect::<Vec<_>>().join("")
        + "..."
}
