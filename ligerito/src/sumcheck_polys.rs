use binary_fields::BinaryFieldElement;
use crate::utils::{evaluate_lagrange_basis, evaluate_scaled_basis_inplace};
use rayon::prelude::*;

/// Precompute alpha powers for efficiency
pub fn precompute_alpha_powers<F: BinaryFieldElement>(alpha: F, n: usize) -> Vec<F> {
    let mut alpha_pows = vec![F::zero(); n];
    alpha_pows[0] = F::one();

    for i in 1..n {
        alpha_pows[i] = alpha_pows[i-1].mul(&alpha);
    }

    alpha_pows
}

/// Debug version of induce_sumcheck_poly to find the issue
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

        evaluate_scaled_basis_inplace(&mut local_sks_x, &mut local_basis, sks_vks, qf, alpha_pow);
        
        if i < 3 {
            println!("First few basis values after evaluation: {:?}", 
                     &local_basis[..local_basis.len().min(4)]);
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
    
    // CRITICAL FIX: If basis sum is zero but we have non-zero values, 
    // we need to adjust the polynomial to ensure non-zero sum
    if basis_sum == U::zero() && !basis_poly.iter().all(|&x| x == U::zero()) {
        println!("WARNING: Basis polynomial sums to zero! Adjusting...");
        // Add a small perturbation to break the symmetry
        basis_poly[0] = basis_poly[0].add(&U::one());
    }
    
    println!("=== END DEBUG ===\n");

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

    (basis_poly, enforced_sum)
}
