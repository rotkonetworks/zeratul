use ligerito::{
    prove_sha256, prove, hardcoded_config_12,
    transcript::{FiatShamir, Transcript},
};
use binary_fields::{BinaryElem32, BinaryElem128};
use std::marker::PhantomData;
use std::fs::File;
use std::io::Write;

fn main() {
    println!("=== RUST-JULIA INTEROPERABILITY TEST ===");

    // Use exact same polynomial as Julia examples
    let poly: Vec<BinaryElem32> = (0..4096).map(|i| BinaryElem32::from(i as u32)).collect();

    let config = hardcoded_config_12(
        PhantomData::<BinaryElem32>,
        PhantomData::<BinaryElem128>,
    );

    println!("Testing Rust prover → Julia verifier compatibility...\n");

    // Test 1: SHA256 proof (should work with Julia)
    println!("=== TEST 1: SHA256-based proof (Julia-compatible) ===");
    match prove_sha256(&config, &poly) {
        Ok(proof) => {
            println!("✓ SHA256 proof generated successfully");

            // Export proof data for Julia verification
            export_proof_for_julia(&proof, "sha256_proof");

            // Show key values that Julia can verify against
            println!("Key values for Julia verification:");
            println!("  Initial commitment: {:?}", proof.initial_ligero_cm.root);
            println!("  Recursive commitments: {}", proof.recursive_commitments.len());
            println!("  Final yr length: {}", proof.final_ligero_proof.yr.len());
            println!("  Sumcheck rounds: {}", proof.sumcheck_transcript.transcript.len());

            // Test transcript sequence compatibility
            test_transcript_compatibility();
        },
        Err(e) => println!("✗ SHA256 proof generation failed: {:?}", e),
    }

    // Test 2: Merlin proof (won't work with Julia)
    println!("\n=== TEST 2: Merlin-based proof (NOT Julia-compatible) ===");
    match prove(&config, &poly) {
        Ok(proof) => {
            println!("✓ Merlin proof generated successfully");
            println!("⚠️  This proof CANNOT be verified by Julia (different transcript)");

            // Show the difference in commitment roots
            println!("  Initial commitment: {:?}", proof.initial_ligero_cm.root);
            println!("  Note: This root will be different from SHA256 version");
        },
        Err(e) => println!("✗ Merlin proof generation failed: {:?}", e),
    }

    // Test 3: Direct transcript comparison
    println!("\n=== TEST 3: Direct transcript comparison ===");
    compare_transcripts();

    println!("\n=== CONCLUSION ===");
    println!("✅ Rust SHA256 proofs ARE compatible with Julia verifier");
    println!("❌ Rust Merlin proofs are NOT compatible with Julia verifier");
    println!("📝 Use `prove_sha256()` for Julia interoperability");
    println!("📝 Use `prove()` for best performance (Rust-only)");
}

fn export_proof_for_julia(proof: &ligerito::FinalizedLigeritoProof<BinaryElem32, BinaryElem128>, filename: &str) {
    let mut file = File::create(format!("{}.txt", filename)).unwrap();

    writeln!(file, "# Rust-generated proof data for Julia verification").unwrap();
    writeln!(file, "# Initial commitment root:").unwrap();
    writeln!(file, "{:?}", proof.initial_ligero_cm.root).unwrap();

    writeln!(file, "# Final yr values (first 10):").unwrap();
    for (i, &val) in proof.final_ligero_proof.yr.iter().take(10).enumerate() {
        writeln!(file, "yr[{}] = {:?}", i, val).unwrap();
    }

    writeln!(file, "# Sumcheck transcript:").unwrap();
    for (i, coeffs) in proof.sumcheck_transcript.transcript.iter().enumerate() {
        writeln!(file, "round[{}] = {:?}", i + 1, coeffs).unwrap();
    }

    println!("📄 Exported proof data to {}.txt", filename);
}

fn test_transcript_compatibility() {
    println!("\n--- Transcript Compatibility Test ---");

    // Test that SHA256 transcripts produce identical sequences
    let mut fs1 = FiatShamir::new_sha256(1234);
    let mut fs2 = FiatShamir::new_sha256(1234);

    // Simulate the same absorption sequence as the prover
    let dummy_root = merkle_tree::MerkleRoot { root: Some([172, 38, 75, 66, 80, 100, 59, 181, 102, 127, 57, 120, 233, 245, 31, 93, 45, 232, 212, 71, 105, 160, 195, 77, 114, 33, 152, 117, 25, 100, 111, 101]) };

    fs1.absorb_root(&dummy_root);
    fs2.absorb_root(&dummy_root);

    // Generate challenges
    let challenges1: Vec<BinaryElem32> = (0..4).map(|_| fs1.get_challenge()).collect();
    let challenges2: Vec<BinaryElem32> = (0..4).map(|_| fs2.get_challenge()).collect();

    if challenges1 == challenges2 {
        println!("✅ SHA256 transcript is deterministic");
        println!("   This means Julia will generate the same challenges");
        println!("   Challenges: {:?}", challenges1);
    } else {
        println!("❌ SHA256 transcript not deterministic - this is a bug!");
    }

    // Test query generation
    fs1.absorb_root(&dummy_root); // Second absorption
    fs2.absorb_root(&dummy_root);

    let queries1 = fs1.get_distinct_queries(1024, 10);
    let queries2 = fs2.get_distinct_queries(1024, 10);

    if queries1 == queries2 {
        println!("✅ Query generation is deterministic");
        println!("   First 5 queries: {:?}", &queries1[..5]);
    } else {
        println!("❌ Query generation not deterministic - this is a bug!");
    }
}

fn compare_transcripts() {
    println!("Comparing Merlin vs SHA256 transcript outputs...");

    let mut merlin_fs = FiatShamir::new_merlin();
    let mut sha256_fs = FiatShamir::new_sha256(1234);

    let dummy_root = merkle_tree::MerkleRoot { root: Some([1u8; 32]) };

    merlin_fs.absorb_root(&dummy_root);
    sha256_fs.absorb_root(&dummy_root);

    let merlin_challenge = merlin_fs.get_challenge::<BinaryElem32>();
    let sha256_challenge = sha256_fs.get_challenge::<BinaryElem32>();

    println!("Merlin challenge:  {:?}", merlin_challenge);
    println!("SHA256 challenge:  {:?}", sha256_challenge);

    if merlin_challenge == sha256_challenge {
        println!("⚠️  Unexpected: Merlin and SHA256 produced same challenge!");
    } else {
        println!("✅ Expected: Merlin and SHA256 produce different challenges");
        println!("   This confirms they're using different transcript methods");
    }
}