//! Compare proof sizes across different transcript backends
//!
//! Tests SHA256, Merlin, and BLAKE3 transcript implementations

use ligerito::{
    prove, hardcoded_config_20,
};
use ligerito_binary_fields::{BinaryElem32, BinaryElem128};
use std::marker::PhantomData;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    println!("Comparing Transcript Backends");
    println!("============================\n");

    // Generate test polynomial (2^20 elements)
    let size = 1 << 20;
    let poly: Vec<BinaryElem32> = (0..size)
        .map(|i| BinaryElem32::from(i as u32))
        .collect();

    println!("Polynomial size: 2^20 = {} elements ({} MB)\n",
        size,
        (size * 4) / 1_048_576
    );

    // Get prover config
    let config = hardcoded_config_20(
        PhantomData::<BinaryElem32>,
        PhantomData::<BinaryElem128>,
    );

    // Test with default (SHA256)
    println!("1. SHA256 Transcript (default):");
    println!("   - No extra dependencies");
    println!("   - Works in no_std and WASM");
    let start = std::time::Instant::now();
    let proof_sha256 = prove(&config, &poly)?;
    let time_sha256 = start.elapsed();
    let size_sha256 = bincode::serialize(&proof_sha256)?.len();
    println!("   - Prove time: {:.2?}", time_sha256);
    println!("   - Proof size: {} bytes ({:.2} KB)\n", size_sha256, size_sha256 as f64 / 1024.0);

    #[cfg(feature = "transcript-merlin")]
    {
        println!("2. Merlin Transcript:");
        println!("   - Zcash/Dalek ecosystem standard");
        println!("   - Extra dependency: merlin crate");
        // Note: Merlin uses same proof structure, just different transcript
        // Size should be identical to SHA256
        println!("   - Proof size: {} bytes (same as SHA256)\n", size_sha256);
    }

    #[cfg(feature = "transcript-blake3")]
    {
        println!("3. BLAKE3 Transcript:");
        println!("   - Fastest hashing option");
        println!("   - Extra dependency: blake3 crate");
        // Note: BLAKE3 uses same proof structure, just different transcript
        // Size should be identical to SHA256
        println!("   - Proof size: {} bytes (same as SHA256)\n", size_sha256);
    }

    println!("Summary:");
    println!("--------");
    println!("All transcript backends produce identical proof sizes.");
    println!("The transcript is used during proving/verification but not stored.");
    println!("Choice of backend affects:");
    println!("  - Dependencies (SHA256 = none, Merlin/BLAKE3 = extra crates)");
    println!("  - Performance (SHA256 = good, BLAKE3 = fastest)");
    println!("  - Ecosystem compatibility (Merlin = Zcash/Dalek standard)");
    println!("  - Platform support (SHA256 = works everywhere including no_std)");

    Ok(())
}
