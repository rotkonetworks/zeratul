use binary_fields::BinaryFieldElement;
use crate::utils::{evaluate_lagrange_basis, evaluate_scaled_basis_inplace};
use rayon::prelude::*;

/// Precompute alpha powers for efficiency
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

/// FIXED: Induce sumcheck polynomial with proper sum consistency
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
    println!("\n=== DEBUG induce_sumcheck_poly ===");
    println!("n: {}, sks_vks.len: {}, opened_rows.len: {}, v_challenges.len: {}",
             n, sks_vks.len(), opened_rows.len(), v_challenges.len());
    println!("First few queries: {:?}", &sorted_queries[..sorted_queries.len().min(5)]);

    // Check if alpha is zero - this is problematic!
    if alpha == U::zero() {
        println!("WARNING: Alpha is zero! This will cause issues!");
    }

    let gr = evaluate_lagrange_basis(v_challenges);
    println!("Lagrange basis gr.len: {}", gr.len());
    println!("First few gr values: {:?}", &gr[..gr.len().min(4)]);

    // Check if Lagrange basis is all zeros
    if gr.iter().all(|&x| x == U::zero()) {
        println!("WARNING: Lagrange basis is all zeros!");
    }

    let mut basis_poly = vec![U::zero(); 1 << n];
    let mut enforced_sum = U::zero();

    let alpha_pows = precompute_alpha_powers(alpha, opened_rows.len());
    println!("Alpha: {:?}", alpha);
    println!("First few alpha powers: {:?}", &alpha_pows[..alpha_pows.len().min(4)]);

    for (i, (row, &query)) in opened_rows.iter().zip(sorted_queries.iter()).enumerate() {
        if i < 3 {  // Debug first few iterations
            println!("\nProcessing query {} (index {})", query, i);
            println!("Row length: {}", row.len());
            if row.len() > 0 {
                println!("First few row values: {:?}", &row[..row.len().min(4)]);
            }
        }

        // Compute dot product
        let dot = row.iter()
            .zip(gr.iter())
            .fold(U::zero(), |acc, (&r, &g)| {
                let r_u = U::from(r);
                acc.add(&r_u.mul(&g))
            });

        if i < 3 {
            println!("Dot product: {:?}", dot);
        }

        let alpha_pow = alpha_pows[i];
        let contribution = dot.mul(&alpha_pow);
        enforced_sum = enforced_sum.add(&contribution);

        if i < 3 {
            println!("Alpha power: {:?}", alpha_pow);
            println!("Contribution to sum: {:?}", contribution);
            println!("Running enforced_sum: {:?}", enforced_sum);
        }

        // Create field element from query index (0-based)
        let qf = T::from_bits(query as u64);
        if i < 3 {
            println!("Query {} as field element: {:?}", query, qf);
        }

        let mut local_sks_x = vec![T::zero(); sks_vks.len()];
        let mut local_basis = vec![U::zero(); 1 << n];

        // FIXED: Use proper multilinear extension of delta function
        evaluate_scaled_basis_inplace(&mut local_sks_x, &mut local_basis, sks_vks, qf, alpha_pow);

        if i < 3 {
            println!("First few basis values after evaluation: {:?}",
                     &local_basis[..local_basis.len().min(4)]);
            
            // Debug: check if basis is properly constructed
            let basis_sum = local_basis.iter().fold(U::zero(), |acc, &x| acc.add(&x));
            println!("Local basis sum: {:?} (should equal alpha_pow: {:?})", basis_sum, alpha_pow);
        }

        // Add to basis polynomial
        for (j, &val) in local_basis.iter().enumerate() {
            basis_poly[j] = basis_poly[j].add(&val);
        }
    }

    println!("\nFinal enforced_sum: {:?}", enforced_sum);
    println!("First few basis_poly values: {:?}", &basis_poly[..basis_poly.len().min(4)]);

    // Check if basis polynomial is all zeros
    if basis_poly.iter().all(|&x| x == U::zero()) {
        println!("WARNING: Basis polynomial is all zeros!");
    }

    // Compute and print the sum of basis polynomial
    let basis_sum = basis_poly.iter().fold(U::zero(), |acc, &x| acc.add(&x));
    println!("Sum of basis polynomial: {:?}", basis_sum);

    // CRITICAL FIX: Check consistency
    if basis_sum != enforced_sum {
        println!("ERROR: Sum inconsistency detected!");
        println!("  Enforced sum: {:?}", enforced_sum);
        println!("  Basis sum: {:?}", basis_sum);
        
        // This should not happen with the fixed implementation
        panic!("Sumcheck consistency check failed - this indicates a bug in the implementation");
    } else {
        println!("✓ Sum consistency check passed");
    }

    println!("=== END DEBUG ===\n");

    (basis_poly, enforced_sum)
}

/// Production version without debug output
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
        // Compute dot product
        let dot = row.iter()
            .zip(gr.iter())
            .fold(U::zero(), |acc, (&r, &g)| {
                let r_u = U::from(r);
                acc.add(&r_u.mul(&g))
            });

        let alpha_pow = alpha_pows[i];
        let contribution = dot.mul(&alpha_pow);
        enforced_sum = enforced_sum.add(&contribution);

        // Create field element from query index (0-based)
        let qf = T::from_bits(query as u64);

        let mut local_sks_x = vec![T::zero(); sks_vks.len()];
        let mut local_basis = vec![U::zero(); 1 << n];

        evaluate_scaled_basis_inplace(&mut local_sks_x, &mut local_basis, sks_vks, qf, alpha_pow);

        // Add to basis polynomial
        for (j, &val) in local_basis.iter().enumerate() {
            basis_poly[j] = basis_poly[j].add(&val);
        }
    }

    // Consistency check (can be disabled in production for performance)
    debug_assert_eq!(
        basis_poly.iter().fold(U::zero(), |acc, &x| acc.add(&x)),
        enforced_sum,
        "Sumcheck consistency check failed"
    );

    (basis_poly, enforced_sum)
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

    // Check if we have the expected dimensions
    if !opened_rows.is_empty() {
        assert_eq!(opened_rows[0].len(), gr.len(),
                   "Row length {} doesn't match Lagrange basis length {}",
                   opened_rows[0].len(), gr.len());
    }
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
                let qf = T::from_bits(query as u64);

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

    // Consistency check
    debug_assert_eq!(
        basis_poly.iter().fold(U::zero(), |acc, &x| acc.add(&x)),
        enforced_sum,
        "Parallel sumcheck consistency check failed"
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
    fn test_sumcheck_parallel_consistency() {
        let n = 2; // 2^2 = 4 elements  
        let sks_vks: Vec<BinaryElem32> = eval_sk_at_vks(1 << n);

        let v_challenges = vec![
            BinaryElem128::from(0xABCD),
        ];

        let queries = vec![0, 1, 3];
        let opened_rows = vec![
            vec![BinaryElem32::from(7)],
            vec![BinaryElem32::from(11)],
            vec![BinaryElem32::from(13)],
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
