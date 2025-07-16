use binary_fields::BinaryFieldElement;
use reed_solomon::ReedSolomon;
use merkle_tree::{build_merkle_tree, Hash};
use crate::data_structures::{RecursiveLigeroWitness};
use crate::utils::{evaluate_lagrange_basis, eval_sk_at_vks, evaluate_scaled_basis_inplace};
use rayon::prelude::*;
use sha2::{Sha256, Digest};

pub fn poly2mat<F: BinaryFieldElement>(
    poly: &[F],
    m: usize,
    n: usize,
    inv_rate: usize,
) -> Vec<Vec<F>> {
    let m_target = m * inv_rate;
    let mut mat = vec![vec![F::zero(); n]; m_target];

    mat.par_iter_mut()
        .enumerate()
        .for_each(|(i, row)| {
            for j in 0..n {
                let idx = j * m + i;
                if idx < poly.len() {
                    row[j] = poly[idx];
                }
            }
        });

    mat
}

pub fn encode_cols<F: BinaryFieldElement + Send + Sync>(
    poly_mat: &mut Vec<Vec<F>>,
    rs: &ReedSolomon<F>,
    parallel: bool,
) {
    let n = poly_mat[0].len();

    if parallel {
        let mut cols: Vec<Vec<F>> = (0..n)
            .into_par_iter()
            .map(|j| {
                poly_mat.iter()
                    .map(|row| row[j])
                    .collect()
            })
            .collect();

        cols.par_iter_mut()
            .for_each(|col| {
                reed_solomon::encode_in_place(rs, col);
            });

        for (i, row) in poly_mat.iter_mut().enumerate() {
            for (j, col) in cols.iter().enumerate() {
                row[j] = col[i];
            }
        }
    } else {
        for j in 0..n {
            let mut col: Vec<F> = poly_mat.iter().map(|row| row[j]).collect();
            reed_solomon::encode_in_place(rs, &mut col);
            for (i, val) in col.iter().enumerate() {
                poly_mat[i][j] = *val;
            }
        }
    }
}

/// Hash a row of field elements with deterministic serialization
#[inline(always)]
pub fn hash_row<F: BinaryFieldElement>(row: &[F]) -> Hash {
    let mut hasher = Sha256::new();
    
    // Hash row length for domain separation
    hasher.update(&(row.len() as u32).to_le_bytes());
    
    // Hash each element using bytemuck for safe serialization
    let elem_size = std::mem::size_of::<F>();
    for elem in row.iter() {
        // Use bytemuck to get raw bytes safely
        let bytes = unsafe {
            std::slice::from_raw_parts(
                elem as *const F as *const u8,
                elem_size
            )
        };
        hasher.update(bytes);
    }
    
    hasher.finalize().into()
}

pub fn ligero_commit<F: BinaryFieldElement + Send + Sync>(
    poly: &[F],
    m: usize,
    n: usize,
    rs: &ReedSolomon<F>,
) -> RecursiveLigeroWitness<F> {
    let mut poly_mat = poly2mat(poly, m, n, 4);
    encode_cols(&mut poly_mat, rs, true);

    let hashed_rows: Vec<Hash> = poly_mat.iter()
        .map(|row| hash_row(row))
        .collect();
    
    let tree = build_merkle_tree(&hashed_rows);

    RecursiveLigeroWitness { mat: poly_mat, tree }
}

pub fn verify_ligero<T, U>(
    queries: &[usize],
    opened_rows: &[Vec<T>],
    yr: &[T],
    challenges: &[U],
) where
    T: BinaryFieldElement + Send + Sync,
    U: BinaryFieldElement + Send + Sync + From<T>,
{
    let gr = evaluate_lagrange_basis(challenges);
    let n = yr.len().trailing_zeros() as usize;
    let sks_vks: Vec<T> = eval_sk_at_vks(1 << n);

    queries.par_iter()
        .zip(opened_rows.par_iter())
        .for_each(|(&query, row)| {
            let dot = row.iter()
                .zip(gr.iter())
                .fold(U::zero(), |acc, (&r, &g)| {
                    let r_u = U::from(r);
                    acc.add(&r_u.mul(&g))
                });

            let qf = T::from_bits(query as u64);

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
