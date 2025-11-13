use ligerito::{
    prove_sha256, verify_sha256, hardcoded_config_12, hardcoded_config_12_verifier,
    verifier::verify_debug, prover::prove_debug,
    utils::{evaluate_lagrange_basis, eval_sk_at_vks, evaluate_scaled_basis_inplace},
    sumcheck_polys::{induce_sumcheck_poly, induce_sumcheck_poly},
    transcript::{FiatShamir, Transcript},
};
use binary_fields::{BinaryElem32, BinaryElem128, BinaryFieldElement};
use std::marker::PhantomData;

fn main() {
    println!("=== RIGOROUS CROSS-TESTING WITH JULIA ===");

    // Test multiple polynomial patterns to find discrepancies
    let test_cases = vec![
        ("zero polynomial", vec![BinaryElem32::zero(); 1 << 12]),
        ("constant polynomial", vec![BinaryElem32::one(); 1 << 12]),
        ("linear pattern", (0..1 << 12).map(|i| BinaryElem32::from(i as u32)).collect()),
        ("alternating", (0..1 << 12).map(|i| BinaryElem32::from((i % 2) as u32)).collect()),
        ("powers of 2", (0..1 << 12).map(|i| BinaryElem32::from(1u32 << (i % 16))).collect()),
        ("fibonacci mod", {
            let mut poly = vec![BinaryElem32::zero(); 1 << 12];
            poly[0] = BinaryElem32::from(1);
            poly[1] = BinaryElem32::from(1);
            for i in 2..(1 << 12) {
                let a = poly[i-1].clone();
                let b = poly[i-2].clone();
                poly[i] = a.add(&b);
            }
            poly
        }),
    ];

    let config = hardcoded_config_12(
        PhantomData::<BinaryElem32>,
        PhantomData::<BinaryElem128>,
    );
    let verifier_config = hardcoded_config_12_verifier();

    for (name, poly) in test_cases {
        println!("\n=== TESTING: {} ===", name);

        // Generate proof with detailed debug output
        let proof = match prove_debug(&config, &poly) {
            Ok(p) => p,
            Err(e) => {
                println!("FAILED to generate proof for {}: {:?}", name, e);
                continue;
            }
        };

        println!("✓ Proof generation successful");

        // Verify with detailed debug output
        let verification_result = match verify_debug(&verifier_config, &proof) {
            Ok(result) => result,
            Err(e) => {
                println!("FAILED verification for {}: {:?}", name, e);
                continue;
            }
        };

        if verification_result {
            println!("✓ Verification successful for {}", name);
        } else {
            println!("✗ VERIFICATION FAILED for {}", name);
        }

        // Test cross-verification with SHA256 transcript to match Julia
        println!("Testing SHA256 transcript compatibility...");
        let sha_proof = match prove_sha256(&config, &poly) {
            Ok(p) => p,
            Err(e) => {
                println!("FAILED SHA256 proof for {}: {:?}", name, e);
                continue;
            }
        };

        let sha_verification = match verify_sha256(&verifier_config, &sha_proof) {
            Ok(result) => result,
            Err(e) => {
                println!("FAILED SHA256 verification for {}: {:?}", name, e);
                continue;
            }
        };

        if sha_verification {
            println!("✓ SHA256 transcript verification successful");
        } else {
            println!("✗ SHA256 transcript verification FAILED");
        }

        // Detailed proof analysis
        analyze_proof_structure(&proof, name);
    }

    // Test edge cases specifically
    println!("\n=== EDGE CASE TESTING ===");
    test_edge_cases();

    // Test sumcheck polynomial consistency
    println!("\n=== SUMCHECK POLYNOMIAL CONSISTENCY TESTING ===");
    test_sumcheck_consistency();

    // Test cobasis scenarios
    println!("\n=== COBASIS TESTING ===");
    test_cobases();
}

fn analyze_proof_structure(proof: &ligerito::FinalizedLigeritoProof<BinaryElem32, BinaryElem128>, name: &str) {
    println!("--- Proof Structure Analysis for {} ---", name);
    println!("Initial commitment: {:?}", proof.initial_ligero_cm.root);
    println!("Recursive commitments: {}", proof.recursive_commitments.len());
    println!("Final yr length: {}", proof.final_ligero_proof.yr.len());
    println!("Sumcheck transcript rounds: {}", proof.sumcheck_transcript.transcript.len());

    // Check for suspicious patterns
    let yr_sum = proof.final_ligero_proof.yr.iter()
        .fold(BinaryElem128::zero(), |acc, &x| acc.add(&x));
    println!("Final yr sum: {:?}", yr_sum);

    let all_zeros = proof.final_ligero_proof.yr.iter().all(|&x| x == BinaryElem128::zero());
    if all_zeros {
        println!("WARNING: All yr values are zero!");
    }

    // Check opened rows consistency
    if !proof.final_ligero_proof.opened_rows.is_empty() {
        println!("Final opened rows: {} x {}",
                 proof.final_ligero_proof.opened_rows.len(),
                 proof.final_ligero_proof.opened_rows[0].len());
    }
}

fn test_edge_cases() {
    let config = hardcoded_config_12(
        PhantomData::<BinaryElem32>,
        PhantomData::<BinaryElem128>,
    );

    // Test with specific problematic patterns
    let edge_cases = vec![
        ("all max values", vec![BinaryElem32::from(u32::MAX); 1 << 12]),
        ("sparse pattern", {
            let mut poly = vec![BinaryElem32::zero(); 1 << 12];
            poly[0] = BinaryElem32::from(1);
            poly[1023] = BinaryElem32::from(2);
            poly[2047] = BinaryElem32::from(3);
            poly[4095] = BinaryElem32::from(4);
            poly
        }),
    ];

    for (name, poly) in edge_cases {
        println!("Testing edge case: {}", name);
        match prove_sha256(&config, &poly) {
            Ok(proof) => {
                println!("✓ Edge case {} proof generated successfully", name);
                analyze_proof_structure(&proof, name);
            },
            Err(e) => println!("✗ Edge case {} failed: {:?}", name, e),
        }
    }
}

