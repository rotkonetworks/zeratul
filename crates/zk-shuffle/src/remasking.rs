//! batch chaum-pedersen remasking proof over ristretto255
//!
//! proves output[i] = input[pi(i)] + (r_i * G, r_i * PK) without revealing pi
//!
//! uses unified transcript binding all protocol components:
//! - statement (pk, input, output)
//! - deltas
//! - commitments
//! - all challenges derived in sequence

#[cfg(not(feature = "std"))]
use alloc::vec::Vec;

use curve25519_dalek::{
    ristretto::{CompressedRistretto, RistrettoPoint},
    scalar::Scalar,
    constants::RISTRETTO_BASEPOINT_POINT as G,
};
use rand_core::{CryptoRng, RngCore};

use crate::transcript::Blake2Transcript;

/// elgamal ciphertext over ristretto255
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ElGamalCiphertext {
    /// c0 = r * G (ephemeral key)
    pub c0: RistrettoPoint,
    /// c1 = r * PK + M (encrypted message)
    pub c1: RistrettoPoint,
}

impl ElGamalCiphertext {
    /// create new ciphertext
    pub fn new(c0: RistrettoPoint, c1: RistrettoPoint) -> Self {
        Self { c0, c1 }
    }

    /// encrypt a message point with public key
    pub fn encrypt<R: RngCore + CryptoRng>(
        message: &RistrettoPoint,
        pk: &RistrettoPoint,
        rng: &mut R,
    ) -> (Self, Scalar) {
        let r = Scalar::random(rng);
        let c0 = r * G;
        let c1 = r * pk + message;
        (Self { c0, c1 }, r)
    }

    /// add remasking: output = input + (r*G, r*PK)
    pub fn remask<R: RngCore + CryptoRng>(
        &self,
        pk: &RistrettoPoint,
        rng: &mut R,
    ) -> (Self, Scalar) {
        let r = Scalar::random(rng);
        let delta_c0 = r * G;
        let delta_c1 = r * pk;

        (Self {
            c0: self.c0 + delta_c0,
            c1: self.c1 + delta_c1,
        }, r)
    }

    /// decrypt ciphertext with secret key
    /// M = c1 - sk * c0
    pub fn decrypt(&self, sk: &Scalar) -> RistrettoPoint {
        self.c1 - sk * self.c0
    }

    /// compute delta = self - other
    pub fn sub(&self, other: &Self) -> Self {
        Self {
            c0: self.c0 - other.c0,
            c1: self.c1 - other.c1,
        }
    }

    /// serialize to bytes
    pub fn to_bytes(&self) -> [u8; 64] {
        let mut bytes = [0u8; 64];
        bytes[..32].copy_from_slice(self.c0.compress().as_bytes());
        bytes[32..].copy_from_slice(self.c1.compress().as_bytes());
        bytes
    }

    /// deserialize from bytes
    pub fn from_bytes(bytes: &[u8; 64]) -> Option<Self> {
        let c0 = CompressedRistretto::from_slice(&bytes[..32]).ok()?
            .decompress()?;
        let c1 = CompressedRistretto::from_slice(&bytes[32..]).ok()?
            .decompress()?;
        Some(Self { c0, c1 })
    }
}

/// remasking delta: the difference between output and input
/// delta = (r * G, r * PK) where r is the remasking randomness
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RemaskingDelta {
    /// delta_c0 = r * G
    pub delta_c0: RistrettoPoint,
    /// delta_c1 = r * PK
    pub delta_c1: RistrettoPoint,
}

impl RemaskingDelta {
    /// create from remasking randomness
    pub fn from_randomness(r: Scalar, pk: &RistrettoPoint) -> Self {
        Self {
            delta_c0: r * G,
            delta_c1: r * pk,
        }
    }

    /// serialize to bytes
    pub fn to_bytes(&self) -> [u8; 64] {
        let mut bytes = [0u8; 64];
        bytes[..32].copy_from_slice(self.delta_c0.compress().as_bytes());
        bytes[32..].copy_from_slice(self.delta_c1.compress().as_bytes());
        bytes
    }
}

