use binary_fields::{BinaryElem32, BinaryElem128, BinaryFieldElement};
use ligerito::{
    prove, configs,
    VerifierConfig, FinalizedLigeritoProof,
    transcript::{FiatShamir, Transcript},
    ligero::hash_row,
    sumcheck_polys::induce_sumcheck_poly_debug,
    utils::eval_sk_at_vks,
};
use std::time::Instant;

// Manual inlined verification with timing
fn verify_with_timing<T, U>(
    config: &VerifierConfig,
    proof: &FinalizedLigeritoProof<T, U>,
) -> ligerito::Result<()>
where
    T: BinaryFieldElement + Send + Sync,
    U: BinaryFieldElement + Send + Sync + From<T>,
{
    let total_start = Instant::now();

    // Precompute basis evaluations
    let t0 = Instant::now();
    let cached_initial_sks: Vec<T> = eval_sk_at_vks(1 << config.initial_dim);
    let cached_recursive_sks: Vec<Vec<U>> = config.log_dims
        .iter()
        .map(|&dim| eval_sk_at_vks(1 << dim))
        .collect();
    println!("  [1] Basis caching:        {:7.2}ms", t0.elapsed().as_secs_f64() * 1000.0);

    // Initialize transcript
    let t0 = Instant::now();
    let mut fs = FiatShamir::new_merlin();
    fs.absorb_root(&proof.initial_ligero_cm.root);
    let partial_evals_0_t: Vec<T> = (0..config.initial_k)
        .map(|_| fs.get_challenge())
        .collect();
    let partial_evals_0: Vec<U> = partial_evals_0_t
        .iter()
        .map(|&x| U::from(x))
        .collect();
    println!("  [2] Transcript init:      {:7.2}ms", t0.elapsed().as_secs_f64() * 1000.0);

    // Initial Merkle verification
    let t0 = Instant::now();
    if proof.recursive_commitments.is_empty() {
        return Err(ligerito::LigeritoError::InvalidProof);
    }
    fs.absorb_root(&proof.recursive_commitments[0].root);
    let depth = config.initial_dim + 2;
    let queries = fs.get_distinct_queries(1 << depth, 148);
    let hashed_leaves: Vec<merkle_tree::Hash> = proof.initial_ligero_proof.opened_rows
        .iter()
        .map(|row| hash_row(row))
        .collect();

    if !merkle_tree::verify(
        &proof.initial_ligero_cm.root,
        &proof.initial_ligero_proof.merkle_proof,
        depth,
        &hashed_leaves,
        &queries,
    ) {
        return Err(ligerito::LigeritoError::InvalidProof);
    }
    println!("  [3] Initial Merkle:       {:7.2}ms", t0.elapsed().as_secs_f64() * 1000.0);

    // Initial sumcheck polynomial
    let t0 = Instant::now();
    let alpha = fs.get_challenge::<U>();
    let sks_vks = &cached_initial_sks;
    let (basis_poly, enforced_sum) = induce_sumcheck_poly_debug(
        config.initial_dim,
        &sks_vks,
        &proof.initial_ligero_proof.opened_rows,
        &partial_evals_0,
        &queries,
        alpha,
    );
    let basis_sum = basis_poly.iter().fold(U::zero(), |acc, &x| acc.add(&x));
    if basis_sum != enforced_sum {
        return Err(ligerito::LigeritoError::InvalidProof);
    }
    println!("  [4] Initial sumcheck poly:{:7.2}ms", t0.elapsed().as_secs_f64() * 1000.0);

    let mut current_sum = enforced_sum;

    // Sumcheck rounds
    let t0 = Instant::now();
    for round in 0..proof.sumcheck_transcript.transcript.len() {
        let coeffs = proof.sumcheck_transcript.transcript[round];
        let r = fs.get_challenge::<U>();

        let (s0, s1, _s2) = coeffs;
        let evaluated = s0.add(&s1.mul(&r));

        if evaluated != current_sum {
            return Err(ligerito::LigeritoError::InvalidProof);
        }

        current_sum = s0.add(&s1.mul(&r));
    }
    println!("  [5] Sumcheck rounds:      {:7.2}ms", t0.elapsed().as_secs_f64() * 1000.0);

    // Recursive rounds
    let t0 = Instant::now();
    for i in 0..config.recursive_steps {
        if i >= proof.recursive_proofs.len() {
            return Err(ligerito::LigeritoError::InvalidProof);
        }

        let ligero_proof = &proof.recursive_proofs[i];

        let depth_r = config.log_dims[i] + 2;
        let queries_r = fs.get_distinct_queries(1 << depth_r, 148);

        let hashed_r: Vec<merkle_tree::Hash> = ligero_proof.opened_rows
            .iter()
            .map(|row| hash_row(row))
            .collect();

        let next_cm = if i + 1 < proof.recursive_commitments.len() {
            &proof.recursive_commitments[i + 1].root
        } else {
            &proof.final_ligero_proof.merkle_proof.siblings[0]
        };

        if !merkle_tree::verify(
            &proof.recursive_commitments[i].root,
            &ligero_proof.merkle_proof,
            depth_r,
            &hashed_r,
            &queries_r,
        ) {
            return Err(ligerito::LigeritoError::InvalidProof);
        }

        let beta = fs.get_challenge::<U>();
        let glue_sum = current_sum.add(&beta.mul(&U::zero()));
        fs.absorb_elem(glue_sum);

        if i + 1 < proof.recursive_commitments.len() {
            fs.absorb_root(next_cm);
        }

        let alpha_r = fs.get_challenge::<U>();
        let sks_vks_r = &cached_recursive_sks[i];
        let (_, enforced_sum_next) = induce_sumcheck_poly_debug(
            config.log_dims[i],
            &sks_vks_r,
            &ligero_proof.opened_rows,
            &vec![],
            &queries_r,
            alpha_r,
        );

        current_sum = current_sum.add(&enforced_sum_next);
    }
    println!("  [6] Recursive rounds:     {:7.2}ms", t0.elapsed().as_secs_f64() * 1000.0);

    // Final round
    let t0 = Instant::now();
    let final_depth = config.log_dims[config.recursive_steps - 1] + 2;
    let final_queries = fs.get_distinct_queries(1 << final_depth, 148);

    let final_hashed: Vec<merkle_tree::Hash> = proof.final_ligero_proof.opened_rows
        .iter()
        .map(|row| hash_row(row))
        .collect();

    // Note: simplified - actual final verification more complex
    println!("  [7] Final verification:   {:7.2}ms", t0.elapsed().as_secs_f64() * 1000.0);

    println!("  ─────────────────────────────────────");
    println!("  Total verification:       {:7.2}ms", total_start.elapsed().as_secs_f64() * 1000.0);

    Ok(())
}

fn main() {
    println!("=== Detailed Verifier Profiling (2^20) ===\n");

    let n = 1 << 20;
    let poly: Vec<BinaryElem32> = (0..n)
        .map(|i| BinaryElem32::from_bits((i % 256) as u64))
        .collect();

    let prover_config = configs::hardcoded_config_20_prover();
    let verifier_config = configs::hardcoded_config_20_verifier();

    println!("Generating proof...");
    let proof = prove(&prover_config, &poly).expect("Proving failed");
    println!("Proof generated.\n");

    println!("Running profiled verification:\n");

    for run in 0..3 {
        println!("Run {}:", run + 1);
        match verify_with_timing::<BinaryElem32, BinaryElem128>(&verifier_config, &proof) {
            Ok(_) => {},
            Err(e) => println!("  ERROR: {:?}", e),
        }
        println!();
    }
}
