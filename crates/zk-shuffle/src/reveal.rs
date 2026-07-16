//! sigma proofs for key possession and verified decryption shares
//!
//! - PossessionProof: schnorr proof of knowledge of sk for pk = sk*G,
//!   blocks rogue-key attacks on the aggregate key
//! - RevealProof: chaum-pedersen dleq proving share = sk*c0 with the same
//!   sk as pk = sk*G, blocks forged decryption shares

use curve25519_dalek::{
    constants::RISTRETTO_BASEPOINT_POINT as G,
    ristretto::{CompressedRistretto, RistrettoPoint},
    scalar::Scalar,
};
use rand_core::{CryptoRng, RngCore};

use crate::transcript::Blake2Transcript;

/// schnorr proof of possession: knowledge of sk such that pk = sk * G
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PossessionProof {
    /// commitment R = k * G
    pub commitment: RistrettoPoint,
    /// response z = k + c * sk
    pub response: Scalar,
}

impl PossessionProof {
    /// prove possession of sk for pk = sk * G
    pub fn prove<R: RngCore + CryptoRng>(sk: &Scalar, rng: &mut R) -> Self {
        let pk = sk * G;
        let k = Scalar::random(rng);
        let commitment = k * G;
        let c = Self::challenge(&pk, &commitment);
        Self {
            commitment,
            response: k + c * sk,
        }
    }

    /// verify: z * G == R + c * pk
    pub fn verify(&self, pk: &RistrettoPoint) -> bool {
        let c = Self::challenge(pk, &self.commitment);
        self.response * G == self.commitment + c * pk
    }

    fn challenge(pk: &RistrettoPoint, commitment: &RistrettoPoint) -> Scalar {
        let mut t = Blake2Transcript::new(b"zk-shuffle.pop.v1");
        t.append_message(b"pk", pk.compress().as_bytes());
        t.append_message(b"R", commitment.compress().as_bytes());
        let mut bytes = [0u8; 64];
        t.challenge_bytes(b"c", &mut bytes);
        Scalar::from_bytes_mod_order_wide(&bytes)
    }

    /// serialize to bytes
    pub fn to_bytes(&self) -> [u8; 64] {
        let mut bytes = [0u8; 64];
        bytes[..32].copy_from_slice(self.commitment.compress().as_bytes());
        bytes[32..].copy_from_slice(self.response.as_bytes());
        bytes
    }

    /// deserialize from bytes
    pub fn from_bytes(bytes: &[u8]) -> Option<Self> {
        if bytes.len() != 64 {
            return None;
        }
        let commitment = CompressedRistretto::from_slice(&bytes[..32])
            .ok()?
            .decompress()?;
        let mut z_bytes = [0u8; 32];
        z_bytes.copy_from_slice(&bytes[32..]);
        let response = Scalar::from_canonical_bytes(z_bytes).into_option()?;
        Some(Self {
            commitment,
            response,
        })
    }
}

/// chaum-pedersen dleq proof for a decryption share:
/// knowledge of sk such that pk = sk * G and share = sk * c0
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RevealProof {
    /// commitment R = k * G
    pub commitment_g: RistrettoPoint,
    /// commitment S = k * c0
    pub commitment_c0: RistrettoPoint,
    /// response z = k + c * sk
    pub response: Scalar,
}

impl RevealProof {
    /// compute share = sk * c0 and prove it correct against pk = sk * G
    pub fn prove<R: RngCore + CryptoRng>(
        sk: &Scalar,
        c0: &RistrettoPoint,
        rng: &mut R,
    ) -> (RistrettoPoint, Self) {
        let pk = sk * G;
        let share = sk * c0;
        let k = Scalar::random(rng);
        let commitment_g = k * G;
        let commitment_c0 = k * c0;
        let c = Self::challenge(&pk, c0, &share, &commitment_g, &commitment_c0);
        let proof = Self {
            commitment_g,
            commitment_c0,
            response: k + c * sk,
        };
        (share, proof)
    }

    /// verify: z * G == R + c * pk and z * c0 == S + c * share
    pub fn verify(
        &self,
        pk: &RistrettoPoint,
        c0: &RistrettoPoint,
        share: &RistrettoPoint,
    ) -> bool {
        let c = Self::challenge(pk, c0, share, &self.commitment_g, &self.commitment_c0);
        self.response * G == self.commitment_g + c * pk
            && self.response * c0 == self.commitment_c0 + c * share
    }