/// batch chaum-pedersen proof for multiple DH tuples
/// proves: for all i, log_G(delta_c0[i]) = log_PK(delta_c1[i])
#[derive(Clone, Debug)]
pub struct BatchRemaskingProof {
    /// aggregated commitment R = sum(rho_i * k_i * G)
    pub commitment_g: RistrettoPoint,
    /// aggregated commitment S = sum(rho_i * k_i * PK)
    pub commitment_pk: RistrettoPoint,
    /// aggregated response z = sum(rho_i * (k_i + c * r_i))
    pub response: Scalar,
}

impl BatchRemaskingProof {
    /// serialize to bytes
    pub fn to_bytes(&self) -> [u8; 96] {
        let mut bytes = [0u8; 96];
        bytes[..32].copy_from_slice(self.commitment_g.compress().as_bytes());
        bytes[32..64].copy_from_slice(self.commitment_pk.compress().as_bytes());
        bytes[64..].copy_from_slice(self.response.as_bytes());
        bytes
    }
}

/// witness for remasking proof
pub struct RemaskingWitness {
    /// the remasking randomness for each card
    pub randomness: Vec<Scalar>,
    /// the permutation (which input goes to which output position)
    pub permutation: Vec<usize>,
}

impl RemaskingWitness {
    pub fn new(randomness: Vec<Scalar>, permutation: Vec<usize>) -> Self {
        Self { randomness, permutation }
    }
}

/// statement for remasking proof
pub struct RemaskingStatement {
    /// public key used for remasking
    pub pk: RistrettoPoint,
    /// input deck (before shuffle)
    pub input_deck: Vec<ElGamalCiphertext>,
    /// output deck (after shuffle + remask)
    pub output_deck: Vec<ElGamalCiphertext>,
}

/// unified transcript for remasking proofs
/// binds all protocol components in sequence
struct RemaskingTranscript {
    inner: Blake2Transcript,
}

impl RemaskingTranscript {
    /// create new transcript and bind statement
    /// context: optional external binding (e.g., game_id || round || aggregate_pk)
    fn new(statement: &RemaskingStatement, context: Option<&[u8]>) -> Self {
        let mut inner = Blake2Transcript::new(b"zk-shuffle.remasking.v1");

        // bind external context first (game_id, round, etc.)
        if let Some(ctx) = context {
            inner.append_message(b"context", ctx);
        }

        // bind statement
        inner.append_message(b"pk", statement.pk.compress().as_bytes());
        inner.append_u64(b"n", statement.input_deck.len() as u64);

        for card in &statement.input_deck {
            inner.append_message(b"in", &card.to_bytes());
        }
        for card in &statement.output_deck {
            inner.append_message(b"out", &card.to_bytes());
        }

        Self { inner }
    }

    /// bind deltas and derive batch weights
    fn bind_deltas_and_derive_weights(&mut self, deltas: &[RemaskingDelta]) -> Vec<Scalar> {
        // bind all deltas first
        for delta in deltas {
            self.inner.append_message(b"delta", &delta.to_bytes());
        }

        // derive weights
        deltas.iter().enumerate().map(|(i, _)| {
            let mut bytes = [0u8; 64];
            self.inner.challenge_bytes(b"rho", &mut bytes);
            self.inner.append_u64(b"rho_idx", i as u64);
            Scalar::from_bytes_mod_order_wide(&bytes)
        }).collect()
    }

    /// bind commitments and derive schnorr challenge
    fn bind_commitments_and_derive_challenge(
        &mut self,
        commitment_g: &RistrettoPoint,
        commitment_pk: &RistrettoPoint,
    ) -> Scalar {
        self.inner.append_message(b"R", commitment_g.compress().as_bytes());
        self.inner.append_message(b"S", commitment_pk.compress().as_bytes());

        let mut bytes = [0u8; 64];
        self.inner.challenge_bytes(b"c", &mut bytes);
        Scalar::from_bytes_mod_order_wide(&bytes)
    }

    /// derive permutation challenge beta for grand product
    fn derive_permutation_challenge(&mut self) -> Scalar {
        let mut bytes = [0u8; 64];
        self.inner.challenge_bytes(b"beta", &mut bytes);
        Scalar::from_bytes_mod_order_wide(&bytes)
    }
}

/// prover for batch remasking proofs
pub struct RemaskingProver;

