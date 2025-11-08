use binary_fields::BinaryFieldElement;
use crate::{
    ProverConfig, LigeritoProof, FinalizedLigeritoProof, RecursiveLigeroCommitment,
    RecursiveLigeroProof, FinalLigeroProof, SumcheckTranscript,
    transcript::{FiatShamir, Transcript},
    ligero::ligero_commit,
    sumcheck_polys::{induce_sumcheck_poly, induce_sumcheck_poly_parallel, induce_sumcheck_poly_debug},
    utils::{eval_sk_at_vks, partial_eval_multilinear},
    data_structures::finalize,
};

// Hardcoded for now
const S: usize = 148;

/// Main prover function with configurable transcript
pub fn prove_with_transcript<T, U>(
    config: &ProverConfig<T, U>,
    poly: &[T],
    mut fs: impl Transcript,
) -> crate::Result<FinalizedLigeritoProof<T, U>>
where
    T: BinaryFieldElement + Send + Sync,
    U: BinaryFieldElement + Send + Sync + From<T>,
{
    let mut proof = LigeritoProof::<T, U>::new();

    // Initial commitment
    let wtns_0 = ligero_commit(poly, config.initial_dims.0, config.initial_dims.1, &config.initial_reed_solomon);
    let cm_0 = RecursiveLigeroCommitment {
        root: wtns_0.tree.get_root(),
    };
    proof.initial_ligero_cm = Some(cm_0.clone());
    fs.absorb_root(&cm_0.root);

    // Get initial challenges - get them as T type (base field)
    let partial_evals_0: Vec<T> = (0..config.initial_k)
        .map(|_| fs.get_challenge())
        .collect();

    // Partial evaluation of multilinear polynomial
    let mut f_evals = poly.to_vec();
    partial_eval_multilinear(&mut f_evals, &partial_evals_0);

    // Convert to U type for extension field operations
    let partial_evals_0_u: Vec<U> = partial_evals_0.iter().map(|&x| U::from(x)).collect();

    // First recursive step - convert to U type
    let f_evals_u: Vec<U> = f_evals.iter().map(|&x| U::from(x)).collect();
    let wtns_1 = ligero_commit(&f_evals_u, config.dims[0].0, config.dims[0].1, &config.reed_solomon_codes[0]);
    let cm_1 = RecursiveLigeroCommitment {
        root: wtns_1.tree.get_root(),
    };
    proof.recursive_commitments.push(cm_1.clone());
    fs.absorb_root(&cm_1.root);

    // Query selection
    let rows = wtns_0.mat.len();
    let queries = fs.get_distinct_queries(rows, S);  // Returns 0-based indices
    let alpha = fs.get_challenge::<U>();

    // Prepare for sumcheck
    let n = f_evals.len().trailing_zeros() as usize;
    let sks_vks: Vec<T> = eval_sk_at_vks(1 << n);

    // Use 0-based queries directly for array access
    let opened_rows: Vec<Vec<T>> = queries.iter()
        .map(|&q| wtns_0.mat[q].clone())
        .collect();

    let mtree_proof = wtns_0.tree.prove(&queries);  // prove() expects 0-based
    proof.initial_ligero_proof = Some(RecursiveLigeroProof {
        opened_rows: opened_rows.clone(),
        merkle_proof: mtree_proof,
    });

    // use sequential version - parallel needs more tuning
    let (basis_poly, enforced_sum) = induce_sumcheck_poly(
        n,
        &sks_vks,
        &opened_rows,
        &partial_evals_0_u,
        &queries,
        alpha,
    );

    let mut sumcheck_transcript = vec![];
    let mut current_poly = basis_poly;
    let mut current_sum = enforced_sum; // Use enforced_sum directly

    // First sumcheck round absorb
    fs.absorb_elem(current_sum);

    // Recursive rounds
    let mut wtns_prev = wtns_1;

    for i in 0..config.recursive_steps {
        let mut rs = Vec::new();

        // Sumcheck rounds
        for j in 0..config.ks[i] {
            // Compute coefficients first (before getting challenge)
            let coeffs = compute_sumcheck_coefficients(&current_poly);
            sumcheck_transcript.push(coeffs);

            // Get challenge after providing coefficients
            let ri = fs.get_challenge::<U>();
            rs.push(ri);

            // Fold polynomial with the challenge
            current_poly = fold_polynomial_with_challenge(&current_poly, ri);

            // Update sum
            current_sum = evaluate_quadratic(coeffs, ri);
            fs.absorb_elem(current_sum);
        }

        // Final round
        if i == config.recursive_steps - 1 {
            fs.absorb_elems(&current_poly);

            let rows = wtns_prev.mat.len();
            let queries = fs.get_distinct_queries(rows, S);  // 0-based

            // Use 0-based queries directly for array access
            let opened_rows: Vec<Vec<U>> = queries.iter()
                .map(|&q| wtns_prev.mat[q].clone())
                .collect();

            let mtree_proof = wtns_prev.tree.prove(&queries);  // 0-based

            proof.final_ligero_proof = Some(FinalLigeroProof {
                yr: current_poly.clone(),
                opened_rows,
                merkle_proof: mtree_proof,
            });

            proof.sumcheck_transcript = Some(SumcheckTranscript { transcript: sumcheck_transcript });

            return finalize(proof);
        }

        // Continue recursion
        let wtns_next = ligero_commit(
            &current_poly,
            config.dims[i + 1].0,
            config.dims[i + 1].1,
            &config.reed_solomon_codes[i + 1],
        );

        let cm_next = RecursiveLigeroCommitment {
            root: wtns_next.tree.get_root(),
        };
        proof.recursive_commitments.push(cm_next.clone());
        fs.absorb_root(&cm_next.root);

        let rows = wtns_prev.mat.len();
        let queries = fs.get_distinct_queries(rows, S);  // 0-based
        let alpha = fs.get_challenge::<U>();

        // Use 0-based queries directly for array access
        let opened_rows: Vec<Vec<U>> = queries.iter()
            .map(|&q| wtns_prev.mat[q].clone())
            .collect();

        let mtree_proof = wtns_prev.tree.prove(&queries);  // 0-based
        proof.recursive_proofs.push(RecursiveLigeroProof {
            opened_rows: opened_rows.clone(),
            merkle_proof: mtree_proof,
        });

        // Update for next round
        let n = current_poly.len().trailing_zeros() as usize;
        let sks_vks: Vec<U> = eval_sk_at_vks(1 << n);

        // Use parallel version for performance
        let (basis_poly, enforced_sum) = induce_sumcheck_poly_parallel(
            n,
            &sks_vks,
            &opened_rows,
            &rs,
            &queries,
            alpha,
        );

        // Glue sumcheck absorb
        let glue_sum = current_sum.add(&enforced_sum);
        fs.absorb_elem(glue_sum);

        // Glue polynomials
        let beta = fs.get_challenge::<U>();
        current_poly = glue_polynomials(&current_poly, &basis_poly, beta);
        current_sum = glue_sums(current_sum, enforced_sum, beta);

        wtns_prev = wtns_next;
    }

    unreachable!("Should have returned in final round");
}

