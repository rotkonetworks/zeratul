use binary_fields::BinaryFieldElement;
use reed_solomon::ReedSolomon;
use merkle_tree::{build_merkle_tree, Hash};
use crate::data_structures::{RecursiveLigeroWitness};
use crate::utils::{evaluate_lagrange_basis, eval_sk_at_vks, evaluate_scaled_basis_inplace};
use rayon::prelude::*;
use sha2::{Sha256, Digest};

/// Convert polynomial to matrix form
pub fn poly2mat<F: BinaryFieldElement>(
    poly: &[F],
    m: usize,
    n: usize,
    inv_rate: usize,
) -> Vec<Vec<F>> {
    let m_target = m * inv_rate;
    let mut mat = vec![vec![F::zero(); n]; m_target];

    // Parallel filling
    mat.par_iter_mut()
        .enumerate()
        .for_each(|(i, row)| {
            for j in 0..n {
                if j * m + i < poly.len() {
                    row[j] = poly[j * m + i];
                }
            }
        });

    mat
}

/// Encode columns in parallel
pub fn encode_cols<F: BinaryFieldElement + Send + Sync>(
    poly_mat: &mut Vec<Vec<F>>,
    rs: &ReedSolomon<F>,
    parallel: bool,
) {
    let n = poly_mat[0].len();

    if parallel {
        // Transpose for column-wise access
        let mut cols: Vec<Vec<F>> = (0..n)
            .into_par_iter()
            .map(|j| {
                poly_mat.iter()
                    .map(|row| row[j])
                    .collect()
            })
            .collect();

        // Encode each column
        cols.par_iter_mut()
            .for_each(|col| {
                reed_solomon::encode_in_place(rs, col);
            });

        // Transpose back
        for (i, row) in poly_mat.iter_mut().enumerate() {
            for (j, col) in cols.iter().enumerate() {
                row[j] = col[i];
            }
        }
    } else {
        // Sequential version
        for j in 0..n {
            let mut col: Vec<F> = poly_mat.iter().map(|row| row[j]).collect();
            reed_solomon::encode_in_place(rs, &mut col);
            for (i, val) in col.iter().enumerate() {
                poly_mat[i][j] = *val;
            }
        }
    }
}

/// Hash a row of field elements
fn hash_row<F: BinaryFieldElement>(row: &[F]) -> Hash {
    let mut hasher = Sha256::new();
    
    for elem in row {
        let elem_bytes = unsafe {
            std::slice::from_raw_parts(
                elem as *const F as *const u8,
                std::mem::size_of::<F>()
            )
        };
        hasher.update(elem_bytes);
    }
    
    hasher.finalize().into()
}

/// Commit to polynomial using Ligero
pub fn ligero_commit<F: BinaryFieldElement + Send + Sync>(
    poly: &[F],
    m: usize,
    n: usize,
    rs: &ReedSolomon<F>,
) -> RecursiveLigeroWitness<F> {
    let mut poly_mat = poly2mat(poly, m, n, 4);
    encode_cols(&mut poly_mat, rs, true);

    // Hash each row to create leaves for merkle tree
    let hashed_rows: Vec<Hash> = poly_mat.iter()
        .map(|row| hash_row(row))
        .collect();
    
    let tree = build_merkle_tree(&hashed_rows);

    RecursiveLigeroWitness { mat: poly_mat, tree }
}

/// Verify Ligero opening
pub fn verify_ligero<T, U>(
    queries: &[usize],
    opened_rows: &[Vec<T>],
    yr: &[T],
    challenges: &[U],
) where
    T: BinaryFieldElement + Send + Sync,
    U: BinaryFieldElement + Send + Sync + From<T>,
{
    println!("verify_ligero: {} queries, {} rows, yr len: {}", 
             queries.len(), opened_rows.len(), yr.len());
    
    let gr = evaluate_lagrange_basis(challenges);
    let n = yr.len().trailing_zeros() as usize;
    let sks_vks: Vec<T> = eval_sk_at_vks(1 << n);

    // Check first query serially for debugging
    if !queries.is_empty() {
        let query = queries[0];
        let row = &opened_rows[0];
        
        println!("First query: {}", query);
        
        // Compute dot product
        let dot = row.iter()
            .zip(gr.iter())
            .fold(U::zero(), |acc, (&r, &g)| {
                let r_u = U::from(r);
                acc.add(&r_u.mul(&g))
            });

        println!("Dot product: {:?}", dot);

        let qf = T::from_bits((query - 1) as u64);
        println!("qf: {:?}", qf);

        let mut local_sks_x = vec![T::zero(); sks_vks.len()];
        let mut local_basis = vec![U::zero(); 1 << n];

        let scale = U::one();
        evaluate_scaled_basis_inplace(&mut local_sks_x, &mut local_basis, &sks_vks, qf, scale);

        let e = yr.iter()
            .zip(local_basis.iter())
            .fold(U::zero(), |acc, (&y, &b)| {
                let y_u = U::from(y);
                acc.add(&y_u.mul(&b))
            });

        println!("Expected: {:?}", e);
        println!("Match: {}", e == dot);
    }

    // Parallel verification
    queries.par_iter()
        .zip(opened_rows.par_iter())
        .for_each(|(&query, row)| {
            // Compute dot product
            let dot = row.iter()
                .zip(gr.iter())
                .fold(U::zero(), |acc, (&r, &g)| {
                    let r_u = U::from(r);
                    acc.add(&r_u.mul(&g))
                });

            // Create field element from query index (0-based in storage, but queries are 1-based)
            let qf = T::from_bits((query - 1) as u64);

            let mut local_sks_x = vec![T::zero(); sks_vks.len()];
            let mut local_basis = vec![U::zero(); 1 << n];

            let scale = U::one();
            evaluate_scaled_basis_inplace(&mut local_sks_x, &mut local_basis, &sks_vks, qf, scale);

            let e = yr.iter()
                .zip(local_basis.iter())
                .fold(U::zero(), |acc, (&y, &b)| {
                    let y_u = U::from(y);
                    acc.add(&y_u.mul(&b))
                });

            assert_eq!(e, dot, "Verification failed at query {}", query);
        });
}
