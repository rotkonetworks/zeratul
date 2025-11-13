use ligerito::{prove_sha256, hardcoded_config_12, hardcoded_config_12_verifier};
use ligerito::ligero::verify_ligero;
use ligerito::transcript::{Sha256Transcript, Transcript};
use ligerito::data_structures::*;
// Use local definition of evaluate_quadratic
use ligerito_binary_fields::{BinaryElem32, BinaryElem128, BinaryFieldElement};
use std::marker::PhantomData;

fn evaluate_quadratic<F: BinaryFieldElement>(coeffs: (F, F, F), x: F) -> F {
    let (a, b, c) = coeffs;
    // f(x) = ax^2 + bx + c
    a.mul(&x.mul(&x)).add(&b.mul(&x)).add(&c)
}

fn main() {
    println!("=== TESTING VERIFY_LIGERO WITH REAL CHALLENGES ===");

    // Generate proof
    let config = hardcoded_config_12(
        PhantomData::<BinaryElem32>,
        PhantomData::<BinaryElem128>,
    );

    let poly: Vec<BinaryElem32> = (0..4096)
        .map(|i| BinaryElem32::from(i as u32))
        .collect();

    println!("Generating proof...");
    let proof = prove_sha256(&config, &poly).unwrap();

    // Now manually extract the real challenges by simulating verifier
    let verifier_config = hardcoded_config_12_verifier();
    let mut fs = Sha256Transcript::new(1234);

    // Follow the same steps as verifier to get the real rs
    fs.absorb_root(&proof.initial_ligero_cm.root);

    // Get initial challenges in base field
    let partial_evals_0_t: Vec<BinaryElem32> = (0..verifier_config.initial_k)
        .map(|_| fs.get_challenge())
        .collect();

    let partial_evals_0: Vec<BinaryElem128> = partial_evals_0_t
        .iter()
        .map(|&x| BinaryElem128::from(x))
        .collect();

    // First recursive commitment
    fs.absorb_root(&proof.recursive_commitments[0].root);

    // Verify initial proof
    let depth = verifier_config.initial_dim + 3; // LOG_INV_RATE = 3
    let queries = fs.get_distinct_queries(1 << depth, 128); // S = 128

    let alpha = fs.get_challenge::<BinaryElem128>();

    // Get the enforced sum for initial step
    use ligerito::sumcheck_polys::induce_sumcheck_poly;
    use ligerito::utils::eval_sk_at_vks;

    let sks_vks: Vec<BinaryElem32> = eval_sk_at_vks(1 << verifier_config.initial_dim);
    let (_, enforced_sum) = induce_sumcheck_poly(
        verifier_config.initial_dim,
        &sks_vks,
        &proof.initial_ligero_proof.opened_rows,
        &partial_evals_0,
        &queries,
        alpha,
    );

    let mut current_sum = enforced_sum;
    fs.absorb_elem(current_sum);

    let mut transcript_idx = 0;

    // Extract REAL rs from the final recursive step (i = 0)
    let i = 0;
    let mut rs = Vec::with_capacity(verifier_config.ks[i]);

    println!("Extracting real challenges from sumcheck transcript...");

    // Sumcheck rounds - this is where the REAL rs are generated
    for round in 0..verifier_config.ks[i] {
        if transcript_idx >= proof.sumcheck_transcript.transcript.len() {
            panic!("Not enough transcript entries");
        }

        let coeffs = proof.sumcheck_transcript.transcript[transcript_idx];
        let claimed_sum = evaluate_quadratic(coeffs, BinaryElem128::zero())
            .add(&evaluate_quadratic(coeffs, BinaryElem128::one()));

        println!("Round {}: coeffs = {:?}", round, coeffs);
        println!("Round {}: claimed_sum = {:?}, current_sum = {:?}", round, claimed_sum, current_sum);

        if claimed_sum != current_sum {
            panic!("Sumcheck failed at round {}", round);
        }

        let ri = fs.get_challenge::<BinaryElem128>();
        rs.push(ri);
        current_sum = evaluate_quadratic(coeffs, ri);
        fs.absorb_elem(current_sum);

        println!("Round {}: challenge ri = {:?}", round, ri);
        println!("Round {}: new current_sum = {:?}", round, current_sum);

        transcript_idx += 1;
    }

    println!("\nReal folding challenges rs: {:?}", rs);

    // Now get to the final round
    fs.absorb_elems(&proof.final_ligero_proof.yr);

    let depth = verifier_config.log_dims[i] + 3;
    let final_queries = fs.get_distinct_queries(1 << depth, 128);

    println!("\nFinal queries: {:?}", &final_queries[..5.min(final_queries.len())]);

    // Test verify_ligero with REAL challenges
    println!("\n=== TESTING WITH REAL CHALLENGES ===");

    for &query in &final_queries[..3.min(final_queries.len())] {
        if query < proof.final_ligero_proof.opened_rows.len() {
            let single_row = vec![proof.final_ligero_proof.opened_rows[query].clone()];
            let single_query = vec![query];

            println!("\n--- Testing query {} with REAL challenges ---", query);

            let result = std::panic::catch_unwind(|| {
                verify_ligero(&single_query, &single_row, &proof.final_ligero_proof.yr, &rs);
            });

            match result {
                Ok(()) => println!("Query {} PASSED with REAL challenges!", query),
                Err(_) => println!("Query {} FAILED with REAL challenges", query),
            }
        }
    }

    println!("\nIf queries still fail with REAL challenges, the issue is deeper.");
    println!("If they pass, we know the challenge extraction was the problem.");
}