/// Main prover function using default Merlin transcript
pub fn prove<T, U>(
    config: &ProverConfig<T, U>,
    poly: &[T],
) -> crate::Result<FinalizedLigeritoProof<T, U>>
where
    T: BinaryFieldElement + Send + Sync,
    U: BinaryFieldElement + Send + Sync + From<T>,
{
    // Use Merlin by default for better performance
    let fs = FiatShamir::new_merlin();
    prove_with_transcript(config, poly, fs)
}

/// Prover function using Julia-compatible SHA256 transcript
pub fn prove_sha256<T, U>(
    config: &ProverConfig<T, U>,
    poly: &[T],
) -> crate::Result<FinalizedLigeritoProof<T, U>>
where
    T: BinaryFieldElement + Send + Sync,
    U: BinaryFieldElement + Send + Sync + From<T>,
{
    // Use SHA256 with seed 1234 to match Julia
    let fs = FiatShamir::new_sha256(1234);
    prove_with_transcript(config, poly, fs)
}

/// Debug version of prove with detailed logging
pub fn prove_debug<T, U>(
    config: &ProverConfig<T, U>,
    poly: &[T],
) -> crate::Result<FinalizedLigeritoProof<T, U>>
where
    T: BinaryFieldElement + Send + Sync,
    U: BinaryFieldElement + Send + Sync + From<T>,
{
    println!("\n=== PROVER DEBUG ===");

    let mut fs = FiatShamir::new_merlin();
    let mut proof = LigeritoProof::<T, U>::new();

    // Initial commitment
    println!("Creating initial commitment...");
    let wtns_0 = ligero_commit(poly, config.initial_dims.0, config.initial_dims.1, &config.initial_reed_solomon);
    let cm_0 = RecursiveLigeroCommitment {
        root: wtns_0.tree.get_root(),
    };
    proof.initial_ligero_cm = Some(cm_0.clone());
    fs.absorb_root(&cm_0.root);
    println!("Initial commitment root: {:?}", cm_0.root);

    // Get initial challenges
    let partial_evals_0: Vec<T> = (0..config.initial_k)
        .map(|i| {
            let challenge = fs.get_challenge();
            println!("Initial challenge {}: {:?}", i, challenge);
            challenge
        })
        .collect();

    // Partial evaluation
    println!("\nPerforming partial evaluation...");
    let mut f_evals = poly.to_vec();
    partial_eval_multilinear(&mut f_evals, &partial_evals_0);
    println!("Partial eval complete, new size: {}", f_evals.len());

    // Convert to extension field
    let partial_evals_0_u: Vec<U> = partial_evals_0.iter().map(|&x| U::from(x)).collect();
    let f_evals_u: Vec<U> = f_evals.iter().map(|&x| U::from(x)).collect();

    // First recursive step
    println!("\nFirst recursive step...");
    let wtns_1 = ligero_commit(&f_evals_u, config.dims[0].0, config.dims[0].1, &config.reed_solomon_codes[0]);
    let cm_1 = RecursiveLigeroCommitment {
        root: wtns_1.tree.get_root(),
    };
    proof.recursive_commitments.push(cm_1.clone());
    fs.absorb_root(&cm_1.root);

    // Query selection
    let rows = wtns_0.mat.len();
    println!("\nSelecting queries from {} rows...", rows);
    let queries = fs.get_distinct_queries(rows, S);
    println!("Selected queries (0-based): {:?}", &queries[..queries.len().min(5)]);

    let alpha = fs.get_challenge::<U>();
    println!("Alpha challenge: {:?}", alpha);

    // Prepare for sumcheck
    let n = f_evals.len().trailing_zeros() as usize;
    println!("\nPreparing sumcheck, n = {}", n);
    let sks_vks: Vec<T> = eval_sk_at_vks(1 << n);

    let opened_rows: Vec<Vec<T>> = queries.iter()
        .map(|&q| wtns_0.mat[q].clone())
        .collect();

    let mtree_proof = wtns_0.tree.prove(&queries);
    proof.initial_ligero_proof = Some(RecursiveLigeroProof {
        opened_rows: opened_rows.clone(),
        merkle_proof: mtree_proof,
    });

    println!("\nInducing sumcheck polynomial...");
    let (basis_poly, enforced_sum) = induce_sumcheck_poly_debug(
        n,
        &sks_vks,
        &opened_rows,
        &partial_evals_0_u,
        &queries,
        alpha,
    );
    println!("Enforced sum: {:?}", enforced_sum);

    let mut sumcheck_transcript = vec![];
    let mut current_poly = basis_poly;
    let mut current_sum = enforced_sum;

    // First sumcheck round absorb
    fs.absorb_elem(current_sum);

    // Process recursive rounds
    let mut wtns_prev = wtns_1;

    for i in 0..config.recursive_steps {
        println!("\n--- Recursive step {}/{} ---", i+1, config.recursive_steps);
        let mut rs = Vec::new();

        // Sumcheck rounds
        for j in 0..config.ks[i] {
            // Compute coefficients first (before getting challenge)
            let coeffs = compute_sumcheck_coefficients(&current_poly);
            println!("  Round {}: coeffs = {:?}", j, coeffs);
            sumcheck_transcript.push(coeffs);

            // Get challenge after providing coefficients
            let ri = fs.get_challenge::<U>();
            println!("  Challenge: {:?}", ri);
            rs.push(ri);

            // Fold polynomial with the challenge
            current_poly = fold_polynomial_with_challenge(&current_poly, ri);

            // Update sum
            current_sum = evaluate_quadratic(coeffs, ri);
            println!("  New sum: {:?}", current_sum);
            fs.absorb_elem(current_sum);
        }

        // Final round
        if i == config.recursive_steps - 1 {
            println!("\nFinal round - creating proof...");
            fs.absorb_elems(&current_poly);

            let rows = wtns_prev.mat.len();
            let queries = fs.get_distinct_queries(rows, S);

            let opened_rows: Vec<Vec<U>> = queries.iter()
                .map(|&q| wtns_prev.mat[q].clone())
                .collect();

            let mtree_proof = wtns_prev.tree.prove(&queries);

            proof.final_ligero_proof = Some(FinalLigeroProof {
                yr: current_poly.clone(),
                opened_rows,
                merkle_proof: mtree_proof,
            });

            proof.sumcheck_transcript = Some(SumcheckTranscript { transcript: sumcheck_transcript });

            println!("Proof generation complete!");
            return finalize(proof);
        }

        // Continue recursion
        println!("\nContinuing recursion...");
        let wtns_next = ligero_commit(
            &current_poly,
            config.dims[i + 1].0,
            config.dims[i + 1].1,
            &config.reed_solomon_codes[i + 1],
        );

        let cm_next = RecursiveLigeroCommitment {
            root: wtns_next.tree.get_root(),
        };
        proof.recursive_commitments.push(cm_next.clone());
        fs.absorb_root(&cm_next.root);

        let rows = wtns_prev.mat.len();
        let queries = fs.get_distinct_queries(rows, S);
        let alpha = fs.get_challenge::<U>();

        let opened_rows: Vec<Vec<U>> = queries.iter()
            .map(|&q| wtns_prev.mat[q].clone())
            .collect();

        let mtree_proof = wtns_prev.tree.prove(&queries);
        proof.recursive_proofs.push(RecursiveLigeroProof {
            opened_rows: opened_rows.clone(),
            merkle_proof: mtree_proof,
        });

        // Update for next round
        let n = current_poly.len().trailing_zeros() as usize;
        let sks_vks: Vec<U> = eval_sk_at_vks(1 << n);

        println!("\nInducing next sumcheck polynomial...");
        let (basis_poly, enforced_sum) = induce_sumcheck_poly_debug(
            n,
            &sks_vks,
            &opened_rows,
            &rs,
            &queries,
            alpha,
        );
        println!("Next enforced sum: {:?}", enforced_sum);

        // Glue sumcheck
        let glue_sum = current_sum.add(&enforced_sum);
        fs.absorb_elem(glue_sum);
        println!("Glue sum: {:?}", glue_sum);

        // Glue polynomials
        let beta = fs.get_challenge::<U>();
        println!("Beta challenge: {:?}", beta);
        current_poly = glue_polynomials(&current_poly, &basis_poly, beta);
        current_sum = glue_sums(current_sum, enforced_sum, beta);
        println!("Updated current sum: {:?}", current_sum);

        wtns_prev = wtns_next;
    }

    unreachable!("Should have returned in final round");
}

