use binary_fields::BinaryFieldElement;
use crate::utils::{evaluate_lagrange_basis, evaluate_scaled_basis_inplace};
use rayon::prelude::*;

/// tensorized dot product exploiting kronecker structure
/// reduces o(2^k) to o(k × 2^(k-1)) by folding dimensions
/// iterates challenges in reverse since lagrange basis maps r0 to lsb
fn tensorized_dot_product<T, U>(row: &[T], challenges: &[U]) -> U
where
    T: BinaryFieldElement,
    U: BinaryFieldElement + From<T>,
{
    let k = challenges.len();
    if k == 0 {
        return if row.len() == 1 {
            U::from(row[0])
        } else {
            U::zero()
        };
    }

    assert_eq!(row.len(), 1 << k, "Row length must be 2^k");

    let mut current: Vec<U> = row.iter().map(|&x| U::from(x)).collect();

    // fold from last to first challenge
    for &r in challenges.iter().rev() {
        let half = current.len() / 2;
        let one_minus_r = U::one().add(&r); // in gf(2^n): 1-r = 1+r

        for i in 0..half {
            // lagrange contraction: (1-r)*left + r*right
            current[i] = current[2*i].mul(&one_minus_r)
                        .add(&current[2*i+1].mul(&r));
        }
        current.truncate(half);
    }

    current[0]
}

/// precompute powers of alpha to avoid repeated multiplications
pub fn precompute_alpha_powers<F: BinaryFieldElement>(alpha: F, n: usize) -> Vec<F> {
    let mut alpha_pows = vec![F::zero(); n];
    if n > 0 {
        alpha_pows[0] = F::one();
        for i in 1..n {
            alpha_pows[i] = alpha_pows[i-1].mul(&alpha);
        }
    }
    alpha_pows
}

/// debug version with consistency checks enabled
pub fn induce_sumcheck_poly_debug<T, U>(
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
    let mut basis_poly = vec![U::zero(); 1 << n];
    let mut enforced_sum = U::zero();
    let alpha_pows = precompute_alpha_powers(alpha, opened_rows.len());

    // reuse allocations across iterations
    let mut local_sks_x = vec![T::zero(); sks_vks.len()];
    let mut local_basis = vec![U::zero(); 1 << n];

    for (i, (row, &query)) in opened_rows.iter().zip(sorted_queries.iter()).enumerate() {
        let dot = tensorized_dot_product(row, v_challenges);
        let contribution = dot.mul(&alpha_pows[i]);
        enforced_sum = enforced_sum.add(&contribution);

        let query_mod = query % (1 << n);
        let qf = T::from_bits(query_mod as u64);

        // clear and reuse buffers
        local_sks_x.fill(T::zero());
        local_basis.fill(U::zero());
        evaluate_scaled_basis_inplace(&mut local_sks_x, &mut local_basis, sks_vks, qf, contribution);

        for (j, &val) in local_basis.iter().enumerate() {
            basis_poly[j] = basis_poly[j].add(&val);
        }
    }

    let basis_sum = basis_poly.iter().fold(U::zero(), |acc, &x| acc.add(&x));
    if basis_sum != enforced_sum {
        panic!("sumcheck consistency check failed");
    }

    (basis_poly, enforced_sum)
}

/// production version using direct indexing optimization
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
    let alpha_pows = precompute_alpha_powers(alpha, opened_rows.len());

    let mut basis_poly = vec![U::zero(); 1 << n];
    let mut enforced_sum = U::zero();

    for (i, (row, &query)) in opened_rows.iter().zip(sorted_queries.iter()).enumerate() {
        let dot = row.iter()
            .zip(gr.iter())
            .fold(U::zero(), |acc, (&r, &g)| acc.add(&U::from(r).mul(&g)));

        let contribution = dot.mul(&alpha_pows[i]);
        enforced_sum = enforced_sum.add(&contribution);

        let query_mod = query % (1 << n);
        basis_poly[query_mod] = basis_poly[query_mod].add(&contribution);
    }

    debug_assert_eq!(
        basis_poly.iter().fold(U::zero(), |acc, &x| acc.add(&x)),
        enforced_sum,
        "sumcheck consistency failed"
    );

    (basis_poly, enforced_sum)
}