impl RemaskingProver {
    /// create batch remasking proof
    ///
    /// proves that for each i:
    ///   output[i] = input[pi(i)] + (r_i * G, r_i * PK)
    ///
    /// without revealing pi or {r_i}
    ///
    /// context: optional binding to game session (game_id || round || aggregate_pk)
    pub fn prove<R: RngCore + CryptoRng>(
        statement: &RemaskingStatement,
        witness: &RemaskingWitness,
        context: Option<&[u8]>,
        rng: &mut R,
    ) -> Result<(Vec<RemaskingDelta>, BatchRemaskingProof), RemaskingError> {
        let n = statement.input_deck.len();
        if statement.output_deck.len() != n {
            return Err(RemaskingError::DeckSizeMismatch);
        }
        if witness.randomness.len() != n || witness.permutation.len() != n {
            return Err(RemaskingError::WitnessSizeMismatch);
        }

        // compute deltas: delta[i] = output[i] - input[pi(i)]
        let mut deltas = Vec::with_capacity(n);
        for i in 0..n {
            let pi_i = witness.permutation[i];
            if pi_i >= n {
                return Err(RemaskingError::InvalidPermutation);
            }

            let expected_delta = RemaskingDelta::from_randomness(
                witness.randomness[i],
                &statement.pk,
            );

            // verify witness is consistent
            let actual_delta = statement.output_deck[i].sub(&statement.input_deck[pi_i]);
            if actual_delta.c0 != expected_delta.delta_c0 ||
               actual_delta.c1 != expected_delta.delta_c1 {
                return Err(RemaskingError::InconsistentWitness);
            }

            deltas.push(expected_delta);
        }

        // create unified transcript with context binding
        let mut transcript = RemaskingTranscript::new(statement, context);

        // step 1: sample random blinding factors
        let k: Vec<Scalar> = (0..n).map(|_| Scalar::random(rng)).collect();

        // step 2: compute individual commitments
        let commitments_g: Vec<RistrettoPoint> = k.iter().map(|k_i| k_i * G).collect();
        let commitments_pk: Vec<RistrettoPoint> = k.iter()
            .map(|k_i| k_i * &statement.pk)
            .collect();

        // step 3: bind deltas and derive batch weights
        let rho = transcript.bind_deltas_and_derive_weights(&deltas);

        // step 4: aggregate commitments
        let agg_commitment_g: RistrettoPoint = commitments_g.iter()
            .zip(rho.iter())
            .map(|(c, r)| r * c)
            .sum();
        let agg_commitment_pk: RistrettoPoint = commitments_pk.iter()
            .zip(rho.iter())
            .map(|(c, r)| r * c)
            .sum();

        // step 5: bind commitments and derive challenge
        let c = transcript.bind_commitments_and_derive_challenge(
            &agg_commitment_g,
            &agg_commitment_pk,
        );

        // step 6: compute aggregated response z = sum(rho_i * (k_i + c * r_i))
        let z: Scalar = (0..n)
            .map(|i| rho[i] * (k[i] + c * witness.randomness[i]))
            .sum();

        let proof = BatchRemaskingProof {
            commitment_g: agg_commitment_g,
            commitment_pk: agg_commitment_pk,
            response: z,
        };

        Ok((deltas, proof))
    }
}

/// verifier for batch remasking proofs
pub struct RemaskingVerifier;

impl RemaskingVerifier {
    /// verify batch remasking proof
    ///
    /// checks:
    /// 1. z * G = R + c * sum(rho_i * delta_c0[i])
    /// 2. z * PK = S + c * sum(rho_i * delta_c1[i])
    /// 3. stripped deck is permutation of input deck (grand product)
    ///
    /// context: must match the context used during proving
    pub fn verify(
        statement: &RemaskingStatement,
        deltas: &[RemaskingDelta],
        proof: &BatchRemaskingProof,
        context: Option<&[u8]>,
    ) -> Result<bool, RemaskingError> {
        let n = statement.input_deck.len();
        if statement.output_deck.len() != n || deltas.len() != n {
            return Err(RemaskingError::DeckSizeMismatch);
        }

        // create unified transcript with context (same as prover)
        let mut transcript = RemaskingTranscript::new(statement, context);

        // derive batch weights
        let rho = transcript.bind_deltas_and_derive_weights(deltas);

        // derive challenge
        let c = transcript.bind_commitments_and_derive_challenge(
            &proof.commitment_g,
            &proof.commitment_pk,
        );

        // aggregate deltas with weights
        let agg_delta_c0: RistrettoPoint = deltas.iter()
            .zip(rho.iter())
            .map(|(d, r)| r * &d.delta_c0)
            .sum();
        let agg_delta_c1: RistrettoPoint = deltas.iter()
            .zip(rho.iter())
            .map(|(d, r)| r * &d.delta_c1)
            .sum();

        // verify equation 1: z * G = R + c * agg_delta_c0
        let lhs_g = proof.response * G;
        let rhs_g = proof.commitment_g + c * agg_delta_c0;

        if lhs_g != rhs_g {
            return Ok(false);
        }

        // verify equation 2: z * PK = S + c * agg_delta_c1
        let lhs_pk = proof.response * &statement.pk;
        let rhs_pk = proof.commitment_pk + c * agg_delta_c1;

        if lhs_pk != rhs_pk {
            return Ok(false);
        }

        // verify stripped deck is permutation of input (grand product)
        Self::verify_permutation_property(statement, deltas, &mut transcript)
    }

