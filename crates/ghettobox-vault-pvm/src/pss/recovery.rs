//! threshold recovery using OSST contributions
//!
//! elgamal-style encryption to group key with threshold decryption:
//! - encrypt: (R, C) = (g^r, m ⊕ H(Y^r))
//! - partial decrypt: each provider computes R^{x_i} with OSST proof
//! - combine: lagrange interpolate partials → Y^r → m

use curve25519_dalek::{ristretto::RistrettoPoint, scalar::Scalar};
use osst::{Contribution, SecretShare, compute_lagrange_coefficients, verify};
use sha2::Sha256;
use serde::{Deserialize, Serialize};

/// ciphertext encrypted to group public key
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct GroupCiphertext {
    /// R = g^r (ephemeral public key)
    #[serde(with = "point_hex")]
    pub ephemeral: RistrettoPoint,
    /// C = m ⊕ H(Y^r) (encrypted data)
    pub ciphertext: Vec<u8>,
}

/// partial decryption from a single provider
#[derive(Clone, Debug)]
pub struct PartialDecryption {
    /// provider index (1-indexed)
    pub index: u32,
    /// R^{x_i} = g^{r·x_i} (partial shared secret)
    pub partial: RistrettoPoint,
    /// OSST contribution proving knowledge of x_i
    pub contribution: Contribution<RistrettoPoint>,
}

impl GroupCiphertext {
    /// encrypt data to group public key
    ///
    /// uses elgamal-style encryption:
    /// - R = g^r (ephemeral key)
    /// - shared = Y^r (DH with group key)
    /// - C = m ⊕ H(shared)
    pub fn encrypt(group_pubkey: &RistrettoPoint, data: &[u8]) -> Self {
        let mut rng = rand::thread_rng();
        let r = Scalar::random(&mut rng);

        // R = g^r
        let ephemeral = RistrettoPoint::mul_base(&r);

        // shared = Y^r
        let shared = group_pubkey * r;

        // derive mask from shared secret
        let mask = derive_mask(&shared, data.len());

        // C = m ⊕ mask
        let ciphertext: Vec<u8> = data.iter()
            .zip(mask.iter())
            .map(|(a, b)| a ^ b)
            .collect();

        Self { ephemeral, ciphertext }
    }

    /// decrypt with reconstructed shared secret
    pub fn decrypt(&self, shared_secret: &RistrettoPoint) -> Vec<u8> {
        let mask = derive_mask(shared_secret, self.ciphertext.len());

        self.ciphertext.iter()
            .zip(mask.iter())
            .map(|(a, b)| a ^ b)
            .collect()
    }
}

/// provider generates partial decryption with OSST proof
pub fn partial_decrypt(
    share: &SecretShare<Scalar>,
    ciphertext: &GroupCiphertext,
    payload: &[u8],
) -> PartialDecryption {
    let mut rng = rand::thread_rng();

    // partial = R^{x_i}
    let partial = ciphertext.ephemeral * share.scalar;

    // generate OSST contribution proving knowledge of x_i
    let contribution = share.contribute(&mut rng, payload);

    PartialDecryption {
        index: share.index,
        partial,
        contribution,
    }
}

/// combine partial decryptions to recover shared secret
///
/// uses lagrange interpolation on the partial values:
/// Y^r = R^s = Σ λ_i · R^{x_i}
pub fn combine_partials(
    partials: &[PartialDecryption],
    group_pubkey: &RistrettoPoint,
    threshold: u32,
    payload: &[u8],
) -> Result<RistrettoPoint, RecoveryError> {
    if partials.len() < threshold as usize {
        return Err(RecoveryError::InsufficientContributions {
            got: partials.len(),
            need: threshold as usize,
        });
    }

    // verify all OSST contributions
    let contributions: Vec<Contribution<RistrettoPoint>> = partials
        .iter()
        .map(|p| p.contribution.clone())
        .collect();

    let valid = verify(group_pubkey, &contributions, threshold, payload)
        .map_err(|e| RecoveryError::VerificationFailed(format!("{:?}", e)))?;

    if !valid {
        return Err(RecoveryError::VerificationFailed("OSST proof invalid".into()));
    }

    // compute lagrange coefficients
    let indices: Vec<u32> = partials.iter().map(|p| p.index).collect();
    let lagrange = compute_lagrange_coefficients::<Scalar>(&indices)
        .map_err(|e| RecoveryError::LagrangeError(format!("{:?}", e)))?;

    // interpolate: Y^r = Σ λ_i · R^{x_i}
    let mut shared_secret = RistrettoPoint::default();
    for (partial, lambda) in partials.iter().zip(lagrange.iter()) {
        shared_secret += partial.partial * lambda;
    }

    Ok(shared_secret)
}

