use ligerito::{
    prove_sha256, verify_sha256, hardcoded_config_12, hardcoded_config_12_verifier,
    transcript::{FiatShamir, Transcript},
    sumcheck_polys::induce_sumcheck_poly,
    utils::eval_sk_at_vks,
};
use binary_fields::{BinaryElem32, BinaryElem128, BinaryFieldElement};
use std::marker::PhantomData;

/// This test verifies Julia-Rust compatibility specifically for the indexing issue
/// Julia uses 1-based indexing internally but we need to ensure our 0-based indexing
/// produces equivalent results when accounting for the mathematical relationships
fn main() {
    println!("=== JULIA-RUST COMPATIBILITY TEST ===");
    println!("Testing specific discrepancies between Julia 1-based and Rust 0-based indexing");

    // Use the exact same polynomial as Julia test
    let poly: Vec<BinaryElem32> = (0..4096).map(|i| BinaryElem32::from(i as u32)).collect();

    let config = hardcoded_config_12(
        PhantomData::<BinaryElem32>,
        PhantomData::<BinaryElem128>,
    );
    let verifier_config = hardcoded_config_12_verifier();

    println!("Polynomial size: {}", poly.len());
    println!("First 10 elements: {:?}", &poly[..10]);

    // Generate proof with SHA256 to match Julia
    println!("\n=== GENERATING PROOF WITH SHA256 TRANSCRIPT ===");
    let proof = match prove_sha256(&config, &poly) {
        Ok(p) => p,
        Err(e) => {
            println!("Failed to generate proof: {:?}", e);
            return;
        }
    };

    println!("✓ Proof generated successfully");
    println!("Initial commitment: {:?}", proof.initial_ligero_cm.root);
    println!("Recursive commitments: {}", proof.recursive_commitments.len());
    println!("Final yr length: {}", proof.final_ligero_proof.yr.len());
    println!("Sumcheck rounds: {}", proof.sumcheck_transcript.transcript.len());

    // Print key values for Julia comparison
    println!("\n=== KEY VALUES FOR JULIA COMPARISON ===");
    println!("Final yr (first 10):");
    for (i, &val) in proof.final_ligero_proof.yr.iter().take(10).enumerate() {
        println!("  [{}] {:?}", i, val);
    }

    println!("Sumcheck transcript:");
    for (i, coeffs) in proof.sumcheck_transcript.transcript.iter().enumerate() {
        println!("  Round {}: {:?}", i + 1, coeffs);
    }

    // Verify the proof
    println!("\n=== VERIFYING PROOF ===");
    let verification_result = match verify_sha256(&verifier_config, &proof) {
        Ok(result) => result,
        Err(e) => {
            println!("Verification error: {:?}", e);
            return;
        }
    };

    if verification_result {
        println!("✓ Verification successful");
    } else {
        println!("✗ Verification failed");
    }

    // Manual step-by-step verification to identify any discrepancies
    println!("\n=== MANUAL STEP-BY-STEP VERIFICATION ===");
    manual_verification_steps(&config, &verifier_config, &proof);

    // Test specific Julia patterns
    println!("\n=== TESTING JULIA PATTERNS ===");
    test_julia_patterns();
}

