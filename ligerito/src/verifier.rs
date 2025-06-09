use binary_fields::{BinaryFieldElement, BinaryPolynomial};
use crate::utils::partial_eval_multilinear;
use crate::{
    VerifierConfig, FinalizedLigeritoProof,
    transcript::{FiatShamir, Transcript},
    ligero::verify_ligero,
    sumcheck_polys::induce_sumcheck_poly_parallel,
    utils::eval_sk_at_vks,
};
use merkle_tree::{self, Hash};
use sha2::{Sha256, Digest};

const S: usize = 148;
const LOG_INV_RATE: usize = 2;

/// Hash a row of field elements  
fn hash_row<F: BinaryFieldElement>(row: &[F]) -> Hash {
    let mut hasher = Sha256::new();
    
    // SECURITY FIX: Replace unsafe memory access with proper field element serialization
    for (i, elem) in row.iter().enumerate() {
        // Hash the element index first for position-dependence
        hasher.update(&(i as u32).to_le_bytes());
        
        // Serialize the field element in a safe, deterministic way
        // Use the field element's Debug trait which is guaranteed to be implemented
        let elem_bytes = format!("{:?}", elem).into_bytes();
        hasher.update(&elem_bytes);
    }
    
    hasher.finalize().into()
}

/// Main verifier function
pub fn verify<T, U>(
    config: &VerifierConfig,
    proof: &FinalizedLigeritoProof<T, U>,
) -> crate::Result<bool>
where
    T: BinaryFieldElement + Send + Sync,
    U: BinaryFieldElement + Send + Sync + From<T>,
{
    // Initialize Fiat-Shamir
    let mut fs = FiatShamir::new_merlin();

    // Absorb initial commitment
    fs.absorb_root(&proof.initial_ligero_cm.root);

    // Get initial challenges
    let partial_evals_0: Vec<U> = (0..config.initial_k)
        .map(|_| fs.get_challenge())
        .collect();

    // First recursive commitment
    fs.absorb_root(&proof.recursive_commitments[0].root);

    // Verify initial proof
    let depth = config.initial_dim + LOG_INV_RATE;
    let queries = fs.get_distinct_queries(1 << depth, S);

    // Hash the opened rows for verification
    let hashed_leaves: Vec<Hash> = proof.initial_ligero_proof.opened_rows.iter()
        .map(|row| hash_row(row))
        .collect();

    let res = merkle_tree::verify(
        &proof.initial_ligero_cm.root,
        &proof.initial_ligero_proof.merkle_proof,
        depth,
        &hashed_leaves,
        &queries,
    );

    if !res {
        return Ok(false);
    }

    let alpha = fs.get_challenge::<U>();

    // Induce sumcheck polynomial
    let sks_vks: Vec<T> = eval_sk_at_vks(1 << config.initial_dim);
    let (_, enforced_sum) = induce_sumcheck_poly_parallel(
        config.initial_dim,
        &sks_vks,
        &proof.initial_ligero_proof.opened_rows,
        &partial_evals_0,
        &queries,
        alpha,
    );

    // Verify sumcheck
    let mut current_sum = enforced_sum;
    let mut transcript_idx = 0;
    
    // Initial absorb
    fs.absorb_elem(current_sum);

    for i in 0..config.recursive_steps {
        let mut rs = Vec::new();

        // Sumcheck rounds
        for _ in 0..config.ks[i] {
            // Verify claimed sum
            let coeffs = proof.sumcheck_transcript.transcript[transcript_idx];
            let claimed_sum = evaluate_quadratic(coeffs, U::zero()).add(&evaluate_quadratic(coeffs, U::one()));

            if claimed_sum != current_sum {
                return Ok(false);
            }

            let ri = fs.get_challenge::<U>();
            rs.push(ri);
            current_sum = evaluate_quadratic(coeffs, ri);
            fs.absorb_elem(current_sum);

            transcript_idx += 1;
        }

        let root = &proof.recursive_commitments[i].root;

        // Final round
        if i == config.recursive_steps - 1 {
            fs.absorb_elems(&proof.final_ligero_proof.yr);

            let depth = config.log_dims[i] + LOG_INV_RATE;
            let queries = fs.get_distinct_queries(1 << depth, S);

            // Hash final opened rows
            let hashed_final: Vec<Hash> = proof.final_ligero_proof.opened_rows.iter()
                .map(|row| hash_row(row))
                .collect();

            let res = merkle_tree::verify(
                root,
                &proof.final_ligero_proof.merkle_proof,
                depth,
                &hashed_final,
                &queries,
            );

            if !res {
                return Ok(false);
            }

            // Verify Ligero consistency
            verify_ligero(&queries, &proof.final_ligero_proof.opened_rows, &proof.final_ligero_proof.yr, &rs);

            // Final sumcheck verification
            let final_r = fs.get_challenge::<U>();
            let mut f_eval = proof.final_ligero_proof.yr.clone();
            partial_eval_multilinear(&mut f_eval, &[final_r]);

            let claimed_eval = f_eval[0];
            // SECURITY FIX: Add missing final sumcheck verification
            // The final evaluation must match the current sum for verification to succeed
            if claimed_eval != current_sum {
                return Ok(false); // Verification failed - invalid proof
            }
            
            return Ok(true); // Verification successful
        }

        // Continue recursion
        fs.absorb_root(&proof.recursive_commitments[i + 1].root);

        let depth = config.log_dims[i] + LOG_INV_RATE;
        let ligero_proof = &proof.recursive_proofs[i];
        let queries = fs.get_distinct_queries(1 << depth, S);

        // Hash recursive opened rows
        let hashed_rec: Vec<Hash> = ligero_proof.opened_rows.iter()
            .map(|row| hash_row(row))
            .collect();

        let res = merkle_tree::verify(
            root,
            &ligero_proof.merkle_proof,
            depth,
            &hashed_rec,
            &queries,
        );

        if !res {
            return Ok(false);
        }

        let alpha = fs.get_challenge::<U>();

        // Induce next polynomial
        let sks_vks: Vec<U> = eval_sk_at_vks(1 << config.log_dims[i]);
        let (_, enforced_sum) = induce_sumcheck_poly_parallel(
            config.log_dims[i],
            &sks_vks,
            &ligero_proof.opened_rows,
            &rs,
            &queries,
            alpha,
        );

        // Glue verification
        let glue_sum = current_sum.add(&enforced_sum);
        fs.absorb_elem(glue_sum);
        
        let beta = fs.get_challenge::<U>();
        current_sum = glue_sums(current_sum, enforced_sum, beta);
    }

    Ok(true)
}

