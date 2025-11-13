use ligerito::{prove_sha256, hardcoded_config_12, hardcoded_config_12_verifier, verify_sha256};
use ligerito::utils::{evaluate_lagrange_basis, eval_sk_at_vks, evaluate_scaled_basis_inplace};
use ligerito_binary_fields::{BinaryElem32, BinaryElem128, BinaryFieldElement, BinaryPolynomial};
use std::marker::PhantomData;

fn main() {
    println!("=== TRACING VERIFY_LIGERO STEP BY STEP ===");

    // Create the same proof as our failing test
    let config = hardcoded_config_12(
        PhantomData::<BinaryElem32>,
        PhantomData::<BinaryElem128>,
    );

    // Use simple but diverse polynomial
    let poly: Vec<BinaryElem32> = (0..4096)
        .map(|i| BinaryElem32::from(i as u32))
        .collect();

    println!("Generating proof...");
    let proof = prove_sha256(&config, &poly).unwrap();

    // Get the data that would be passed to verify_ligero
    let yr = &proof.final_ligero_proof.yr;
    let opened_rows = &proof.final_ligero_proof.opened_rows;

    // We need to reconstruct the challenges (rs) that were used
    // These come from the sumcheck folding process
    println!("Final yr length: {}", yr.len());
    println!("Number of opened rows: {}", opened_rows.len());
    println!("Length of each opened row: {}", opened_rows[0].len());

    // Let's manually trace the first query to understand what should happen
    let query_idx = 0;
    let query = 42; // Let's use a specific query
    let row = &opened_rows[query_idx];

    println!("\n=== MANUAL TRACE FOR QUERY {} ===", query);
    println!("Row data: {:?}", &row[..4.min(row.len())]);

    // Extract the REAL challenges from the sumcheck transcript
    let sumcheck_challenges: Vec<BinaryElem128> = proof.sumcheck_transcript.transcript.iter()
        .map(|(s0, s1, s2)| {
            // Each round produces a challenge - we need to reconstruct this
            // For now, let's use s1 as a proxy (this might not be exactly right)
            *s1
        })
        .collect();

    println!("Real sumcheck challenges from transcript: {:?}", sumcheck_challenges);

    // Use the real challenges instead of dummy ones
    let challenges_to_use = if sumcheck_challenges.len() >= 2 {
        &sumcheck_challenges
    } else {
        println!("WARNING: Not enough sumcheck challenges, using dummy");
        &vec![BinaryElem128::from(123), BinaryElem128::from(456)]
    };

    // Compute Lagrange basis
    let gr = evaluate_lagrange_basis(challenges_to_use);
    println!("Lagrange basis gr: {:?}", &gr[..4.min(gr.len())]);

    // Compute dot = row * gr
    let dot = row.iter()
        .zip(gr.iter())
        .fold(BinaryElem128::zero(), |acc, (&r, &g)| {
            let r_u = BinaryElem128::from(r);
            acc.add(&r_u.mul(&g))
        });
    println!("dot = row * gr = {:?}", dot);

    // Now compute the multilinear basis evaluation
    let n = yr.len().trailing_zeros() as usize;
    println!("n = {} (so yr has {} = 2^{} elements)", n, yr.len(), n);

    let sks_vks: Vec<BinaryElem32> = eval_sk_at_vks(1 << n);
    println!("sks_vks length: {}", sks_vks.len());

    // Convert query to field element
    let query_for_basis = query % (1 << n);
    let qf = BinaryElem32::from_poly(
        <BinaryElem32 as BinaryFieldElement>::Poly::from_value(query_for_basis as u64)
    );
    println!("query_for_basis = {}, qf = {:?}", query_for_basis, qf);

    // Evaluate the multilinear basis
    let mut local_sks_x = vec![BinaryElem32::zero(); sks_vks.len()];
    let mut local_basis = vec![BinaryElem128::zero(); 1 << n];
    let scale = BinaryElem128::from(BinaryElem32::one());

    evaluate_scaled_basis_inplace(&mut local_sks_x, &mut local_basis, &sks_vks, qf, scale);

    // Check the basis structure
    let non_zero_count = local_basis.iter().filter(|&&x| x != BinaryElem128::zero()).count();
    println!("Non-zero elements in local_basis: {}", non_zero_count);

    if non_zero_count <= 5 {
        for (i, &val) in local_basis.iter().enumerate() {
            if val != BinaryElem128::zero() {
                println!("  local_basis[{}] = {:?}", i, val);
            }
        }
    }

    // Compute e = yr * local_basis
    let e = yr.iter()
        .zip(local_basis.iter())
        .fold(BinaryElem128::zero(), |acc, (&y, &b)| {
            let y_u = BinaryElem128::from(y);
            acc.add(&y_u.mul(&b))
        });
    println!("e = yr * local_basis = {:?}", e);

    println!("\n=== COMPARISON ===");
    println!("dot = {:?}", dot);
    println!("e   = {:?}", e);
    println!("Equal? {}", dot == e);

    if dot != e {
        println!("MISMATCH! This is the source of our verification failure.");

        // Let's see if we can understand why they're different
        println!("\n=== DEBUGGING THE MISMATCH ===");

        // Check if local_basis is a delta function as expected
        if non_zero_count == 1 {
            let non_zero_idx = local_basis.iter().position(|&x| x != BinaryElem128::zero()).unwrap();
            println!("local_basis is delta function at index {}", non_zero_idx);
            println!("So e = yr[{}] * scale = {:?} * {:?}", non_zero_idx, yr[non_zero_idx], scale);
            println!("Which gives: {:?}", BinaryElem128::from(yr[non_zero_idx]).mul(&scale));
        }

        // The issue might be that we're using the wrong challenges
        println!("NOTE: We used dummy challenges. The real issue might be:");
        println!("1. Wrong challenges passed to verify_ligero");
        println!("2. Wrong relationship being checked");
        println!("3. Bug in multilinear basis evaluation");
    }
}