//! batch chaum-pedersen verification
//!
//! verifies multiple dl equality proofs efficiently using random linear combinations
//! reduces n multi-exponentiations to 2 multi-exponentiations
//!
//! based on the optimization from:
//! "batch verification of short signatures" (bellare et al.)

#[cfg(not(feature = "std"))]
use alloc::vec::Vec;

use blake2::{Blake2s256, Digest};

/// domain separator for batch verification challenges
const BATCH_DOMAIN_SEP: &[u8] = b"mental-poker.batch-cp.v1";

/// a chaum-pedersen proof of discrete log equality
///
/// proves knowledge of x such that A = x*G and B = x*H
#[derive(Clone, Debug)]
pub struct ChaumPedersenProof {
    /// commitment: R = r*G
    pub commitment_g: (u64, u64),
    /// commitment: S = r*H
    pub commitment_h: (u64, u64),
    /// response: z = r + c*x
    pub response: u64,
}

/// statement for chaum-pedersen proof
///
/// public values (G, H, A, B) where we prove log_G(A) = log_H(B)
#[derive(Clone, Debug)]
pub struct ChaumPedersenStatement {
    /// base point G
    pub base_g: (u64, u64),
    /// base point H
    pub base_h: (u64, u64),
    /// public value A = x*G
    pub public_a: (u64, u64),
    /// public value B = x*H
    pub public_b: (u64, u64),
}

/// batch verification context
///
/// collects multiple proofs for efficient batch verification
#[derive(Clone, Debug)]
pub struct BatchVerifier {
    /// accumulated proofs
    proofs: Vec<(ChaumPedersenStatement, ChaumPedersenProof)>,
    /// transcript seed for deterministic challenges
    seed: [u8; 32],
}

impl BatchVerifier {
    /// create new batch verifier with transcript binding
    pub fn new(seed: &[u8]) -> Self {
        let mut hasher = Blake2s256::new();
        hasher.update(BATCH_DOMAIN_SEP);
        hasher.update(seed);
        let hash = hasher.finalize();

        let mut seed_arr = [0u8; 32];
        seed_arr.copy_from_slice(&hash);

        Self {
            proofs: Vec::new(),
            seed: seed_arr,
        }
    }

    /// add a proof to the batch
    pub fn add(&mut self, statement: ChaumPedersenStatement, proof: ChaumPedersenProof) {
        self.proofs.push((statement, proof));
    }

    /// number of proofs in batch
    pub fn len(&self) -> usize {
        self.proofs.len()
    }

    /// check if batch is empty
    pub fn is_empty(&self) -> bool {
        self.proofs.is_empty()
    }

    /// generate deterministic batch challenge for proof i
    fn challenge_for(&self, i: usize) -> u64 {
        let mut hasher = Blake2s256::new();
        hasher.update(&self.seed);
        hasher.update(&(i as u64).to_le_bytes());

        // include proof data for uniqueness
        if i < self.proofs.len() {
            let (stmt, proof) = &self.proofs[i];
            hasher.update(&stmt.base_g.0.to_le_bytes());
            hasher.update(&stmt.base_g.1.to_le_bytes());
            hasher.update(&stmt.public_a.0.to_le_bytes());
            hasher.update(&stmt.public_a.1.to_le_bytes());
            hasher.update(&proof.commitment_g.0.to_le_bytes());
            hasher.update(&proof.response.to_le_bytes());
        }

        let hash = hasher.finalize();
        u64::from_le_bytes(hash[..8].try_into().unwrap())
    }

    /// compute fiat-shamir challenge for single proof
    fn single_challenge(stmt: &ChaumPedersenStatement, proof: &ChaumPedersenProof) -> u64 {
        let mut hasher = Blake2s256::new();
        hasher.update(b"chaum-pedersen.challenge");
        hasher.update(&stmt.base_g.0.to_le_bytes());
        hasher.update(&stmt.base_g.1.to_le_bytes());
        hasher.update(&stmt.base_h.0.to_le_bytes());
        hasher.update(&stmt.base_h.1.to_le_bytes());
        hasher.update(&stmt.public_a.0.to_le_bytes());
        hasher.update(&stmt.public_a.1.to_le_bytes());
        hasher.update(&stmt.public_b.0.to_le_bytes());
        hasher.update(&stmt.public_b.1.to_le_bytes());
        hasher.update(&proof.commitment_g.0.to_le_bytes());
        hasher.update(&proof.commitment_g.1.to_le_bytes());
        hasher.update(&proof.commitment_h.0.to_le_bytes());
        hasher.update(&proof.commitment_h.1.to_le_bytes());

        let hash = hasher.finalize();
        u64::from_le_bytes(hash[..8].try_into().unwrap())
    }