fn manual_verification_steps(
    prover_config: &ligerito::ProverConfig<BinaryElem32, BinaryElem128>,
    verifier_config: &ligerito::VerifierConfig,
    proof: &ligerito::FinalizedLigeritoProof<BinaryElem32, BinaryElem128>,
) {
    let mut fs = FiatShamir::new_sha256(1234);

    // Step 1: Absorb initial commitment
    fs.absorb_root(&proof.initial_ligero_cm.root);
    println!("Step 1: Absorbed initial commitment");

    // Step 2: Generate initial challenges (exactly as Julia does)
    let partial_evals_0: Vec<BinaryElem32> = (0..prover_config.initial_k)
        .map(|_| fs.get_challenge())
        .collect();
    println!("Step 2: Initial challenges: {:?}", partial_evals_0);

    // Step 3: Absorb first recursive commitment
    fs.absorb_root(&proof.recursive_commitments[0].root);
    println!("Step 3: Absorbed recursive commitment");

    // Step 4: Generate queries
    let depth = verifier_config.initial_dim + 2; // LOG_INV_RATE = 2
    let queries = fs.get_distinct_queries(1 << depth, 148); // S = 148
    println!("Step 4: Generated {} queries", queries.len());
    println!("  First 10 queries: {:?}", &queries[..10.min(queries.len())]);
    println!("  Min query: {}, Max query: {}",
             queries.iter().min().unwrap_or(&0),
             queries.iter().max().unwrap_or(&0));

    // Step 5: Test sumcheck polynomial generation
    let alpha = fs.get_challenge::<BinaryElem128>();
    println!("Step 5: Alpha challenge: {:?}", alpha);

    let sks_vks: Vec<BinaryElem32> = eval_sk_at_vks(1 << verifier_config.initial_dim);
    let partial_evals_0_u: Vec<BinaryElem128> = partial_evals_0.iter()
        .map(|&x| BinaryElem128::from(x))
        .collect();

    // This is where the mathematical relationship matters most
    let (basis_poly, enforced_sum) = induce_sumcheck_poly(
        verifier_config.initial_dim,
        &sks_vks,
        &proof.initial_ligero_proof.opened_rows,
        &partial_evals_0_u,
        &queries,
        alpha,
    );

    println!("Step 5: Sumcheck polynomial generated");
    println!("  Basis poly length: {}", basis_poly.len());
    println!("  Enforced sum: {:?}", enforced_sum);

    // Verify sum consistency
    let basis_sum = basis_poly.iter().fold(BinaryElem128::zero(), |acc, &x| acc.add(&x));
    if basis_sum == enforced_sum {
        println!("  ✓ Sum consistency check passed");
    } else {
        println!("  ✗ Sum consistency check failed");
        println!("    Expected: {:?}", enforced_sum);
        println!("    Actual: {:?}", basis_sum);
    }

    // Compare with proof structure
    println!("\nProof structure validation:");
    println!("  Opened rows count: {}", proof.initial_ligero_proof.opened_rows.len());
    println!("  Expected queries count: {}", queries.len());
    if proof.initial_ligero_proof.opened_rows.len() == queries.len() {
        println!("  ✓ Opened rows count matches query count");
    } else {
        println!("  ✗ Opened rows count mismatch");
    }
}

fn test_julia_patterns() {
    println!("Testing specific patterns that might differ between Julia and Rust...");

    // Test 1: Index conversion patterns
    println!("\n1. Index conversion patterns:");
    let julia_1_based = vec![1, 5, 10, 42]; // Julia 1-based indices
    let rust_0_based: Vec<usize> = julia_1_based.iter().map(|&x| x - 1).collect(); // Convert to 0-based
    println!("  Julia (1-based): {:?}", julia_1_based);
    println!("  Rust (0-based):  {:?}", rust_0_based);

    // Test 2: Query modulo operations
    println!("\n2. Query modulo operations:");
    let n = 6; // 2^6 = 64
    for &query in &[42, 75, 123] {
        let rust_mod = query % (1 << n);
        let julia_equivalent = ((query - 1) % (1 << n)) + 1; // Simulate Julia 1-based modulo
        println!("  Query {}: Rust mod {} = {}, Julia equivalent would be {}",
                 query, 1 << n, rust_mod, julia_equivalent);
    }

    // Test 3: Array access patterns
    println!("\n3. Array access patterns:");
    let test_array = vec![10, 20, 30, 40, 50];
    let julia_indices = vec![1, 3, 5]; // Julia 1-based
    let rust_indices = vec![0, 2, 4];  // Rust 0-based (equivalent positions)

    println!("  Array: {:?}", test_array);
    println!("  Julia access [1,3,5]: [{}, {}, {}]",
             test_array[0], test_array[2], test_array[4]);
    println!("  Rust access [0,2,4]:  [{}, {}, {}]",
             test_array[0], test_array[2], test_array[4]);

    // They should be identical - the mathematical relationship is preserved
}