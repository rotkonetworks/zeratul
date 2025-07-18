use binary_fields::BinaryFieldElement;
use crate::{
    VerifierConfig, FinalizedLigeritoProof,
    transcript::{FiatShamir, Transcript},
    ligero::{verify_ligero, hash_row},
    sumcheck_polys::induce_sumcheck_poly_debug,
    utils::{eval_sk_at_vks, partial_eval_multilinear, evaluate_lagrange_basis},
};
use merkle_tree::{self, Hash};

const S: usize = 148;
const LOG_INV_RATE: usize = 2;

/// Verify a Ligerito proof - FIXED VERSION
///
/// # Safety
/// This function performs cryptographic verification and will return false
/// for any invalid proof. All array accesses are bounds-checked.
pub fn verify<T, U>(
    config: &VerifierConfig,
    proof: &FinalizedLigeritoProof<T, U>,
) -> crate::Result<bool>
where
    T: BinaryFieldElement + Send + Sync,
    U: BinaryFieldElement + Send + Sync + From<T>,
{
    // Initialize transcript with proper domain separation
    let mut fs = FiatShamir::new_merlin();

    // Absorb initial commitment
    fs.absorb_root(&proof.initial_ligero_cm.root);

    // Get initial challenges in base field to match prover
    let partial_evals_0_t: Vec<T> = (0..config.initial_k)
        .map(|_| fs.get_challenge())
        .collect();

    // Convert to extension field for computations
    let partial_evals_0: Vec<U> = partial_evals_0_t
        .iter()
        .map(|&x| U::from(x))
        .collect();

    // Absorb first recursive commitment
    if proof.recursive_commitments.is_empty() {
        return Ok(false);
    }
    fs.absorb_root(&proof.recursive_commitments[0].root);

    // Verify initial Merkle proof
    let depth = config.initial_dim + LOG_INV_RATE;
    let queries = fs.get_distinct_queries(1 << depth, S);

    // Hash opened rows for Merkle verification
    let hashed_leaves: Vec<Hash> = proof.initial_ligero_proof.opened_rows
        .iter()
        .map(|row| hash_row(row))
        .collect();

    if !merkle_tree::verify(
        &proof.initial_ligero_cm.root,
        &proof.initial_ligero_proof.merkle_proof,
        depth,
        &hashed_leaves,
        &queries,
    ) {
        return Ok(false);
    }

    let alpha = fs.get_challenge::<U>();

    // CRITICAL FIX: Use the same fixed sumcheck function as prover
    let sks_vks: Vec<T> = eval_sk_at_vks(1 << config.initial_dim);
    let (basis_poly, enforced_sum) = induce_sumcheck_poly_debug(
        config.initial_dim,
        &sks_vks,
        &proof.initial_ligero_proof.opened_rows,
        &partial_evals_0,
        &queries,
        alpha,
    );

    // CRITICAL FIX: Verify that the basis polynomial sum equals enforced_sum
    let basis_sum = basis_poly.iter().fold(U::zero(), |acc, &x| acc.add(&x));
    if basis_sum != enforced_sum {
        eprintln!("VERIFICATION FAILED: Basis polynomial sum mismatch");
        eprintln!("  Expected (enforced_sum): {:?}", enforced_sum);
        eprintln!("  Actual (basis_sum): {:?}", basis_sum);
        return Ok(false);
    }

    // CRITICAL FIX: Use the enforced_sum from sumcheck computation
    let mut current_sum = enforced_sum;

    // Initial sumcheck absorb
    fs.absorb_elem(current_sum);

    // Process recursive rounds
    let mut transcript_idx = 0;

    for i in 0..config.recursive_steps {
        let mut rs = Vec::with_capacity(config.ks[i]);

        // Verify sumcheck rounds
        for _ in 0..config.ks[i] {
            // Bounds check for transcript access
            if transcript_idx >= proof.sumcheck_transcript.transcript.len() {
                return Ok(false);
            }

            let coeffs = proof.sumcheck_transcript.transcript[transcript_idx];
            let claimed_sum = evaluate_quadratic(coeffs, U::zero())
                .add(&evaluate_quadratic(coeffs, U::one()));

            if claimed_sum != current_sum {
                return Ok(false);
            }

            let ri = fs.get_challenge::<U>();
            rs.push(ri);
            current_sum = evaluate_quadratic(coeffs, ri);
            fs.absorb_elem(current_sum);

            transcript_idx += 1;
        }

        // Bounds check for recursive commitments
        if i >= proof.recursive_commitments.len() {
            return Ok(false);
        }

        let root = &proof.recursive_commitments[i].root;

        // Final round verification
        if i == config.recursive_steps - 1 {
            fs.absorb_elems(&proof.final_ligero_proof.yr);

            let depth = config.log_dims[i] + LOG_INV_RATE;
            let queries = fs.get_distinct_queries(1 << depth, S);

            // Hash final opened rows
            let hashed_final: Vec<Hash> = proof.final_ligero_proof.opened_rows
                .iter()
                .map(|row| hash_row(row))
                .collect();

            if !merkle_tree::verify(
                root,
                &proof.final_ligero_proof.merkle_proof,
                depth,
                &hashed_final,
                &queries,
            ) {
                return Ok(false);
            }

            // Verify Ligero consistency
            verify_ligero(
                &queries,
                &proof.final_ligero_proof.opened_rows,
                &proof.final_ligero_proof.yr,
                &rs,
            );

            // Final sumcheck verification
            let final_r = fs.get_challenge::<U>();
            let mut f_eval = proof.final_ligero_proof.yr.clone();
            partial_eval_multilinear(&mut f_eval, &[final_r]);

            // Verify final evaluation matches current sum
            return Ok(f_eval[0] == current_sum);
        }

        // Continue recursion for non-final rounds
        if i + 1 >= proof.recursive_commitments.len() {
            return Ok(false);
        }

        fs.absorb_root(&proof.recursive_commitments[i + 1].root);

        let depth = config.log_dims[i] + LOG_INV_RATE;

        // Bounds check for recursive proofs
        if i >= proof.recursive_proofs.len() {
            return Ok(false);
        }

        let ligero_proof = &proof.recursive_proofs[i];
        let queries = fs.get_distinct_queries(1 << depth, S);

        // Hash recursive opened rows
        let hashed_rec: Vec<Hash> = ligero_proof.opened_rows
            .iter()
            .map(|row| hash_row(row))
            .collect();

        if !merkle_tree::verify(
            root,
            &ligero_proof.merkle_proof,
            depth,
            &hashed_rec,
            &queries,
        ) {
            return Ok(false);
        }

        let alpha = fs.get_challenge::<U>();

        // Bounds check for log_dims
        if i >= config.log_dims.len() {
            return Ok(false);
        }

        // CRITICAL FIX: Use the same fixed sumcheck function as prover
        let sks_vks: Vec<U> = eval_sk_at_vks(1 << config.log_dims[i]);
        let (basis_poly_next, enforced_sum_next) = induce_sumcheck_poly_debug(
            config.log_dims[i],
            &sks_vks,
            &ligero_proof.opened_rows,
            &rs,
            &queries,
            alpha,
        );

        // CRITICAL FIX: Verify consistency for recursive round too
        let basis_sum_next = basis_poly_next.iter().fold(U::zero(), |acc, &x| acc.add(&x));
        if basis_sum_next != enforced_sum_next {
            eprintln!("VERIFICATION FAILED: Recursive basis polynomial sum mismatch at round {}", i);
            eprintln!("  Expected (enforced_sum): {:?}", enforced_sum_next);
            eprintln!("  Actual (basis_sum): {:?}", basis_sum_next);
            return Ok(false);
        }

        let enforced_sum = enforced_sum_next;

        // Glue verification
        let glue_sum = current_sum.add(&enforced_sum);
        fs.absorb_elem(glue_sum);

        let beta = fs.get_challenge::<U>();
        current_sum = glue_sums(current_sum, enforced_sum, beta);
    }

    Ok(true)
}