    /// verify batch using random linear combination
    ///
    /// instead of checking for each i:
    ///   z_i * G = R_i + c_i * A_i
    ///   z_i * H = S_i + c_i * B_i
    ///
    /// we check with random weights ρ_i:
    ///   Σ ρ_i * z_i * G = Σ ρ_i * (R_i + c_i * A_i)
    ///   Σ ρ_i * z_i * H = Σ ρ_i * (S_i + c_i * B_i)
    ///
    /// this reduces n verification equations to 2 multi-scalar multiplications
    ///
    /// returns (is_valid, individual_checks) where individual_checks contains
    /// per-proof verification results for debugging
    pub fn verify_batch(&self) -> (bool, Vec<bool>) {
        if self.proofs.is_empty() {
            return (true, Vec::new());
        }

        let n = self.proofs.len();
        let mut individual_results = Vec::with_capacity(n);

        // for this simplified implementation, we do individual checks
        // and also compute the batch equation
        //
        // in a real implementation with elliptic curve points, we would:
        // 1. compute batch challenge ρ_i for each proof
        // 2. compute weighted sums of scalars and points
        // 3. check single msm equation
        //
        // here we simulate with u64 arithmetic as placeholder

        // individual verification (for comparison/debugging)
        for (stmt, proof) in &self.proofs {
            let c = Self::single_challenge(stmt, proof);

            // check: z * G = R + c * A (simplified with addition as placeholder)
            // in real EC: z * G == R + c * A
            let lhs_g = proof.response.wrapping_mul(stmt.base_g.0);
            let rhs_g = proof
                .commitment_g
                .0
                .wrapping_add(c.wrapping_mul(stmt.public_a.0));

            let lhs_h = proof.response.wrapping_mul(stmt.base_h.0);
            let rhs_h = proof
                .commitment_h
                .0
                .wrapping_add(c.wrapping_mul(stmt.public_b.0));

            let valid = (lhs_g == rhs_g) && (lhs_h == rhs_h);
            individual_results.push(valid);
        }

        // batch verification
        // compute weighted sum with random challenges
        let mut batch_lhs_g = 0u64;
        let mut batch_rhs_g = 0u64;
        let mut batch_lhs_h = 0u64;
        let mut batch_rhs_h = 0u64;

        for i in 0..n {
            let rho_i = self.challenge_for(i);
            let (stmt, proof) = &self.proofs[i];
            let c_i = Self::single_challenge(stmt, proof);

            // accumulate: ρ_i * z_i * G
            batch_lhs_g = batch_lhs_g.wrapping_add(
                rho_i.wrapping_mul(proof.response.wrapping_mul(stmt.base_g.0)),
            );

            // accumulate: ρ_i * (R_i + c_i * A_i)
            batch_rhs_g = batch_rhs_g.wrapping_add(rho_i.wrapping_mul(
                proof
                    .commitment_g
                    .0
                    .wrapping_add(c_i.wrapping_mul(stmt.public_a.0)),
            ));

            // accumulate: ρ_i * z_i * H
            batch_lhs_h = batch_lhs_h.wrapping_add(
                rho_i.wrapping_mul(proof.response.wrapping_mul(stmt.base_h.0)),
            );

            // accumulate: ρ_i * (S_i + c_i * B_i)
            batch_rhs_h = batch_rhs_h.wrapping_add(rho_i.wrapping_mul(
                proof
                    .commitment_h
                    .0
                    .wrapping_add(c_i.wrapping_mul(stmt.public_b.0)),
            ));
        }

        let batch_valid = (batch_lhs_g == batch_rhs_g) && (batch_lhs_h == batch_rhs_h);

        (batch_valid, individual_results)
    }

    /// verify batch, returning only overall result
    pub fn verify(&self) -> bool {
        self.verify_batch().0
    }
}

/// batch verification result with detailed diagnostics
#[derive(Clone, Debug)]
pub struct BatchVerificationResult {
    /// overall batch validity
    pub valid: bool,
    /// number of proofs verified
    pub count: usize,
    /// indices of invalid proofs (if any)
    pub invalid_indices: Vec<usize>,
}

impl BatchVerificationResult {
    /// create from verification results
    pub fn from_results(valid: bool, individual_checks: &[bool]) -> Self {
        let invalid_indices: Vec<usize> = individual_checks
            .iter()
            .enumerate()
            .filter_map(|(i, &v)| if !v { Some(i) } else { None })
            .collect();

        Self {
            valid,
            count: individual_checks.len(),
            invalid_indices,
        }
    }
}