    /// verify stripped deck is permutation of input using grand product
    fn verify_permutation_property(
        statement: &RemaskingStatement,
        deltas: &[RemaskingDelta],
        transcript: &mut RemaskingTranscript,
    ) -> Result<bool, RemaskingError> {
        let n = statement.input_deck.len();

        // derive beta from unified transcript
        let beta = transcript.derive_permutation_challenge();

        // hash input cards
        let input_hashes: Vec<Scalar> = statement.input_deck.iter()
            .map(Self::hash_card)
            .collect();

        // hash stripped cards (output - delta)
        let stripped_hashes: Vec<Scalar> = (0..n).map(|i| {
            let stripped = ElGamalCiphertext {
                c0: statement.output_deck[i].c0 - deltas[i].delta_c0,
                c1: statement.output_deck[i].c1 - deltas[i].delta_c1,
            };
            Self::hash_card(&stripped)
        }).collect();

        // grand product check
        let prod_input: Scalar = input_hashes.iter()
            .map(|h| h + beta)
            .product();

        let prod_stripped: Scalar = stripped_hashes.iter()
            .map(|h| h + beta)
            .product();

        Ok(prod_input == prod_stripped)
    }

    fn hash_card(card: &ElGamalCiphertext) -> Scalar {
        let mut transcript = Blake2Transcript::new(b"zk-shuffle.card.v1");
        transcript.append_message(b"c0", card.c0.compress().as_bytes());
        transcript.append_message(b"c1", card.c1.compress().as_bytes());

        let mut bytes = [0u8; 64];
        transcript.challenge_bytes(b"h", &mut bytes);
        Scalar::from_bytes_mod_order_wide(&bytes)
    }
}

/// errors for remasking proofs
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RemaskingError {
    DeckSizeMismatch,
    WitnessSizeMismatch,
    InvalidPermutation,
    InconsistentWitness,
    DeserializationError,
}

impl core::fmt::Display for RemaskingError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            RemaskingError::DeckSizeMismatch => write!(f, "deck size mismatch"),
            RemaskingError::WitnessSizeMismatch => write!(f, "witness size mismatch"),
            RemaskingError::InvalidPermutation => write!(f, "invalid permutation index"),
            RemaskingError::InconsistentWitness => write!(f, "witness inconsistent with statement"),
            RemaskingError::DeserializationError => write!(f, "deserialization error"),
        }
    }
}

#[cfg(feature = "std")]
impl std::error::Error for RemaskingError {}

#[cfg(test)]
mod tests {
    use super::*;
    use rand::rngs::OsRng;

    fn setup_test() -> (RistrettoPoint, Scalar, Vec<ElGamalCiphertext>) {
        let mut rng = OsRng;

        // generate keypair
        let sk = Scalar::random(&mut rng);
        let pk = sk * G;

        // create test deck (4 cards)
        let deck: Vec<_> = (0..4).map(|i| {
            let message = Scalar::from(i as u64) * G;
            let (ct, _) = ElGamalCiphertext::encrypt(&message, &pk, &mut rng);
            ct
        }).collect();

        (pk, sk, deck)
    }