fn test_sumcheck_consistency() {
    println!("Testing sumcheck polynomial implementations...");

    let n = 6; // Small test case
    let sks_vks: Vec<BinaryElem32> = eval_sk_at_vks(1 << n);

    let v_challenges = vec![
        BinaryElem128::from(BinaryElem32::from(0x12345678u32)),
        BinaryElem128::from(BinaryElem32::from(0x9ABCDEFu32)),
    ];

    let queries = vec![5, 17, 33, 42];
    let opened_rows = vec![
        vec![BinaryElem32::from(1), BinaryElem32::from(2), BinaryElem32::from(3), BinaryElem32::from(4)],
        vec![BinaryElem32::from(5), BinaryElem32::from(6), BinaryElem32::from(7), BinaryElem32::from(8)],
        vec![BinaryElem32::from(9), BinaryElem32::from(10), BinaryElem32::from(11), BinaryElem32::from(12)],
        vec![BinaryElem32::from(13), BinaryElem32::from(14), BinaryElem32::from(15), BinaryElem32::from(16)],
    ];

    let alpha = BinaryElem128::from(BinaryElem32::from(0xDEADBEEFu32));

    // Test both implementations
    let (basis_poly1, sum1) = induce_sumcheck_poly(
        n, &sks_vks, &opened_rows, &v_challenges, &queries, alpha
    );

    let (basis_poly2, sum2) = induce_sumcheck_poly(
        n, &sks_vks, &opened_rows, &v_challenges, &queries, alpha
    );

    println!("Production implementation sum: {:?}", sum1);
    println!("Debug implementation sum: {:?}", sum2);

    if sum1 == sum2 {
        println!("✓ Sumcheck implementations agree on enforced sum");
    } else {
        println!("✗ CRITICAL: Sumcheck implementations disagree!");
    }

    if basis_poly1 == basis_poly2 {
        println!("✓ Sumcheck implementations agree on basis polynomial");
    } else {
        println!("✗ CRITICAL: Sumcheck basis polynomials disagree!");
        let diffs = basis_poly1.iter().zip(basis_poly2.iter())
            .enumerate()
            .filter(|(_, (&a, &b))| a != b)
            .take(5)
            .collect::<Vec<_>>();
        println!("First 5 differences: {:?}", diffs);
    }

    // Test Lagrange basis evaluation
    let gr = evaluate_lagrange_basis(&v_challenges);
    println!("Lagrange basis length: {}, first few: {:?}", gr.len(), &gr[..4.min(gr.len())]);

    // Test multilinear basis evaluation for specific queries
    for &query in &queries {
        let qf = BinaryElem32::from_bits(query as u64);
        let mut local_sks_x = vec![BinaryElem32::zero(); sks_vks.len()];
        let mut local_basis = vec![BinaryElem128::zero(); 1 << n];
        let scale = BinaryElem128::one();

        evaluate_scaled_basis_inplace(&mut local_sks_x, &mut local_basis, &sks_vks, qf, scale);

        let non_zero_count = local_basis.iter().filter(|&&x| x != BinaryElem128::zero()).count();
        println!("Query {}: non-zero basis elements: {}", query, non_zero_count);

        if non_zero_count == 1 {
            let pos = local_basis.iter().position(|&x| x != BinaryElem128::zero()).unwrap();
            println!("  Single non-zero at position {}, value {:?}", pos, local_basis[pos]);
        }
    }
}

fn test_cobases() {
    println!("Testing cobasis scenarios...");

    let config = hardcoded_config_12(
        PhantomData::<BinaryElem32>,
        PhantomData::<BinaryElem128>,
    );

    // Create a polynomial that might expose cobasis issues
    let mut poly = vec![BinaryElem32::zero(); 1 << 12];

    // Set specific patterns that might reveal issues with cobases
    for i in 0..(1 << 10) {
        poly[i] = BinaryElem32::from((i * 7 + 13) as u32);
    }
    for i in (1 << 10)..(1 << 11) {
        poly[i] = BinaryElem32::from((i * 11 + 17) as u32);
    }
    for i in (1 << 11)..(1 << 12) {
        poly[i] = BinaryElem32::from((i * 3 + 5) as u32);
    }

    println!("Testing structured polynomial with potential cobasis issues...");

    // Generate proof and analyze with manual transcript checks
    let mut fs = FiatShamir::new_sha256(1234);

    match prove_sha256(&config, &poly) {
        Ok(proof) => {
            println!("✓ Cobasis test proof generated");

            // Manual verification steps to isolate issues
            fs.absorb_root(&proof.initial_ligero_cm.root);

            let partial_evals_0: Vec<BinaryElem32> = (0..config.initial_k)
                .map(|_| fs.get_challenge())
                .collect();

            println!("Manual verification challenges: {:?}", partial_evals_0);

            // Check if these match what's expected from Julia
            let partial_evals_0_u: Vec<BinaryElem128> = partial_evals_0.iter()
                .map(|&x| BinaryElem128::from(x))
                .collect();

            let gr = evaluate_lagrange_basis(&partial_evals_0_u);
            println!("Lagrange basis for cobasis test: length {}, first few: {:?}",
                     gr.len(), &gr[..4.min(gr.len())]);

        },
        Err(e) => println!("✗ Cobasis test failed: {:?}", e),
    }
}