use ligerito::{prove_sha256, hardcoded_config_12};
use ligerito::utils::{evaluate_lagrange_basis, eval_sk_at_vks, evaluate_scaled_basis_inplace};
use binary_fields::{BinaryElem32, BinaryElem128, BinaryFieldElement, BinaryPolynomial};
use std::marker::PhantomData;

fn main() {
    println!("=== cross verification with julia implementation ===");

    // use exact same polynomial as julia test
    let poly: Vec<BinaryElem32> = (0..4096)
        .map(|i| BinaryElem32::from(i as u32))
        .collect();

    println!("polynomial size: {}", poly.len());
    println!("first 10 elements: {:?}", &poly[..10]);

    let config = hardcoded_config_12(
        PhantomData::<BinaryElem32>,
        PhantomData::<BinaryElem128>,
    );

    println!("config initial_dims: {:?}", config.initial_dims);
    println!("config initial_k: {}", config.initial_k);

    // generate proof with same seed as julia
    let proof = prove_sha256(&config, &poly).unwrap();

    println!("\n=== proof structure comparison ===");
    println!("initial commitment: {:?}", proof.initial_ligero_cm.root);
    println!("recursive commitments: {}", proof.recursive_commitments.len());
    println!("final yr length: {}", proof.final_ligero_proof.yr.len());
    println!("sumcheck rounds: {}", proof.sumcheck_transcript.transcript.len());

    // print first few yr values for comparison
    println!("\n=== final yr values (first 10) ===");
    for (i, &val) in proof.final_ligero_proof.yr.iter().take(10).enumerate() {
        println!("[{}] {:?}", i, val);
    }

    // print sumcheck transcript for comparison
    println!("\n=== sumcheck transcript (coefficients) ===");
    for (i, coeffs) in proof.sumcheck_transcript.transcript.iter().enumerate() {
        println!("round {}: {:?}", i + 1, coeffs);
    }

    // test lagrange basis evaluation with known challenges
    let test_challenges = vec![
        BinaryElem128::from(1),
        BinaryElem128::from(2),
    ];

    println!("\n=== lagrange basis test ===");
    let gr = evaluate_lagrange_basis(&test_challenges);
    println!("challenges: {:?}", test_challenges);
    println!("lagrange basis: {:?}", gr);

    // test multilinear basis evaluation
    println!("\n=== multilinear basis test ===");
    let n = 6; // 2^6 = 64 = yr.len()
    let sks_vks: Vec<BinaryElem32> = eval_sk_at_vks(1 << n);
    println!("sks_vks length: {}", sks_vks.len());
    println!("first 10 sks_vks: {:?}", &sks_vks[..10.min(sks_vks.len())]);

    // test basis evaluation for specific queries
    for query in [0, 1, 42] {
        let qf = BinaryElem32::from_poly(
            <BinaryElem32 as BinaryFieldElement>::Poly::from_value(query as u64)
        );

        let mut local_sks_x = vec![BinaryElem32::zero(); sks_vks.len()];
        let mut local_basis = vec![BinaryElem128::zero(); 1 << n];
        let scale = BinaryElem128::from(BinaryElem32::one());

        evaluate_scaled_basis_inplace(&mut local_sks_x, &mut local_basis, &sks_vks, qf, scale);

        let non_zero_count = local_basis.iter().filter(|&&x| x != BinaryElem128::zero()).count();
        println!("query {}: qf={:?}, non_zero_basis_elements={}", query, qf, non_zero_count);

        if non_zero_count <= 5 {
            for (i, &val) in local_basis.iter().enumerate() {
                if val != BinaryElem128::zero() {
                    println!("  basis[{}] = {:?}", i, val);
                }
            }
        }
    }

    // test opened rows structure
    println!("\n=== opened rows structure ===");
    println!("number of opened rows: {}", proof.final_ligero_proof.opened_rows.len());
    if !proof.final_ligero_proof.opened_rows.is_empty() {
        println!("row length: {}", proof.final_ligero_proof.opened_rows[0].len());
        println!("first row (first 10): {:?}",
            &proof.final_ligero_proof.opened_rows[0][..10.min(proof.final_ligero_proof.opened_rows[0].len())]);
    }

    println!("\n=== export data for julia comparison ===");
    println!("# rust proof data");
    println!("# polynomial: {:?}", &poly[..10]);
    println!("# final_yr: {:?}", &proof.final_ligero_proof.yr[..5.min(proof.final_ligero_proof.yr.len())]);
    println!("# sumcheck_transcript: {:?}", proof.sumcheck_transcript.transcript);
    println!("# initial_commitment: {:?}", proof.initial_ligero_cm.root);
}