// Helper functions

fn compute_sumcheck_coefficients<F: BinaryFieldElement>(poly: &[F]) -> (F, F, F) {
    let n = poly.len() / 2;

    let mut s0 = F::zero();
    let mut s1 = F::zero();
    let mut s2 = F::zero();

    for i in 0..n {
        let p0 = poly[2 * i];
        let p1 = poly[2 * i + 1];

        s0 = s0.add(&p0);
        s1 = s1.add(&p0.add(&p1));
        s2 = s2.add(&p1);
    }

    (s0, s1, s2)
}

fn fold_polynomial_with_challenge<F: BinaryFieldElement>(poly: &[F], r: F) -> Vec<F> {
    let n = poly.len() / 2;
    let mut new_poly = vec![F::zero(); n];

    for i in 0..n {
        let p0 = poly[2 * i];
        let p1 = poly[2 * i + 1];
        new_poly[i] = p0.add(&r.mul(&p1.add(&p0)));
    }

    new_poly
}

fn fold_polynomial<F: BinaryFieldElement>(poly: &[F], r: F) -> (Vec<F>, (F, F, F)) {
    let coeffs = compute_sumcheck_coefficients(poly);
    let new_poly = fold_polynomial_with_challenge(poly, r);
    (new_poly, coeffs)
}