/// Main verifier function with configurable transcript
pub fn verify_with_transcript<T, U>(
    config: &VerifierConfig,
    proof: &FinalizedLigeritoProof<T, U>,
    mut fs: impl Transcript,
) -> crate::Result<bool>
where
    T: BinaryFieldElement + Send + Sync,
    U: BinaryFieldElement + Send + Sync + From<T>,
{
    // Absorb initial commitment
    fs.absorb_root(&proof.initial_ligero_cm.root);

    // Get initial challenges
    let partial_evals_0: Vec<U> = (0..config.initial_k)
        .map(|_| fs.get_challenge())
        .collect();

    // First recursive commitment
    fs.absorb_root(&proof.recursive_commitments[0].root);

    // Verify initial proof
    let depth = config.initial_dim + LOG_INV_RATE;
    let queries = fs.get_distinct_queries(1 << depth, S);

    // Hash the opened rows for verification
    let hashed_leaves: Vec<Hash> = proof.initial_ligero_proof.opened_rows.iter()
        .map(|row| hash_row(row))
        .collect();

    let res = merkle_tree::verify(
        &proof.initial_ligero_cm.root,
        &proof.initial_ligero_proof.merkle_proof,
        depth,
        &hashed_leaves,
        &queries,
    );

    if !res {
        return Ok(false);
    }

    let alpha = fs.get_challenge::<U>();

    // Induce sumcheck polynomial
    let sks_vks: Vec<T> = eval_sk_at_vks(1 << config.initial_dim);
    let (_, enforced_sum) = induce_sumcheck_poly_parallel(
        config.initial_dim,
        &sks_vks,
        &proof.initial_ligero_proof.opened_rows,
        &partial_evals_0,
        &queries,
        alpha,
    );

    // Verify sumcheck
    let mut current_sum = enforced_sum;
    let mut transcript_idx = 0;
    
    // Initial absorb
    fs.absorb_elem(current_sum);

    for i in 0..config.recursive_steps {
        let mut rs = Vec::new();

        // Sumcheck rounds
        for _ in 0..config.ks[i] {
            // Verify claimed sum
            let coeffs = proof.sumcheck_transcript.transcript[transcript_idx];
            let claimed_sum = evaluate_quadratic(coeffs, U::zero()).add(&evaluate_quadratic(coeffs, U::one()));

            if claimed_sum != current_sum {
                return Ok(false);
            }

            let ri = fs.get_challenge::<U>();
            rs.push(ri);
            current_sum = evaluate_quadratic(coeffs, ri);
            fs.absorb_elem(current_sum);

            transcript_idx += 1;
        }

        let root = &proof.recursive_commitments[i].root;

        // Final round
        if i == config.recursive_steps - 1 {
            fs.absorb_elems(&proof.final_ligero_proof.yr);

            let depth = config.log_dims[i] + LOG_INV_RATE;
            let queries = fs.get_distinct_queries(1 << depth, S);

            // Hash final opened rows
            let hashed_final: Vec<Hash> = proof.final_ligero_proof.opened_rows.iter()
                .map(|row| hash_row(row))
                .collect();

            let res = merkle_tree::verify(
                root,
                &proof.final_ligero_proof.merkle_proof,
                depth,
                &hashed_final,
                &queries,
            );

            if !res {
                return Ok(false);
            }

            // Verify Ligero consistency
            verify_ligero(&queries, &proof.final_ligero_proof.opened_rows, &proof.final_ligero_proof.yr, &rs);

            // Final sumcheck verification
            let final_r = fs.get_challenge::<U>();
            let mut f_eval = proof.final_ligero_proof.yr.clone();
            partial_eval_multilinear(&mut f_eval, &[final_r]);

            let claimed_eval = f_eval[0];
            // SECURITY FIX: Add missing final sumcheck verification
            // The final evaluation must match the current sum for verification to succeed
            if claimed_eval != current_sum {
                return Ok(false); // Verification failed - invalid proof
            }
            
            return Ok(true); // Verification successful
        }

        // Continue recursion
        fs.absorb_root(&proof.recursive_commitments[i + 1].root);

        let depth = config.log_dims[i] + LOG_INV_RATE;
        let ligero_proof = &proof.recursive_proofs[i];
        let queries = fs.get_distinct_queries(1 << depth, S);

        // Hash recursive opened rows
        let hashed_rec: Vec<Hash> = ligero_proof.opened_rows.iter()
            .map(|row| hash_row(row))
            .collect();

        let res = merkle_tree::verify(
            root,
            &ligero_proof.merkle_proof,
            depth,
            &hashed_rec,
            &queries,
        );

        if !res {
            return Ok(false);
        }

        let alpha = fs.get_challenge::<U>();

        // Induce next polynomial
        let sks_vks: Vec<U> = eval_sk_at_vks(1 << config.log_dims[i]);
        let (_, enforced_sum) = induce_sumcheck_poly_parallel(
            config.log_dims[i],
            &sks_vks,
            &ligero_proof.opened_rows,
            &rs,
            &queries,
            alpha,
        );

        // Glue verification
        let glue_sum = current_sum.add(&enforced_sum);
        fs.absorb_elem(glue_sum);
        
        let beta = fs.get_challenge::<U>();
        current_sum = glue_sums(current_sum, enforced_sum, beta);
    }

    Ok(true)
}

/// Verifier function using Julia-compatible SHA256 transcript
pub fn verify_sha256<T, U>(
    config: &VerifierConfig,
    proof: &FinalizedLigeritoProof<T, U>,
) -> crate::Result<bool>
where
    T: BinaryFieldElement + Send + Sync,
    U: BinaryFieldElement + Send + Sync + From<T>,
{
    let fs = FiatShamir::new_sha256(1234);
    verify_with_transcript(config, proof, fs)
}

// Helper functions

fn evaluate_quadratic<F: BinaryFieldElement>(coeffs: (F, F, F), x: F) -> F {
    let (a0, a1, a2) = coeffs;
    let linear = a1.add(&a0).add(&a2);
    a0.add(&linear.mul(&x)).add(&a2.mul(&x).mul(&x))
}


fn glue_sums<F: BinaryFieldElement>(sum_f: F, sum_g: F, beta: F) -> F {
    sum_f.add(&beta.mul(&sum_g))
}