/// parallel version for prover - parallelize dot products, not basis accumulation
/// only worth it for many queries (500+) - otherwise use sequential
pub fn induce_sumcheck_poly_parallel<T, U>(
    n: usize,
    _sks_vks: &[T],
    opened_rows: &[Vec<T>],
    v_challenges: &[U],
    sorted_queries: &[usize],
    alpha: U,
) -> (Vec<U>, U)
where
    T: BinaryFieldElement + Send + Sync,
    U: BinaryFieldElement + Send + Sync + From<T>,
{
    // precompute lagrange basis once
    let gr = evaluate_lagrange_basis(v_challenges);

    if !opened_rows.is_empty() {
        assert_eq!(opened_rows[0].len(), gr.len());
    }
    assert_eq!(opened_rows.len(), sorted_queries.len());

    // precompute alpha powers once
    let alpha_pows = precompute_alpha_powers(alpha, opened_rows.len());

    // parallelize ONLY the dot product computation, not basis accumulation
    // this avoids allocating huge per-thread basis_poly copies
    let contributions: Vec<(U, usize)> = opened_rows
        .par_iter()
        .zip(sorted_queries.par_iter())
        .zip(alpha_pows.par_iter())
        .map(|((row, &query), &alpha_pow)| {
            // compute dot product in parallel
            let dot = row.iter()
                .zip(gr.iter())
                .fold(U::zero(), |acc, (&r, &g)| acc.add(&U::from(r).mul(&g)));

            let contribution = dot.mul(&alpha_pow);
            let query_mod = query % (1 << n);

            (contribution, query_mod)
        })
        .collect();

    // sequential accumulation into basis_poly (single allocation)
    let mut basis_poly = vec![U::zero(); 1 << n];
    let mut enforced_sum = U::zero();

    for (contribution, query_mod) in contributions {
        basis_poly[query_mod] = basis_poly[query_mod].add(&contribution);
        enforced_sum = enforced_sum.add(&contribution);
    }

    debug_assert_eq!(
        basis_poly.iter().fold(U::zero(), |acc, &x| acc.add(&x)),
        enforced_sum,
        "parallel sumcheck consistency failed"
    );

    (basis_poly, enforced_sum)
}

#[cfg(test)]
mod tests {
    use super::*;
    use binary_fields::{BinaryElem32, BinaryElem128};
    use crate::utils::eval_sk_at_vks;

    #[test]
    fn test_alpha_powers() {
        let alpha = BinaryElem128::from(5);
        let powers = precompute_alpha_powers(alpha, 4);
        
        assert_eq!(powers[0], BinaryElem128::one());
        assert_eq!(powers[1], alpha);
        assert_eq!(powers[2], alpha.mul(&alpha));
        assert_eq!(powers[3], alpha.mul(&alpha).mul(&alpha));
    }

    #[test]
    fn test_sumcheck_consistency() {
        // Test that enforced_sum equals sum of basis polynomial
        let n = 3; // 2^3 = 8 elements
        let sks_vks: Vec<BinaryElem32> = eval_sk_at_vks(1 << n);

        let v_challenges = vec![
            BinaryElem128::from(0x1234),
            BinaryElem128::from(0x5678),
        ];

        let queries = vec![0, 2, 5];
        let opened_rows = vec![
            vec![BinaryElem32::from(1); 4],
            vec![BinaryElem32::from(2); 4],
            vec![BinaryElem32::from(3); 4],
        ];

        let alpha = BinaryElem128::from(0x9ABC);

        let (basis_poly, enforced_sum) = induce_sumcheck_poly(
            n, &sks_vks, &opened_rows, &v_challenges, &queries, alpha
        );

        // Check sum consistency
        let computed_sum = basis_poly.iter().fold(BinaryElem128::zero(), |acc, &x| acc.add(&x));
        assert_eq!(computed_sum, enforced_sum, "Sum consistency check failed");

        // The basis polynomial should not be all zeros (unless all inputs are zero)
        let all_zero = basis_poly.iter().all(|&x| x == BinaryElem128::zero());
        assert!(!all_zero || alpha == BinaryElem128::zero(), "Basis polynomial should not be all zeros");
    }