/// full threshold recovery: decrypt ciphertext using partial contributions
pub fn threshold_decrypt(
    ciphertext: &GroupCiphertext,
    partials: &[PartialDecryption],
    group_pubkey: &RistrettoPoint,
    threshold: u32,
    payload: &[u8],
) -> Result<Vec<u8>, RecoveryError> {
    let shared_secret = combine_partials(partials, group_pubkey, threshold, payload)?;
    Ok(ciphertext.decrypt(&shared_secret))
}

/// derive mask from shared secret using HKDF-SHA256
fn derive_mask(shared: &RistrettoPoint, len: usize) -> Vec<u8> {
    use hkdf::Hkdf;

    let shared_bytes = shared.compress().as_bytes().to_vec();
    let hk = Hkdf::<Sha256>::new(None, &shared_bytes);

    let mut mask = vec![0u8; len];
    hk.expand(b"vault-pss-mask", &mut mask)
        .expect("hkdf expand should not fail for reasonable lengths");
    mask
}

#[derive(Debug)]
pub enum RecoveryError {
    InsufficientContributions { got: usize, need: usize },
    VerificationFailed(String),
    LagrangeError(String),
}

impl std::fmt::Display for RecoveryError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InsufficientContributions { got, need } => {
                write!(f, "insufficient contributions: got {}, need {}", got, need)
            }
            Self::VerificationFailed(msg) => write!(f, "verification failed: {}", msg),
            Self::LagrangeError(msg) => write!(f, "lagrange error: {}", msg),
        }
    }
}

impl std::error::Error for RecoveryError {}

/// hex serialization for RistrettoPoint
mod point_hex {
    use curve25519_dalek::ristretto::{CompressedRistretto, RistrettoPoint};
    use serde::{Deserialize, Deserializer, Serialize, Serializer};

    pub fn serialize<S>(point: &RistrettoPoint, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let hex_str = hex::encode(point.compress().as_bytes());
        hex_str.serialize(serializer)
    }

