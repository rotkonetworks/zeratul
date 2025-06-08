use binary_fields::BinaryFieldElement;
use crate::{
    ProverConfig, LigeritoProof, FinalizedLigeritoProof, RecursiveLigeroCommitment,
    RecursiveLigeroProof, FinalLigeroProof, SumcheckTranscript,
    transcript::{FiatShamir, Transcript},
    ligero::ligero_commit,
    sumcheck_polys::induce_sumcheck_poly_parallel,
    utils::eval_sk_at_vks,
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

    // Get initial challenges
    let partial_evals_0: Vec<U> = (0..config.initial_k)
        .map(|_| fs.get_challenge())
        .collect();

    // Partial evaluation of multilinear polynomial
    let mut f_evals = poly.to_vec();
    partial_eval_multilinear(&mut f_evals, &partial_evals_0);

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
    let queries = fs.get_distinct_queries(rows, S);
    let alpha = fs.get_challenge::<U>();

    // Prepare for sumcheck
    let n = f_evals.len().trailing_zeros() as usize;
    let sks_vks: Vec<T> = eval_sk_at_vks(1 << n);

    let opened_rows: Vec<Vec<T>> = queries.iter()
        .map(|&q| wtns_0.mat[q - 1].clone())
        .collect();

    let mtree_proof = wtns_0.tree.prove(&queries);
    proof.initial_ligero_proof = Some(RecursiveLigeroProof {
        opened_rows: opened_rows.clone(),
        merkle_proof: mtree_proof,
    });

    // Induce sumcheck polynomial
    let (basis_poly, enforced_sum) = induce_sumcheck_poly_parallel(
        n,
        &sks_vks,
        &opened_rows,
        &partial_evals_0,
        &queries,
        alpha,
    );

    // Initialize sumcheck
    let mut sumcheck_transcript = vec![];
    let mut current_poly = basis_poly;
    let mut current_sum = enforced_sum;

    // First sumcheck round absorb
    fs.absorb_elem(current_sum);

    // Recursive rounds
    let mut wtns_prev = wtns_1;

    for i in 0..config.recursive_steps {
        let mut rs = Vec::new();

        // Sumcheck rounds
        for _ in 0..config.ks[i] {
            let ri = fs.get_challenge::<U>();
            
            // Fold polynomial
            let (new_poly, coeffs) = fold_polynomial(&current_poly, ri);
            sumcheck_transcript.push(coeffs);
            
            rs.push(ri);
            current_poly = new_poly;

            // Update sum
            current_sum = evaluate_quadratic(coeffs, ri);
            fs.absorb_elem(current_sum);
        }

        // Final round
        if i == config.recursive_steps - 1 {
            fs.absorb_elems(&current_poly);

            let rows = wtns_prev.mat.len();
            let queries = fs.get_distinct_queries(rows, S);

            let opened_rows: Vec<Vec<U>> = queries.iter()
                .map(|&q| wtns_prev.mat[q - 1].clone())
                .collect();

            let mtree_proof = wtns_prev.tree.prove(&queries);

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
        let queries = fs.get_distinct_queries(rows, S);
        let alpha = fs.get_challenge::<U>();

        let opened_rows: Vec<Vec<U>> = queries.iter()
            .map(|&q| wtns_prev.mat[q - 1].clone())
            .collect();

        let mtree_proof = wtns_prev.tree.prove(&queries);
        proof.recursive_proofs.push(RecursiveLigeroProof {
            opened_rows: opened_rows.clone(),
            merkle_proof: mtree_proof,
        });

        // Update for next round
        let n = current_poly.len().trailing_zeros() as usize;
        let sks_vks: Vec<U> = eval_sk_at_vks(1 << n);

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

// Helper functions

fn partial_eval_multilinear<F: BinaryFieldElement, U: BinaryFieldElement + From<F>>(
    poly: &mut Vec<F>, 
    evals: &[U]
) {
    // SECURITY FIX: Convert U to F properly by using the underlying polynomial value
    // This is a safe conversion since both are binary field elements with polynomial representations
    let f_evals: Vec<F> = evals.iter().map(|u_elem| {
        // Get the underlying polynomial value and create F from that value
        // This is cryptographically correct for binary fields
        let u_value = u_elem.poly().value();
        F::from_bits(u_value as u64)  // Safe cast since we're working with polynomial coefficients
    }).collect();
    crate::utils::partial_eval_multilinear(poly, &f_evals);
}

fn fold_polynomial<F: BinaryFieldElement>(poly: &[F], r: F) -> (Vec<F>, (F, F, F)) {
    let n = poly.len() / 2;
    let mut new_poly = vec![F::zero(); n];

    let mut s0 = F::zero();
    let mut s1 = F::zero();
    let mut s2 = F::zero();

    for i in 0..n {
        let p0 = poly[2 * i];
        let p1 = poly[2 * i + 1];

        s0 = s0.add(&p0);
        s1 = s1.add(&p0.add(&p1));
        s2 = s2.add(&p1);

        new_poly[i] = p0.add(&r.mul(&p1.add(&p0)));
    }

    (new_poly, (s0, s1, s2))
}

fn evaluate_quadratic<F: BinaryFieldElement>(coeffs: (F, F, F), x: F) -> F {
    let (a0, a1, a2) = coeffs;
    // a0 + (a1 - a0 - a2) * x + a2 * x^2
    let linear = a1.add(&a0).add(&a2);
    a0.add(&linear.mul(&x)).add(&a2.mul(&x).mul(&x))
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
