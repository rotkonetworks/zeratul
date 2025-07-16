use binary_fields::BinaryFieldElement;
use crate::utils::{evaluate_lagrange_basis, evaluate_scaled_basis_inplace};
use rayon::prelude::*;

/// Precompute alpha powers for efficiency
fn precompute_alpha_powers<F: BinaryFieldElement>(alpha: F, n: usize) -> Vec<F> {
    let mut alpha_pows = vec![F::zero(); n];
    alpha_pows[0] = F::one();

    for i in 1..n {
        alpha_pows[i] = alpha_pows[i-1].mul(&alpha);
    }

    alpha_pows
}

/// Induce sumcheck polynomial in parallel
pub fn induce_sumcheck_poly_parallel<T, U>(
    n: usize,
    sks_vks: &[T],
    opened_rows: &[Vec<T>],
    v_challenges: &[U],
    sorted_queries: &[usize],
    alpha: U,
) -> (Vec<U>, U)
where
    T: BinaryFieldElement + Send + Sync,
    U: BinaryFieldElement + Send + Sync + From<T>,
{
    let gr = evaluate_lagrange_basis(v_challenges);
    assert!(opened_rows.iter().all(|row| row.len() == gr.len()));
    assert_eq!(opened_rows.len(), sorted_queries.len());

    let n_threads = rayon::current_num_threads();
    let n_rows = opened_rows.len();
    let chunk_size = (n_rows + n_threads - 1) / n_threads;

    let alpha_pows = precompute_alpha_powers(alpha, n_rows);

    // Parallel computation
    let results: Vec<(Vec<U>, U)> = (0..n_threads)
        .into_par_iter()
        .map(|t| {
            let mut local_basis = vec![U::zero(); 1 << n];
            let mut local_sum = U::zero();
            let mut local_sks_x = vec![T::zero(); sks_vks.len()];

            let start_idx = t * chunk_size;
            let end_idx = ((t + 1) * chunk_size).min(n_rows);

            for i in start_idx..end_idx {
                let row = &opened_rows[i];
                let query = sorted_queries[i];

                // Compute dot product
                let dot = row.iter()
                    .zip(gr.iter())
                    .fold(U::zero(), |acc, (&r, &g)| {
                        let r_u = U::from(r);
                        acc.add(&r_u.mul(&g))
                    });

                let alpha_pow = alpha_pows[i];
                local_sum = local_sum.add(&dot.mul(&alpha_pow));

                // Create field element from query index (0-based)
                let mut qf = T::zero();
                let mut v = query as u64;
                let mut power = T::one();
                
                // Build field element bit by bit
                for _ in 0..64 {
                    if v & 1 == 1 {
                        qf = qf.add(&power);
                    }
                    power = power.add(&power);  // Double
                    v >>= 1;
                    if v == 0 { break; }
                }
                
                let mut temp_basis = vec![U::zero(); 1 << n];
                evaluate_scaled_basis_inplace(&mut local_sks_x, &mut temp_basis, sks_vks, qf, alpha_pow);

                // Add to local basis
                for j in 0..(1 << n) {
                    local_basis[j] = local_basis[j].add(&temp_basis[j]);
                }
            }

            (local_basis, local_sum)
        })
        .collect();

    // Combine results
    let mut basis_poly = vec![U::zero(); 1 << n];
    let mut enforced_sum = U::zero();

    for (partial_basis, partial_sum) in results {
        for (i, &val) in partial_basis.iter().enumerate() {
            basis_poly[i] = basis_poly[i].add(&val);
        }
        enforced_sum = enforced_sum.add(&partial_sum);
    }

    (basis_poly, enforced_sum)
}

/// Non-parallel version for smaller inputs
pub fn induce_sumcheck_poly<T, U>(
    n: usize,
    sks_vks: &[T],
    opened_rows: &[Vec<T>],
    v_challenges: &[U],
    sorted_queries: &[usize],
    alpha: U,
) -> (Vec<U>, U)
where
    T: BinaryFieldElement,
    U: BinaryFieldElement + From<T>,
{
    let gr = evaluate_lagrange_basis(v_challenges);
    let mut basis_poly = vec![U::zero(); 1 << n];
    let mut enforced_sum = U::zero();

    let alpha_pows = precompute_alpha_powers(alpha, opened_rows.len());

    for (i, (row, &query)) in opened_rows.iter().zip(sorted_queries.iter()).enumerate() {
        // Compute dot product
        let dot = row.iter()
            .zip(gr.iter())
            .fold(U::zero(), |acc, (&r, &g)| {
                let r_u = U::from(r);
                acc.add(&r_u.mul(&g))
            });

        let alpha_pow = alpha_pows[i];
        enforced_sum = enforced_sum.add(&dot.mul(&alpha_pow));

        // Create field element from query index (0-based)
        let mut qf = T::zero();
        let mut v = query as u64;
        let mut power = T::one();
        
        // Build field element bit by bit
        for _ in 0..64 {
            if v & 1 == 1 {
                qf = qf.add(&power);
            }
            power = power.add(&power);  // Double
            v >>= 1;
            if v == 0 { break; }
        }
        
        let mut local_sks_x = vec![T::zero(); sks_vks.len()];
        let mut local_basis = vec![U::zero(); 1 << n];

        evaluate_scaled_basis_inplace(&mut local_sks_x, &mut local_basis, sks_vks, qf, alpha_pow);

        // Add to basis polynomial
        for (j, &val) in local_basis.iter().enumerate() {
            basis_poly[j] = basis_poly[j].add(&val);
        }
    }

    (basis_poly, enforced_sum)
}