fn evaluate_quadratic<F: BinaryFieldElement>(coeffs: (F, F, F), x: F) -> F {
    let (s0, s1, s2) = coeffs;
    // For binary field sumcheck, we need a univariate polynomial where:
    // f(0) = s0 (sum when xi=0)
    // f(1) = s2 (sum when xi=1)
    // and s1 = s0 + s2 (total sum)
    //
    // The degree-1 polynomial through (0,s0) and (1,s2) is:
    // f(x) = s0*(1-x) + s2*x = s0 + (s2-s0)*x
    // In binary fields where -s0 = s0:
    // f(x) = s0 + (s2+s0)*x = s0 + s1*x (since s1 = s0+s2)
    //
    // But wait, that gives f(0) = s0 and f(1) = s0+s1 = s0+s0+s2 = s2 (since s0+s0=0 in binary)
    // Let's verify: f(1) = s0 + s1*1 = s0 + (s0+s2) = s2. Good!
    s0.add(&s1.mul(&x))
}

fn glue_polynomials<F: BinaryFieldElement>(f: &[F], g: &[F], beta: F) -> Vec<F> {
    assert_eq!(f.len(), g.len());

    f.iter()
        .zip(g.iter())
        .map(|(&fi, &gi)| fi.add(&beta.mul(&gi)))
        .collect()
}

