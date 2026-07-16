use binary_fields::BinaryFieldElement;
use crate::{
   VerifierConfig, FinalizedLigeritoProof,
   transcript::{FiatShamir, Transcript},
   ligero::verify_ligero,
   sumcheck_polys::induce_sumcheck_poly_debug,
   utils::{eval_sk_at_vks, partial_eval_multilinear, evaluate_lagrange_basis},
};
use merkle_tree::{self, Hash};
use sha2::{Sha256, Digest};

const S: usize = 148;
const LOG_INV_RATE: usize = 2;

/// Hash a row of field elements with deterministic serialization
#[inline(always)]
fn hash_row<F: BinaryFieldElement>(row: &[F]) -> Hash {
   let mut hasher = Sha256::new();
   
   // Position-dependent hashing prevents reordering attacks
   for (i, elem) in row.iter().enumerate() {
       hasher.update(&(i as u32).to_le_bytes());
       
       // Use Debug trait for deterministic serialization
       // This is safe and avoids undefined behavior from raw memory access
       let elem_bytes = format!("{:?}", elem).into_bytes();
       hasher.update(&elem_bytes);
   }
   
   hasher.finalize().into()
}

/// Verify a Ligerito proof
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

   // Induce sumcheck polynomial
   let sks_vks: Vec<T> = eval_sk_at_vks(1 << config.initial_dim);
   let (basis_poly, _) = induce_sumcheck_poly_debug(
       config.initial_dim,
       &sks_vks,
       &proof.initial_ligero_proof.opened_rows,
       &partial_evals_0,
       &queries,
       alpha,
   );
   
   // CRITICAL FIX: For sumcheck, we need the sum of the basis polynomial evaluations
   let mut current_sum = basis_poly.iter().fold(U::zero(), |acc, &x| acc.add(&x));

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

       // Continue recursion
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

       // Induce next polynomial
       let sks_vks: Vec<U> = eval_sk_at_vks(1 << config.log_dims[i]);
       let (basis_poly_next, _) = induce_sumcheck_poly_debug(
           config.log_dims[i],
           &sks_vks,
           &ligero_proof.opened_rows,
           &rs,
           &queries,
           alpha,
       );
       
       // Get the sum of the new basis polynomial
       let enforced_sum = basis_poly_next.iter().fold(U::zero(), |acc, &x| acc.add(&x));

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
   // Reuse main verification logic
   verify_with_transcript_impl(config, proof, &mut fs)
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

// Private implementation to avoid code duplication
#[inline(always)]
fn verify_with_transcript_impl<T, U>(
   config: &VerifierConfig,
   proof: &FinalizedLigeritoProof<T, U>,
   fs: &mut impl Transcript,
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
   let (basis_poly, _) = induce_sumcheck_poly_debug(
       config.initial_dim,
       &sks_vks,
       &proof.initial_ligero_proof.opened_rows,
       &partial_evals_0,
       &queries,
       alpha,
   );
   
   let mut current_sum = basis_poly.iter().fold(U::zero(), |acc, &x| acc.add(&x));

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
       let (basis_poly_next, _) = induce_sumcheck_poly_debug(
           config.log_dims[i],
           &sks_vks,
           &ligero_proof.opened_rows,
           &rs,
           &queries,
           alpha,
       );
       
       let enforced_sum = basis_poly_next.iter().fold(U::zero(), |acc, &x| acc.add(&x));

       let glue_sum = current_sum.add(&enforced_sum);
       fs.absorb_elem(glue_sum);
       
       let beta = fs.get_challenge::<U>();
       current_sum = glue_sums(current_sum, enforced_sum, beta);
   }

   Ok(true)
}

/// Debug version to find where verification fails
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

    // Induce sumcheck polynomial
    let sks_vks: Vec<T> = eval_sk_at_vks(1 << config.initial_dim);
    println!("Computed {} sks_vks", sks_vks.len());
    
    let (basis_poly, _) = induce_sumcheck_poly_debug(
        config.initial_dim,
        &sks_vks,
        &proof.initial_ligero_proof.opened_rows,
        &partial_evals_0,
        &queries,
        alpha,
    );
    
    // CRITICAL FIX: For sumcheck, we need the sum of the basis polynomial evaluations
    let mut current_sum = basis_poly.iter().fold(U::zero(), |acc, &x| acc.add(&x));
    println!("Induced sumcheck, current_sum (basis poly sum): {:?}", current_sum);

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
            let claimed_sum = evaluate_quadratic(coeffs, U::zero())
                .add(&evaluate_quadratic(coeffs, U::one()));

            println!("  Round {}: coeffs={:?}, claimed_sum={:?}, current_sum={:?}", 
                     j, coeffs, claimed_sum, current_sum);
            
            if claimed_sum != current_sum {
                println!("  FAILED: Sumcheck mismatch!");
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

        // Induce next polynomial
        let sks_vks: Vec<U> = eval_sk_at_vks(1 << config.log_dims[i]);
        let (basis_poly_next, _) = induce_sumcheck_poly_debug(
            config.log_dims[i],
            &sks_vks,
            &ligero_proof.opened_rows,
            &rs,
            &queries,
            alpha,
        );
        
        // Get the sum of the new basis polynomial
        let enforced_sum = basis_poly_next.iter().fold(U::zero(), |acc, &x| acc.add(&x));
        println!("Induced next sumcheck, enforced_sum (basis poly sum): {:?}", enforced_sum);

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

// Helper functions

#[inline(always)]
fn evaluate_quadratic<F: BinaryFieldElement>(coeffs: (F, F, F), x: F) -> F {
   let (a0, a1, a2) = coeffs;
   let linear = a1.add(&a0).add(&a2);
   a0.add(&linear.mul(&x)).add(&a2.mul(&x).mul(&x))
}

#[inline(always)]
fn glue_sums<F: BinaryFieldElement>(sum_f: F, sum_g: F, beta: F) -> F {
   sum_f.add(&beta.mul(&sum_g))
}
