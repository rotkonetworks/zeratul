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
    println!("ligero_commit: Converting polynomial to matrix form...");
    let mut poly_mat = poly2mat(poly, m, n, 4);
    
    println!("ligero_commit: Matrix dimensions: {} rows x {} cols", poly_mat.len(), poly_mat[0].len());
    
    println!("ligero_commit: Encoding columns (m={}, n={})...", m, n);
    let start = std::time::Instant::now();
    encode_cols(&mut poly_mat, rs, true);
    println!("ligero_commit: Column encoding took {:?}", start.elapsed());

    // Hash each row to create leaves for merkle tree
    println!("ligero_commit: Hashing {} rows for Merkle tree...", poly_mat.len());
    let hashed_rows: Vec<Hash> = poly_mat.iter()
        .map(|row| hash_row(row))
        .collect();
    
    // Debug: print first few hashes
    println!("First few row hashes:");
    for (i, hash) in hashed_rows.iter().take(3).enumerate() {
        println!("  Row {}: {:?}", i, &hash[..8]);
    }
    
    println!("ligero_commit: Building Merkle tree...");
    println!("  Number of hashed rows (leaves): {}", hashed_rows.len());
    println!("  Is power of 2: {}", hashed_rows.len().is_power_of_two());
    
    let tree = build_merkle_tree(&hashed_rows);
    let tree_depth = tree.get_depth();
    let root = tree.get_root();
    
    println!("ligero_commit: Tree built!");
    println!("  Tree depth: {}", tree_depth);
    println!("  Expected leaves for depth {}: {}", tree_depth, 1 << tree_depth);
    println!("  Root exists: {}", root.root.is_some());
    if let Some(r) = &root.root {
        println!("  Root hash: {:?}", &r[..8]);
    }

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
    println!("=== DEBUG verify_ligero ===");
    println!("verify_ligero: {} queries, {} rows, yr len: {}", 
             queries.len(), opened_rows.len(), yr.len());
    println!("Challenges: {} values", challenges.len());
    
    let gr = evaluate_lagrange_basis(challenges);
    println!("Lagrange basis gr len: {}", gr.len());
    
    let n = yr.len().trailing_zeros() as usize;
    let sks_vks: Vec<T> = eval_sk_at_vks(1 << n);
    println!("sks_vks len: {}, n: {}", sks_vks.len(), n);

    // Check first query serially for debugging
    if !queries.is_empty() {
        let query = queries[0];
        let row = &opened_rows[0];
        
        println!("\nFirst query: {} (1-based)", query);
        println!("Row length: {}", row.len());
        println!("First few row values: {:?}", &row[..4.min(row.len())]);
        
        // Compute dot product
        let dot = row.iter()
            .zip(gr.iter())
            .fold(U::zero(), |acc, (&r, &g)| {
                let r_u = U::from(r);
                acc.add(&r_u.mul(&g))
            });

        println!("Dot product: {:?}", dot);

        let qf = T::from_bits((query - 1) as u64);
        println!("qf (query-1 as field elem): {:?}", qf);

        let mut local_sks_x = vec![T::zero(); sks_vks.len()];
        let mut local_basis = vec![U::zero(); 1 << n];

        let scale = U::one();
        evaluate_scaled_basis_inplace(&mut local_sks_x, &mut local_basis, &sks_vks, qf, scale);
        
        println!("First few basis values: {:?}", &local_basis[..4.min(local_basis.len())]);

        let e = yr.iter()
            .zip(local_basis.iter())
            .fold(U::zero(), |acc, (&y, &b)| {
                let y_u = U::from(y);
                acc.add(&y_u.mul(&b))
            });

        println!("Expected value e: {:?}", e);
        println!("Match: {} (dot == e)", e == dot);
        
        if e != dot {
            println!("MISMATCH! Verification will fail.");
            // Don't panic here, let the assertion below handle it
        }
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