/// convenience function to verify reveal tokens in batch
///
/// for mental poker, reveal tokens are computed as: token = sk * c0
/// where c0 is the first component of the masked card
///
/// we verify: log_G(pk) = log_{c0}(token)
/// which proves token was computed with the same secret key as pk
pub fn batch_verify_reveal_tokens(
    reveal_data: &[(
        (u64, u64), // generator G
        (u64, u64), // public key pk = sk * G
        (u64, u64), // masked card c0
        (u64, u64), // reveal token = sk * c0
        ChaumPedersenProof,
    )],
    transcript_seed: &[u8],
) -> BatchVerificationResult {
    let mut verifier = BatchVerifier::new(transcript_seed);

    for (generator, pk, c0, token, proof) in reveal_data {
        let statement = ChaumPedersenStatement {
            base_g: *generator,
            base_h: *c0,
            public_a: *pk,
            public_b: *token,
        };
        verifier.add(statement, proof.clone());
    }

    let (valid, individual) = verifier.verify_batch();
    BatchVerificationResult::from_results(valid, &individual)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_valid_proof() -> (ChaumPedersenStatement, ChaumPedersenProof) {
        // simplified test case
        // in real implementation, this would use actual EC operations
        let x = 42u64; // secret
        let r = 17u64; // randomness
        let g = (7u64, 1u64); // generator
        let h = (11u64, 1u64); // second base

        // public values
        let a = (x.wrapping_mul(g.0), 1); // x * G
        let b = (x.wrapping_mul(h.0), 1); // x * H

        // commitments
        let commit_g = (r.wrapping_mul(g.0), 1);
        let commit_h = (r.wrapping_mul(h.0), 1);

        let stmt = ChaumPedersenStatement {
            base_g: g,
            base_h: h,
            public_a: a,
            public_b: b,
        };

        // compute challenge
        let c = BatchVerifier::single_challenge(
            &stmt,
            &ChaumPedersenProof {
                commitment_g: commit_g,
                commitment_h: commit_h,
                response: 0,
            },
        );

        // response: z = r + c * x
        let z = r.wrapping_add(c.wrapping_mul(x));

        let proof = ChaumPedersenProof {
            commitment_g: commit_g,
            commitment_h: commit_h,
            response: z,
        };

        (stmt, proof)
    }

    #[test]
    fn test_batch_verifier_empty() {
        let verifier = BatchVerifier::new(b"test");
        assert!(verifier.is_empty());
        assert!(verifier.verify());
    }

    #[test]
    fn test_batch_verifier_single() {
        let mut verifier = BatchVerifier::new(b"test");
        let (stmt, proof) = make_valid_proof();

        verifier.add(stmt, proof);
        assert_eq!(verifier.len(), 1);

        let (valid, individual) = verifier.verify_batch();
        assert!(valid);
        assert_eq!(individual.len(), 1);
        assert!(individual[0]);
    }

    #[test]
    fn test_batch_verifier_multiple() {
        let mut verifier = BatchVerifier::new(b"multi_test");

        for _ in 0..5 {
            let (stmt, proof) = make_valid_proof();
            verifier.add(stmt, proof);
        }

        assert_eq!(verifier.len(), 5);

        let result = verifier.verify();
        assert!(result);
    }

    #[test]
    fn test_batch_verifier_invalid() {
        let mut verifier = BatchVerifier::new(b"invalid_test");

        // add valid proof
        let (stmt, proof) = make_valid_proof();
        verifier.add(stmt, proof);

        // add invalid proof (wrong response)
        let (stmt2, mut proof2) = make_valid_proof();
        proof2.response = proof2.response.wrapping_add(1); // corrupt
        verifier.add(stmt2, proof2);

        let (valid, individual) = verifier.verify_batch();
        assert!(!valid);
        assert!(individual[0]);
        assert!(!individual[1]);
    }

    #[test]
    fn test_batch_verification_result() {
        let checks = vec![true, true, false, true, false];
        let result = BatchVerificationResult::from_results(false, &checks);

        assert!(!result.valid);
        assert_eq!(result.count, 5);
        assert_eq!(result.invalid_indices, vec![2, 4]);
    }

    #[test]
    fn test_challenge_determinism() {
        let verifier1 = BatchVerifier::new(b"same_seed");
        let verifier2 = BatchVerifier::new(b"same_seed");

        let c1 = verifier1.challenge_for(0);
        let c2 = verifier2.challenge_for(0);
        assert_eq!(c1, c2);

        let verifier3 = BatchVerifier::new(b"different_seed");
        let c3 = verifier3.challenge_for(0);
        assert_ne!(c1, c3);
    }
}