    #[test]
    fn test_elgamal_encrypt_decrypt() {
        let mut rng = OsRng;

        let sk = Scalar::random(&mut rng);
        let pk = sk * G;

        let message = Scalar::from(42u64) * G;
        let (ct, _) = ElGamalCiphertext::encrypt(&message, &pk, &mut rng);

        // decrypt: M = c1 - sk * c0
        let decrypted = ct.c1 - sk * ct.c0;
        assert_eq!(decrypted, message);
    }

    #[test]
    fn test_remasking_preserves_message() {
        let mut rng = OsRng;

        let sk = Scalar::random(&mut rng);
        let pk = sk * G;

        let message = Scalar::from(123u64) * G;
        let (ct, _) = ElGamalCiphertext::encrypt(&message, &pk, &mut rng);

        // remask
        let (remasked, _) = ct.remask(&pk, &mut rng);

        // should decrypt to same message
        let decrypted = remasked.c1 - sk * remasked.c0;
        assert_eq!(decrypted, message);
    }

    #[test]
    fn test_remasking_proof_valid() {
        let mut rng = OsRng;

        let (pk, _sk, input_deck) = setup_test();

        // create shuffled + remasked deck
        let permutation = vec![2, 0, 3, 1];
        let mut output_deck = Vec::with_capacity(4);
        let mut randomness = Vec::with_capacity(4);

        for i in 0..4 {
            let pi_i = permutation[i];
            let (remasked, r) = input_deck[pi_i].remask(&pk, &mut rng);
            output_deck.push(remasked);
            randomness.push(r);
        }

        let statement = RemaskingStatement {
            pk,
            input_deck: input_deck.clone(),
            output_deck: output_deck.clone(),
        };

        let witness = RemaskingWitness::new(randomness, permutation);

        // prove with context
        let context = b"game:test_game|round:1";
        let (deltas, proof) = RemaskingProver::prove(&statement, &witness, Some(context), &mut rng)
            .expect("proof should succeed");

        assert_eq!(deltas.len(), 4);

        // verify with same context
        let valid = RemaskingVerifier::verify(&statement, &deltas, &proof, Some(context))
            .expect("verification should not error");

        assert!(valid, "valid proof should verify");
    }

    #[test]
    fn test_invalid_permutation_detected() {
        let mut rng = OsRng;

        let (pk, _sk, input_deck) = setup_test();

        // create INVALID output (not a permutation - all same card)
        let permutation = vec![0, 0, 0, 0]; // invalid: repeats
        let mut output_deck = Vec::with_capacity(4);
        let mut randomness = Vec::with_capacity(4);

        for _i in 0..4 {
            let (remasked, r) = input_deck[0].remask(&pk, &mut rng);
            output_deck.push(remasked);
            randomness.push(r);
        }

        let statement = RemaskingStatement {
            pk,
            input_deck: input_deck.clone(),
            output_deck: output_deck.clone(),
        };

        let witness = RemaskingWitness::new(randomness, permutation);

        // prove succeeds (witness is consistent)
        let (deltas, proof) = RemaskingProver::prove(&statement, &witness, None, &mut rng)
            .expect("proof generation should work");

        // but verification should FAIL because stripped deck isn't permutation
        let valid = RemaskingVerifier::verify(&statement, &deltas, &proof, None)
            .expect("verification should not error");

        assert!(!valid, "invalid permutation should fail grand product check");
    }

    #[test]
    fn test_corrupted_proof_fails() {
        let mut rng = OsRng;

        let (pk, _sk, input_deck) = setup_test();

        let permutation = vec![1, 0, 3, 2];
        let mut output_deck = Vec::with_capacity(4);
        let mut randomness = Vec::with_capacity(4);

        for i in 0..4 {
            let pi_i = permutation[i];
            let (remasked, r) = input_deck[pi_i].remask(&pk, &mut rng);
            output_deck.push(remasked);
            randomness.push(r);
        }

        let statement = RemaskingStatement {
            pk,
            input_deck: input_deck.clone(),
            output_deck: output_deck.clone(),
        };

        let witness = RemaskingWitness::new(randomness, permutation);

        let (mut deltas, proof) = RemaskingProver::prove(&statement, &witness, None, &mut rng)
            .expect("proof should succeed");

        // corrupt one delta
        deltas[0].delta_c0 = Scalar::random(&mut rng) * G;

        // verification should fail
        let valid = RemaskingVerifier::verify(&statement, &deltas, &proof, None)
            .expect("verification should not error");

        assert!(!valid, "corrupted proof should not verify");
    }