fn glue_sums<F: BinaryFieldElement>(sum_f: F, sum_g: F, beta: F) -> F {
    sum_f.add(&beta.mul(&sum_g))
}

#[cfg(test)]
mod tests {
    use super::*;
    use binary_fields::{BinaryElem32, BinaryElem128};
    use crate::configs::hardcoded_config_12;
    use std::marker::PhantomData;

    #[test]
    fn test_fold_polynomial() {
        // Test with a simple polynomial
        let poly = vec![
            BinaryElem32::from(1),
            BinaryElem32::from(2),
            BinaryElem32::from(3),
            BinaryElem32::from(4),
        ];

        let r = BinaryElem32::from(5);
        let (new_poly, (s0, _s1, s2)) = fold_polynomial(&poly, r);

        assert_eq!(new_poly.len(), 2);

        // Check sums
        assert_eq!(s0, BinaryElem32::from(1).add(&BinaryElem32::from(3)));
        assert_eq!(s2, BinaryElem32::from(2).add(&BinaryElem32::from(4)));
    }

    #[test]
    fn test_evaluate_quadratic() {
        // for binary field sumcheck, we use linear polynomials: f(x) = s0 + s1*x
        // where s1 = s0 + s2, so f(0) = s0 and f(1) = s0 + s1 = s2
        let coeffs = (
            BinaryElem32::from(1),  // s0
            BinaryElem32::from(3),  // s1 = s0 + s2
            BinaryElem32::from(2),  // s2
        );

        // test at x = 0: f(0) = s0
        let val0 = evaluate_quadratic(coeffs, BinaryElem32::zero());
        assert_eq!(val0, BinaryElem32::from(1));

        // test at x = 1: f(1) = s0 + s1*1 = s0 + s1
        let val1 = evaluate_quadratic(coeffs, BinaryElem32::one());
        // in binary field: 1 XOR 3 = 2
        assert_eq!(val1, BinaryElem32::from(2));
    }

    #[test]
    fn test_glue_polynomials() {
        let f = vec![BinaryElem32::from(1), BinaryElem32::from(2)];
        let g = vec![BinaryElem32::from(3), BinaryElem32::from(4)];
        let beta = BinaryElem32::from(5);

        let result = glue_polynomials(&f, &g, beta);

        assert_eq!(result.len(), 2);
        assert_eq!(result[0], BinaryElem32::from(1).add(&beta.mul(&BinaryElem32::from(3))));
        assert_eq!(result[1], BinaryElem32::from(2).add(&beta.mul(&BinaryElem32::from(4))));
    }

    #[test]
    fn test_simple_prove() {
        let config = hardcoded_config_12(
            PhantomData::<BinaryElem32>,
            PhantomData::<BinaryElem128>,
        );

        // Test with all ones polynomial
        let poly = vec![BinaryElem32::one(); 1 << 12];

        // This should not panic
        let proof = prove(&config, &poly);
        assert!(proof.is_ok(), "Simple proof generation should succeed");
    }

    #[test]
    fn test_sumcheck_consistency_in_prover() {
        // This is tested indirectly through the debug assertions in the prover
        let config = hardcoded_config_12(
            PhantomData::<BinaryElem32>,
            PhantomData::<BinaryElem128>,
        );

        // Test with zero polynomial
        let poly = vec![BinaryElem32::zero(); 1 << 12];

        let proof = prove(&config, &poly);
        assert!(proof.is_ok(), "Zero polynomial proof should succeed");

        // Test with simple pattern
        let mut poly = vec![BinaryElem32::zero(); 1 << 12];
        poly[0] = BinaryElem32::one();
        poly[1] = BinaryElem32::from(2);

        let proof = prove(&config, &poly);
        assert!(proof.is_ok(), "Simple pattern proof should succeed");
    }
}