    #[test]
    fn test_parallel_vs_sequential() {
        // Test that parallel and sequential versions produce identical results
        let n = 12; // 2^12 = 4096 elements
        let sks_vks: Vec<BinaryElem32> = eval_sk_at_vks(1 << n);

        // Create realistic test data
        let num_queries = 148;
        let v_challenges = vec![
            BinaryElem128::from(0x1234567890abcdef),
            BinaryElem128::from(0xfedcba0987654321),
        ];

        let queries: Vec<usize> = (0..num_queries).map(|i| (i * 113) % (1 << n)).collect();
        let opened_rows: Vec<Vec<BinaryElem32>> = (0..num_queries)
            .map(|i| {
                (0..4).map(|j| BinaryElem32::from((i * j + 1) as u32)).collect()
            })
            .collect();

        let alpha = BinaryElem128::from(0x9ABC);

        // Run sequential version
        let (seq_basis, seq_sum) = induce_sumcheck_poly(
            n, &sks_vks, &opened_rows, &v_challenges, &queries, alpha
        );

        // Run parallel version
        let (par_basis, par_sum) = induce_sumcheck_poly_parallel(
            n, &sks_vks, &opened_rows, &v_challenges, &queries, alpha
        );

        // Compare enforced sums
        assert_eq!(par_sum, seq_sum, "Parallel and sequential enforced sums differ");

        // Compare basis polynomials element by element
        for (i, (&par_val, &seq_val)) in par_basis.iter().zip(seq_basis.iter()).enumerate() {
            if par_val != seq_val {
                println!("Mismatch at index {}: parallel={:?}, sequential={:?}", i, par_val, seq_val);
            }
        }

        assert_eq!(par_basis, seq_basis, "Parallel and sequential basis polynomials differ");
    }

    #[test]
    fn test_sumcheck_parallel_consistency() {
        let n = 2; // 2^2 = 4 elements
        let sks_vks: Vec<BinaryElem32> = eval_sk_at_vks(1 << n);

        // 1 challenge -> Lagrange basis length = 2^1 = 2
        let v_challenges = vec![
            BinaryElem128::from(0xABCD),
        ];

        let queries = vec![0, 1, 3];
        // each row must have length 2 to match Lagrange basis
        let opened_rows = vec![
            vec![BinaryElem32::from(7), BinaryElem32::from(9)],
            vec![BinaryElem32::from(11), BinaryElem32::from(13)],
            vec![BinaryElem32::from(15), BinaryElem32::from(17)],
        ];

        let alpha = BinaryElem128::from(0x1337);

        // Sequential version
        let (basis_seq, sum_seq) = induce_sumcheck_poly(
            n, &sks_vks, &opened_rows, &v_challenges, &queries, alpha
        );

        // Parallel version
        let (basis_par, sum_par) = induce_sumcheck_poly_parallel(
            n, &sks_vks, &opened_rows, &v_challenges, &queries, alpha
        );

        // Results should be identical
        assert_eq!(sum_seq, sum_par, "Sequential and parallel sums should match");
        assert_eq!(basis_seq, basis_par, "Sequential and parallel basis polynomials should match");
    }

    #[test]
    fn test_empty_inputs() {
        let n = 2;
        let sks_vks: Vec<BinaryElem32> = eval_sk_at_vks(1 << n);
        let v_challenges = vec![BinaryElem128::from(1)];
        let queries: Vec<usize> = vec![];
        let opened_rows: Vec<Vec<BinaryElem32>> = vec![];
        let alpha = BinaryElem128::from(42);

        let (basis_poly, enforced_sum) = induce_sumcheck_poly(
            n, &sks_vks, &opened_rows, &v_challenges, &queries, alpha
        );

        // With no inputs, everything should be zero
        assert_eq!(enforced_sum, BinaryElem128::zero());
        assert!(basis_poly.iter().all(|&x| x == BinaryElem128::zero()));
    }

    #[test]
    fn test_single_query() {
        let n = 2; // 2^2 = 4 elements
        let sks_vks: Vec<BinaryElem32> = eval_sk_at_vks(1 << n);

        let v_challenges = vec![BinaryElem128::from(5)];
        let queries = vec![2]; // Single query at index 2
        let opened_rows = vec![vec![BinaryElem32::from(7)]]; // Single row with single element
        let alpha = BinaryElem128::from(3);

        let (basis_poly, enforced_sum) = induce_sumcheck_poly(
            n, &sks_vks, &opened_rows, &v_challenges, &queries, alpha
        );

        // Check that basis polynomial has the expected structure
        let basis_sum = basis_poly.iter().fold(BinaryElem128::zero(), |acc, &x| acc.add(&x));
        assert_eq!(basis_sum, enforced_sum);

        // Since we have only one query, the basis should be mostly zero except at the query point
        let non_zero_count = basis_poly.iter().filter(|&&x| x != BinaryElem128::zero()).count();
        assert!(non_zero_count <= 1, "Should have at most one non-zero entry for single query");
    }
}