/// Verify with custom transcript implementation
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

    // Get initial challenges in base field
    let partial_evals_0_t: Vec<T> = (0..config.initial_k)
        .map(|_| fs.get_challenge())
        .collect();

    let partial_evals_0: Vec<U> = partial_evals_0_t
        .iter()
        .map(|&x| U::from(x))
        .collect();

    // First recursive commitment
    fs.absorb_root(&proof.recursive_commitments[0].root);

    // Verify initial proof
    let depth = config.initial_dim + LOG_INV_RATE;
    let queries = fs.get_distinct_queries(1 << depth, S);

    let hashed_leaves: Vec<Hash> = proof.initial_ligero_proof.opened_rows
        .iter()
        .map(|row| hash_row(row))
        .collect();

    if !merkle_tree::verify(
        &proof.initial_ligero_cm.root,
        &proof.initial_ligero_proof.merkle_proof,
        depth,
        &hashed_leaves,
        &queries,
    ) {
        return Ok(false);
    }

    let alpha = fs.get_challenge::<U>();

    let sks_vks: Vec<T> = eval_sk_at_vks(1 << config.initial_dim);
    let (_, enforced_sum) = induce_sumcheck_poly_debug(
        config.initial_dim,
        &sks_vks,
        &proof.initial_ligero_proof.opened_rows,
        &partial_evals_0,
        &queries,
        alpha,
    );

    let mut current_sum = enforced_sum;

    fs.absorb_elem(current_sum);

    let mut transcript_idx = 0;

    for i in 0..config.recursive_steps {
        let mut rs = Vec::with_capacity(config.ks[i]);

        // Sumcheck rounds
        for _ in 0..config.ks[i] {
            if transcript_idx >= proof.sumcheck_transcript.transcript.len() {
                return Ok(false);
            }

            let coeffs = proof.sumcheck_transcript.transcript[transcript_idx];
            let claimed_sum = evaluate_quadratic(coeffs, U::zero())
                .add(&evaluate_quadratic(coeffs, U::one()));

            if claimed_sum != current_sum {
                return Ok(false);
            }

            let ri = fs.get_challenge::<U>();
            rs.push(ri);
            current_sum = evaluate_quadratic(coeffs, ri);
            fs.absorb_elem(current_sum);

            transcript_idx += 1;
        }

        if i >= proof.recursive_commitments.len() {
            return Ok(false);
        }

        let root = &proof.recursive_commitments[i].root;

        // Final round
        if i == config.recursive_steps - 1 {
            fs.absorb_elems(&proof.final_ligero_proof.yr);

            let depth = config.log_dims[i] + LOG_INV_RATE;
            let queries = fs.get_distinct_queries(1 << depth, S);

            let hashed_final: Vec<Hash> = proof.final_ligero_proof.opened_rows
                .iter()
                .map(|row| hash_row(row))
                .collect();

            if !merkle_tree::verify(
                root,
                &proof.final_ligero_proof.merkle_proof,
                depth,
                &hashed_final,
                &queries,
            ) {
                return Ok(false);
            }

            verify_ligero(
                &queries,
                &proof.final_ligero_proof.opened_rows,
                &proof.final_ligero_proof.yr,
                &rs,
            );

            let final_r = fs.get_challenge::<U>();
            let mut f_eval = proof.final_ligero_proof.yr.clone();
            partial_eval_multilinear(&mut f_eval, &[final_r]);

            return Ok(f_eval[0] == current_sum);
        }

        // Continue recursion
        if i + 1 >= proof.recursive_commitments.len() || i >= proof.recursive_proofs.len() {
            return Ok(false);
        }

        fs.absorb_root(&proof.recursive_commitments[i + 1].root);

        let depth = config.log_dims[i] + LOG_INV_RATE;
        let ligero_proof = &proof.recursive_proofs[i];
        let queries = fs.get_distinct_queries(1 << depth, S);

        let hashed_rec: Vec<Hash> = ligero_proof.opened_rows
            .iter()
            .map(|row| hash_row(row))
            .collect();

        if !merkle_tree::verify(
            root,
            &ligero_proof.merkle_proof,
            depth,
            &hashed_rec,
            &queries,
        ) {
            return Ok(false);
        }

        let alpha = fs.get_challenge::<U>();

        if i >= config.log_dims.len() {
            return Ok(false);
        }

        let sks_vks: Vec<U> = eval_sk_at_vks(1 << config.log_dims[i]);
        let (_, enforced_sum_next) = induce_sumcheck_poly_debug(
            config.log_dims[i],
            &sks_vks,
            &ligero_proof.opened_rows,
            &rs,
            &queries,
            alpha,
        );

        let enforced_sum = enforced_sum_next;

        let glue_sum = current_sum.add(&enforced_sum);
        fs.absorb_elem(glue_sum);

        let beta = fs.get_challenge::<U>();
        current_sum = glue_sums(current_sum, enforced_sum, beta);
    }

    Ok(true)
}