    pub fn deserialize<'de, D>(deserializer: D) -> Result<RistrettoPoint, D::Error>
    where
        D: Deserializer<'de>,
    {
        let hex_str = String::deserialize(deserializer)?;
        let bytes = hex::decode(&hex_str).map_err(serde::de::Error::custom)?;
        if bytes.len() != 32 {
            return Err(serde::de::Error::custom("invalid point length"));
        }
        let arr: [u8; 32] = bytes.try_into().unwrap();
        let compressed = CompressedRistretto::from_slice(&arr)
            .map_err(|_| serde::de::Error::custom("invalid compressed point"))?;
        compressed.decompress()
            .ok_or_else(|| serde::de::Error::custom("point decompression failed"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use curve25519_dalek::constants::RISTRETTO_BASEPOINT_POINT;
    use rand::rngs::OsRng;

    /// simulate shamir split for testing
    fn shamir_split(secret: &Scalar, n: u32, t: u32) -> Vec<SecretShare<Scalar>> {
        let mut rng = OsRng;
        let mut coeffs = vec![*secret];
        for _ in 1..t {
            coeffs.push(Scalar::random(&mut rng));
        }

        (1..=n)
            .map(|i| {
                let x = Scalar::from(i);
                let mut y = Scalar::ZERO;
                let mut x_pow = Scalar::ONE;
                for coeff in &coeffs {
                    y += coeff * x_pow;
                    x_pow *= x;
                }
                SecretShare::new(i, y)
            })
            .collect()
    }

    #[test]
    fn test_encrypt_decrypt_direct() {
        let mut rng = OsRng;
        let secret = Scalar::random(&mut rng);
        let group_pubkey = RISTRETTO_BASEPOINT_POINT * secret;

        let data = b"hello vault pss";
        let ciphertext = GroupCiphertext::encrypt(&group_pubkey, data);

        // simulate direct decryption (with full secret)
        let shared = ciphertext.ephemeral * secret;
        let decrypted = ciphertext.decrypt(&shared);

        assert_eq!(decrypted, data);
    }

    #[test]
    fn test_threshold_recovery() {
        let mut rng = OsRng;

        // setup: 3-of-5 threshold
        let secret = Scalar::random(&mut rng);
        let group_pubkey = RISTRETTO_BASEPOINT_POINT * secret;
        let shares = shamir_split(&secret, 5, 3);

        // encrypt
        let data = b"threshold recovery test data";
        let ciphertext = GroupCiphertext::encrypt(&group_pubkey, data);

        // generate partial decryptions from 3 providers
        let payload = b"recovery-session-001";
        let partials: Vec<PartialDecryption> = shares[0..3]
            .iter()
            .map(|s| partial_decrypt(s, &ciphertext, payload))
            .collect();

        // combine and decrypt
        let decrypted = threshold_decrypt(
            &ciphertext,
            &partials,
            &group_pubkey,
            3,
            payload,
        ).unwrap();

        assert_eq!(decrypted, data);
    }

    #[test]
    fn test_threshold_recovery_non_consecutive() {
        let mut rng = OsRng;

        let secret = Scalar::random(&mut rng);
        let group_pubkey = RISTRETTO_BASEPOINT_POINT * secret;
        let shares = shamir_split(&secret, 5, 3);

        let data = b"non-consecutive shares";
        let ciphertext = GroupCiphertext::encrypt(&group_pubkey, data);

        // use shares 1, 3, 5 (non-consecutive)
        let payload = b"recovery-session-002";
        let partials: Vec<PartialDecryption> = [&shares[0], &shares[2], &shares[4]]
            .iter()
            .map(|s| partial_decrypt(s, &ciphertext, payload))
            .collect();

        let decrypted = threshold_decrypt(
            &ciphertext,
            &partials,
            &group_pubkey,
            3,
            payload,
        ).unwrap();

        assert_eq!(decrypted, data);
    }

    #[test]
    fn test_insufficient_contributions() {
        let mut rng = OsRng;

        let secret = Scalar::random(&mut rng);
        let group_pubkey = RISTRETTO_BASEPOINT_POINT * secret;
        let shares = shamir_split(&secret, 5, 3);

        let data = b"test";
        let ciphertext = GroupCiphertext::encrypt(&group_pubkey, data);

        // only 2 contributions (need 3)
        let payload = b"recovery-fail";
        let partials: Vec<PartialDecryption> = shares[0..2]
            .iter()
            .map(|s| partial_decrypt(s, &ciphertext, payload))
            .collect();

        let result = threshold_decrypt(
            &ciphertext,
            &partials,
            &group_pubkey,
            3,
            payload,
        );

        assert!(matches!(result, Err(RecoveryError::InsufficientContributions { .. })));
    }

    #[test]
    fn test_wrong_payload_fails() {
        let mut rng = OsRng;

        let secret = Scalar::random(&mut rng);
        let group_pubkey = RISTRETTO_BASEPOINT_POINT * secret;
        let shares = shamir_split(&secret, 5, 3);

        let data = b"test";
        let ciphertext = GroupCiphertext::encrypt(&group_pubkey, data);

        // contributions with correct payload
        let correct_payload = b"correct";
        let partials: Vec<PartialDecryption> = shares[0..3]
            .iter()
            .map(|s| partial_decrypt(s, &ciphertext, correct_payload))
            .collect();

        // verify with wrong payload
        let wrong_payload = b"wrong";
        let result = threshold_decrypt(
            &ciphertext,
            &partials,
            &group_pubkey,
            3,
            wrong_payload,
        );

        assert!(matches!(result, Err(RecoveryError::VerificationFailed(_))));
    }

    #[test]
    fn test_ciphertext_serialization() {
        let mut rng = OsRng;
        let secret = Scalar::random(&mut rng);
        let group_pubkey = RISTRETTO_BASEPOINT_POINT * secret;

        let data = b"serialization test";
        let ciphertext = GroupCiphertext::encrypt(&group_pubkey, data);

        // roundtrip through json
        let json = serde_json::to_string(&ciphertext).unwrap();
        let recovered: GroupCiphertext = serde_json::from_str(&json).unwrap();

        assert_eq!(ciphertext.ephemeral, recovered.ephemeral);
        assert_eq!(ciphertext.ciphertext, recovered.ciphertext);
    }
}