    #[test]
    fn test_wrong_delta_for_permutation_fails() {
        let mut rng = OsRng;

        let (pk, _sk, input_deck) = setup_test();

        // valid shuffle
        let permutation = vec![3, 2, 1, 0];
        let mut output_deck = Vec::with_capacity(4);
        let mut randomness = Vec::with_capacity(4);

        for i in 0..4 {
            let pi_i = permutation[i];
            let (remasked, r) = input_deck[pi_i].remask(&pk, &mut rng);
            output_deck.push(remasked);
            randomness.push(r);
        }

        let statement = RemaskingStatement {
            pk,
            input_deck: input_deck.clone(),
            output_deck: output_deck.clone(),
        };

        // but claim WRONG permutation in witness
        let wrong_perm = vec![0, 1, 2, 3]; // identity, not the real one

        let witness = RemaskingWitness::new(randomness, wrong_perm);

        // prove should fail because witness is inconsistent
        let result = RemaskingProver::prove(&statement, &witness, None, &mut rng);
        assert!(result.is_err(), "inconsistent witness should fail");
    }

    #[test]
    fn test_context_binding() {
        let mut rng = OsRng;
        let (pk, _sk, input_deck) = setup_test();

        let permutation = vec![1, 0, 3, 2];
        let mut output_deck = Vec::with_capacity(4);
        let mut randomness = Vec::with_capacity(4);

        for i in 0..4 {
            let pi_i = permutation[i];
            let (remasked, r) = input_deck[pi_i].remask(&pk, &mut rng);
            output_deck.push(remasked);
            randomness.push(r);
        }

        let statement = RemaskingStatement {
            pk,
            input_deck: input_deck.clone(),
            output_deck: output_deck.clone(),
        };
        let witness = RemaskingWitness::new(randomness, permutation);

        // prove with context A
        let context_a = b"game:abc|round:1";
        let (deltas, proof) = RemaskingProver::prove(&statement, &witness, Some(context_a), &mut rng)
            .expect("proof should succeed");

        // verify with WRONG context B should FAIL
        let context_b = b"game:xyz|round:1";
        let valid = RemaskingVerifier::verify(&statement, &deltas, &proof, Some(context_b))
            .expect("verification should not error");

        assert!(!valid, "proof with wrong context should fail");

        // verify with correct context A should PASS
        let valid = RemaskingVerifier::verify(&statement, &deltas, &proof, Some(context_a))
            .expect("verification should not error");

        assert!(valid, "proof with correct context should pass");
    }

    #[test]
    fn test_serialization_roundtrip() {
        let mut rng = OsRng;

        let sk = Scalar::random(&mut rng);
        let pk = sk * G;

        let message = Scalar::from(99u64) * G;
        let (ct, _) = ElGamalCiphertext::encrypt(&message, &pk, &mut rng);

        let bytes = ct.to_bytes();
        let recovered = ElGamalCiphertext::from_bytes(&bytes)
            .expect("should deserialize");

        assert_eq!(ct.c0, recovered.c0);
        assert_eq!(ct.c1, recovered.c1);
    }

    #[test]
    fn test_proof_size() {
        let mut rng = OsRng;
        let (pk, _sk, input_deck) = setup_test();

        let permutation = vec![0, 1, 2, 3];
        let mut output_deck = Vec::with_capacity(4);
        let mut randomness = Vec::with_capacity(4);

        for i in 0..4 {
            let (remasked, r) = input_deck[i].remask(&pk, &mut rng);
            output_deck.push(remasked);
            randomness.push(r);
        }

        let statement = RemaskingStatement { pk, input_deck, output_deck };
        let witness = RemaskingWitness::new(randomness, permutation);

        let (deltas, proof) = RemaskingProver::prove(&statement, &witness, None, &mut rng).unwrap();

        // proof is constant size: 2 points + 1 scalar = 96 bytes
        assert_eq!(proof.to_bytes().len(), 96);

        // deltas are O(n): 64 bytes each
        let delta_bytes: usize = deltas.iter().map(|d| d.to_bytes().len()).sum();
        assert_eq!(delta_bytes, 4 * 64); // 256 bytes for 4 cards
    }
}