    fn challenge(
        pk: &RistrettoPoint,
        c0: &RistrettoPoint,
        share: &RistrettoPoint,
        commitment_g: &RistrettoPoint,
        commitment_c0: &RistrettoPoint,
    ) -> Scalar {
        let mut t = Blake2Transcript::new(b"zk-shuffle.reveal.v1");
        t.append_message(b"pk", pk.compress().as_bytes());
        t.append_message(b"c0", c0.compress().as_bytes());
        t.append_message(b"share", share.compress().as_bytes());
        t.append_message(b"R", commitment_g.compress().as_bytes());
        t.append_message(b"S", commitment_c0.compress().as_bytes());
        let mut bytes = [0u8; 64];
        t.challenge_bytes(b"c", &mut bytes);
        Scalar::from_bytes_mod_order_wide(&bytes)
    }

    /// serialize to bytes
    pub fn to_bytes(&self) -> [u8; 96] {
        let mut bytes = [0u8; 96];
        bytes[..32].copy_from_slice(self.commitment_g.compress().as_bytes());
        bytes[32..64].copy_from_slice(self.commitment_c0.compress().as_bytes());
        bytes[64..].copy_from_slice(self.response.as_bytes());
        bytes
    }

    /// deserialize from bytes
    pub fn from_bytes(bytes: &[u8]) -> Option<Self> {
        if bytes.len() != 96 {
            return None;
        }
        let commitment_g = CompressedRistretto::from_slice(&bytes[..32])
            .ok()?
            .decompress()?;
        let commitment_c0 = CompressedRistretto::from_slice(&bytes[32..64])
            .ok()?
            .decompress()?;
        let mut z_bytes = [0u8; 32];
        z_bytes.copy_from_slice(&bytes[64..]);
        let response = Scalar::from_canonical_bytes(z_bytes).into_option()?;
        Some(Self {
            commitment_g,
            commitment_c0,
            response,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::remasking::ElGamalCiphertext;
    use rand::rngs::OsRng;

    #[test]
    fn test_possession_proof_valid() {
        let sk = Scalar::random(&mut OsRng);
        let pk = sk * G;
        let proof = PossessionProof::prove(&sk, &mut OsRng);
        assert!(proof.verify(&pk));
    }

    #[test]
    fn test_possession_proof_wrong_pk_fails() {
        let sk = Scalar::random(&mut OsRng);
        let proof = PossessionProof::prove(&sk, &mut OsRng);
        let other_pk = Scalar::random(&mut OsRng) * G;
        assert!(!proof.verify(&other_pk));
    }

    #[test]
    fn test_possession_proof_rogue_key_fails() {
        // attacker knows q but not the dlog of pk_b = q*G - pk_a
        let sk_a = Scalar::random(&mut OsRng);
        let pk_a = sk_a * G;
        let q = Scalar::random(&mut OsRng);
        let rogue_pk = q * G - pk_a;
        let proof = PossessionProof::prove(&q, &mut OsRng);
        assert!(!proof.verify(&rogue_pk));
    }

    #[test]
    fn test_possession_proof_serialization() {
        let sk = Scalar::random(&mut OsRng);
        let pk = sk * G;
        let proof = PossessionProof::prove(&sk, &mut OsRng);
        let recovered = PossessionProof::from_bytes(&proof.to_bytes()).unwrap();
        assert_eq!(proof, recovered);
        assert!(recovered.verify(&pk));
    }

    #[test]
    fn test_reveal_proof_valid() {
        let sk = Scalar::random(&mut OsRng);
        let pk = sk * G;
        let msg = Scalar::from(7u64) * G;
        let (ct, _) = ElGamalCiphertext::encrypt(&msg, &pk, &mut OsRng);
        let (share, proof) = RevealProof::prove(&sk, &ct.c0, &mut OsRng);
        assert_eq!(share, sk * ct.c0);
        assert!(proof.verify(&pk, &ct.c0, &share));
    }

    #[test]
    fn test_reveal_proof_forged_share_fails() {
        let sk = Scalar::random(&mut OsRng);
        let pk = sk * G;
        let msg = Scalar::from(7u64) * G;
        let (ct, _) = ElGamalCiphertext::encrypt(&msg, &pk, &mut OsRng);
        let (share, proof) = RevealProof::prove(&sk, &ct.c0, &mut OsRng);
        // forged share with the honest proof
        let fake_share = share + G;
        assert!(!proof.verify(&pk, &ct.c0, &fake_share));
        // honest share against wrong pk
        let other_pk = Scalar::random(&mut OsRng) * G;
        assert!(!proof.verify(&other_pk, &ct.c0, &share));
    }

    #[test]
    fn test_reveal_proof_serialization() {
        let sk = Scalar::random(&mut OsRng);
        let pk = sk * G;
        let c0 = Scalar::random(&mut OsRng) * G;
        let (share, proof) = RevealProof::prove(&sk, &c0, &mut OsRng);
        let recovered = RevealProof::from_bytes(&proof.to_bytes()).unwrap();
        assert_eq!(proof, recovered);
        assert!(recovered.verify(&pk, &c0, &share));
        assert!(RevealProof::from_bytes(&proof.to_bytes()[..95]).is_none());
    }
}