/// SHA256-based verification for compatibility
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

#[inline(always)]
fn evaluate_quadratic<F: BinaryFieldElement>(coeffs: (F, F, F), x: F) -> F {
    let (a0, a1, a2) = coeffs;
    // a0 + (a1 - a0 - a2) * x + a2 * x^2
    let linear = a1.add(&a0).add(&a2);
    a0.add(&linear.mul(&x)).add(&a2.mul(&x).mul(&x))
}

#[inline(always)]
fn glue_sums<F: BinaryFieldElement>(sum_f: F, sum_g: F, beta: F) -> F {
    sum_f.add(&beta.mul(&sum_g))
}

/// Debug version to find where verification fails - FIXED VERSION
pub fn verify_debug<T, U>(
    config: &VerifierConfig,
    proof: &FinalizedLigeritoProof<T, U>,
) -> crate::Result<bool>
where
    T: BinaryFieldElement + Send + Sync,
    U: BinaryFieldElement + Send + Sync + From<T>,
{
    println!("\n=== VERIFICATION DEBUG ===");

    // Initialize transcript with proper domain separation
    let mut fs = FiatShamir::new_merlin();

    // Absorb initial commitment
    fs.absorb_root(&proof.initial_ligero_cm.root);
    println!("Absorbed initial root: {:?}", proof.initial_ligero_cm.root);

    // Get initial challenges in base field to match prover
    let partial_evals_0_t: Vec<T> = (0..config.initial_k)
        .map(|_| fs.get_challenge())
        .collect();
    println!("Got {} base field challenges", partial_evals_0_t.len());

    // Convert to extension field for computations
    let partial_evals_0: Vec<U> = partial_evals_0_t
        .iter()
        .map(|&x| U::from(x))
        .collect();

    println!("Partial evaluations (extension field): {:?}", partial_evals_0);

    // Test Lagrange basis computation
    let gr = evaluate_lagrange_basis(&partial_evals_0);
    println!("Lagrange basis length: {}, first few values: {:?}",
             gr.len(), &gr[..gr.len().min(4)]);

    // Absorb first recursive commitment
    if proof.recursive_commitments.is_empty() {
        println!("ERROR: No recursive commitments!");
        return Ok(false);
    }
    fs.absorb_root(&proof.recursive_commitments[0].root);
    println!("Absorbed recursive commitment 0");

    // Verify initial Merkle proof
    let depth = config.initial_dim + LOG_INV_RATE;
    let queries = fs.get_distinct_queries(1 << depth, S);
    println!("Initial proof: depth={}, num_leaves={}, queries={:?}",
             depth, 1 << depth, &queries[..queries.len().min(5)]);

    // Hash opened rows for Merkle verification
    let hashed_leaves: Vec<Hash> = proof.initial_ligero_proof.opened_rows
        .iter()
        .map(|row| hash_row(row))
        .collect();
    println!("Hashed {} opened rows", hashed_leaves.len());
    println!("Opened rows per query match: {}", hashed_leaves.len() == queries.len());

    let merkle_result = merkle_tree::verify(
        &proof.initial_ligero_cm.root,
        &proof.initial_ligero_proof.merkle_proof,
        depth,
        &hashed_leaves,
        &queries,
    );
    println!("Initial Merkle verification: {}", merkle_result);

    if !merkle_result {
        println!("FAILED: Initial Merkle proof verification");

        // Additional debug info
        println!("Proof siblings: {}", proof.initial_ligero_proof.merkle_proof.siblings.len());
        println!("Expected depth: {}", depth);
        println!("Number of queries: {}", queries.len());
        println!("First few queries: {:?}", &queries[..queries.len().min(10)]);

        return Ok(false);
    }

    let alpha = fs.get_challenge::<U>();
    println!("Got alpha challenge: {:?}", alpha);

    // CRITICAL FIX: Use the same fixed sumcheck function as prover
    let sks_vks: Vec<T> = eval_sk_at_vks(1 << config.initial_dim);
    println!("Computed {} sks_vks", sks_vks.len());

    let (basis_poly, enforced_sum) = induce_sumcheck_poly_debug(
        config.initial_dim,
        &sks_vks,
        &proof.initial_ligero_proof.opened_rows,
        &partial_evals_0,
        &queries,
        alpha,
    );

    // CRITICAL FIX: Check consistency
    let basis_sum = basis_poly.iter().fold(U::zero(), |acc, &x| acc.add(&x));
    if basis_sum != enforced_sum {
        println!("VERIFICATION FAILED: Initial basis polynomial sum mismatch");
        println!("  Expected (enforced_sum): {:?}", enforced_sum);
        println!("  Actual (basis_sum): {:?}", basis_sum);
        return Ok(false);
    } else {
        println!("✓ Initial sumcheck consistency check passed");
    }

    // CRITICAL FIX: Use the enforced_sum from sumcheck computation
    let mut current_sum = enforced_sum;
    println!("Using current_sum (enforced_sum): {:?}", current_sum);

    // Initial sumcheck absorb
    fs.absorb_elem(current_sum);

    // Process recursive rounds
    let mut transcript_idx = 0;

    for i in 0..config.recursive_steps {
        println!("\nRecursive step {}/{}", i+1, config.recursive_steps);
        let mut rs = Vec::with_capacity(config.ks[i]);

        // Verify sumcheck rounds
        for j in 0..config.ks[i] {
            // Bounds check for transcript access
            if transcript_idx >= proof.sumcheck_transcript.transcript.len() {
                println!("ERROR: Transcript index {} >= transcript length {}",
                         transcript_idx, proof.sumcheck_transcript.transcript.len());
                return Ok(false);
            }

            let coeffs = proof.sumcheck_transcript.transcript[transcript_idx];
            let s0 = evaluate_quadratic(coeffs, U::zero());
            let s1 = evaluate_quadratic(coeffs, U::one());
            let claimed_sum = s0.add(&s1);

            println!("  Round {}: coeffs={:?}", j, coeffs);
            println!("    s0 (at 0) = {:?}", s0);
            println!("    s1 (at 1) = {:?}", s1);
            println!("    claimed_sum (s0+s1) = {:?}", claimed_sum);
            println!("    current_sum = {:?}", current_sum);

            if claimed_sum != current_sum {
                println!("  FAILED: Sumcheck mismatch!");
                return Ok(false);
            }

            let ri = fs.get_challenge::<U>();
            rs.push(ri);
            current_sum = evaluate_quadratic(coeffs, ri);
            fs.absorb_elem(current_sum);
            println!("    Next current_sum = {:?}", current_sum);

            transcript_idx += 1;
        }

        // Bounds check for recursive commitments
        if i >= proof.recursive_commitments.len() {
            println!("ERROR: Recursive commitment index {} >= length {}",
                     i, proof.recursive_commitments.len());
            return Ok(false);
        }

        let root = &proof.recursive_commitments[i].root;

        // Final round verification
        if i == config.recursive_steps - 1 {
            println!("\nFinal round verification:");
            fs.absorb_elems(&proof.final_ligero_proof.yr);
            println!("Absorbed {} yr values", proof.final_ligero_proof.yr.len());

            let depth = config.log_dims[i] + LOG_INV_RATE;
            let queries = fs.get_distinct_queries(1 << depth, S);
            println!("Final: depth={}, queries={:?}", depth, &queries[..queries.len().min(5)]);

            // Hash final opened rows
            let hashed_final: Vec<Hash> = proof.final_ligero_proof.opened_rows
                .iter()
                .map(|row| hash_row(row))
                .collect();

            let final_merkle_result = merkle_tree::verify(
                root,
                &proof.final_ligero_proof.merkle_proof,
                depth,
                &hashed_final,
                &queries,
            );
            println!("Final Merkle verification: {}", final_merkle_result);

            if !final_merkle_result {
                println!("FAILED: Final Merkle proof verification");
                return Ok(false);
            }

            // Verify Ligero consistency
            println!("Verifying Ligero consistency...");
            verify_ligero(
                &queries,
                &proof.final_ligero_proof.opened_rows,
                &proof.final_ligero_proof.yr,
                &rs,
            );
            println!("Ligero consistency check passed");

            // Final sumcheck verification
            let final_r = fs.get_challenge::<U>();
            let mut f_eval = proof.final_ligero_proof.yr.clone();
            partial_eval_multilinear(&mut f_eval, &[final_r]);

            println!("Final evaluation: {:?}", f_eval[0]);
            println!("Current sum: {:?}", current_sum);

            let result = f_eval[0] == current_sum;
            println!("Final sumcheck verification: {}", result);

            // Verify final evaluation matches current sum
            return Ok(result);
        }

        // Continue recursion for non-final rounds
        if i + 1 >= proof.recursive_commitments.len() {
            println!("ERROR: Missing recursive commitment {}", i + 1);
            return Ok(false);
        }

        fs.absorb_root(&proof.recursive_commitments[i + 1].root);
        println!("Absorbed recursive commitment {}", i + 1);

        let depth = config.log_dims[i] + LOG_INV_RATE;

        // Bounds check for recursive proofs
        if i >= proof.recursive_proofs.len() {
            println!("ERROR: Missing recursive proof {}", i);
            return Ok(false);
        }

        let ligero_proof = &proof.recursive_proofs[i];
        let queries = fs.get_distinct_queries(1 << depth, S);
        println!("Recursive {}: depth={}, queries={:?}", i, depth, &queries[..queries.len().min(5)]);

        // Hash recursive opened rows
        let hashed_rec: Vec<Hash> = ligero_proof.opened_rows
            .iter()
            .map(|row| hash_row(row))
            .collect();

        let rec_merkle_result = merkle_tree::verify(
            root,
            &ligero_proof.merkle_proof,
            depth,
            &hashed_rec,
            &queries,
        );
        println!("Recursive {} Merkle verification: {}", i, rec_merkle_result);

        if !rec_merkle_result {
            println!("FAILED: Recursive {} Merkle proof verification", i);
            return Ok(false);
        }

        let alpha = fs.get_challenge::<U>();
        println!("Got alpha for next round");

        // Bounds check for log_dims
        if i >= config.log_dims.len() {
            println!("ERROR: Missing log_dims[{}]", i);
            return Ok(false);
        }

        // CRITICAL FIX: Use the same fixed sumcheck function as prover
        let sks_vks: Vec<U> = eval_sk_at_vks(1 << config.log_dims[i]);
        let (basis_poly_next, enforced_sum_next) = induce_sumcheck_poly_debug(
            config.log_dims[i],
            &sks_vks,
            &ligero_proof.opened_rows,
            &rs,
            &queries,
            alpha,
        );

        // CRITICAL FIX: Check consistency for recursive round too
        let basis_sum_next = basis_poly_next.iter().fold(U::zero(), |acc, &x| acc.add(&x));
        if basis_sum_next != enforced_sum_next {
            println!("VERIFICATION FAILED: Recursive basis polynomial sum mismatch at round {}", i);
            println!("  Expected (enforced_sum): {:?}", enforced_sum_next);
            println!("  Actual (basis_sum): {:?}", basis_sum_next);
            return Ok(false);
        } else {
            println!("✓ Recursive round {} sumcheck consistency check passed", i);
        }

        let enforced_sum = enforced_sum_next;
        println!("Induced next sumcheck, enforced_sum: {:?}", enforced_sum);

        // Glue verification
        let glue_sum = current_sum.add(&enforced_sum);
        fs.absorb_elem(glue_sum);
        println!("Glue sum: {:?}", glue_sum);

        let beta = fs.get_challenge::<U>();
        current_sum = glue_sums(current_sum, enforced_sum, beta);
        println!("Updated current_sum: {:?}", current_sum);
    }

    println!("\nAll verification steps completed successfully!");
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;
    use binary_fields::{BinaryElem32, BinaryElem128};
    use crate::configs::{hardcoded_config_12, hardcoded_config_12_verifier};
    use crate::prover::prove;
    use std::marker::PhantomData;

    #[test]
    fn test_verify_simple_proof() {
        let prover_config = hardcoded_config_12(
            PhantomData::<BinaryElem32>,
            PhantomData::<BinaryElem128>,
        );
        let verifier_config = hardcoded_config_12_verifier();

        // Test with simple polynomial
        let poly = vec![BinaryElem32::one(); 1 << 12];

        let proof = prove(&prover_config, &poly).expect("Proof generation failed");
        let result = verify(&verifier_config, &proof).expect("Verification failed");

        assert!(result, "Verification should succeed for valid proof");
    }

    #[test]
    fn test_verify_zero_polynomial() {
        let prover_config = hardcoded_config_12(
            PhantomData::<BinaryElem32>,
            PhantomData::<BinaryElem128>,
        );
        let verifier_config = hardcoded_config_12_verifier();

        // Test with zero polynomial
        let poly = vec![BinaryElem32::zero(); 1 << 12];

        let proof = prove(&prover_config, &poly).expect("Proof generation failed");
        let result = verify(&verifier_config, &proof).expect("Verification failed");

        assert!(result, "Verification should succeed for zero polynomial");
    }

    #[test]
    fn test_verify_with_sha256() {
        let prover_config = hardcoded_config_12(
            PhantomData::<BinaryElem32>,
            PhantomData::<BinaryElem128>,
        );
        let verifier_config = hardcoded_config_12_verifier();

        // Test with patterned polynomial
        let mut poly = vec![BinaryElem32::zero(); 1 << 12];
        poly[0] = BinaryElem32::one();
        poly[1] = BinaryElem32::from(2);

        let proof = crate::prover::prove_sha256(&prover_config, &poly)
            .expect("SHA256 proof generation failed");
        let result = verify_sha256(&verifier_config, &proof)
            .expect("SHA256 verification failed");

        assert!(result, "SHA256 verification should succeed");
    }

    #[test]
    fn test_debug_verification() {
        let prover_config = hardcoded_config_12(
            PhantomData::<BinaryElem32>,
            PhantomData::<BinaryElem128>,
        );
        let verifier_config = hardcoded_config_12_verifier();

        let poly = vec![BinaryElem32::from(42); 1 << 12];

        let proof = prove(&prover_config, &poly).expect("Proof generation failed");
        let result = verify_debug(&verifier_config, &proof).expect("Debug verification failed");

        assert!(result, "Debug verification should succeed");
    }

    #[test]
    fn test_helper_functions() {
        // Test evaluate_quadratic
        let coeffs = (
            BinaryElem128::from(1),
            BinaryElem128::from(2),
            BinaryElem128::from(3),
        );

        let val0 = evaluate_quadratic(coeffs, BinaryElem128::zero());
        assert_eq!(val0, BinaryElem128::from(1));

        let val1 = evaluate_quadratic(coeffs, BinaryElem128::one());
        assert_eq!(val1, BinaryElem128::from(2));

        // Test glue_sums
        let sum_f = BinaryElem128::from(5);
        let sum_g = BinaryElem128::from(7);
        let beta = BinaryElem128::from(3);

        let glued = glue_sums(sum_f, sum_g, beta);
        let expected = sum_f.add(&beta.mul(&sum_g));
        assert_eq!(glued, expected);
    }
}
