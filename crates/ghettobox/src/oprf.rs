//! threshold oprf using ristretto255
//!
//! this is where the actual security comes from. servers evaluate
//! a threshold oprf on blinded inputs - they never see the pin,
//! can't brute force offline, and rate limiting happens server-side.
//!
//! protocol:
//!   client: r = random scalar
//!   client: blinded = hash_to_group(stretched_pin) * r
//!   client: send blinded to k servers
//!   server_i: response_i = blinded * share_i
//!   client: combined = lagrange_interpolate(responses)
//!   client: unlock_key = hash(combined * r^-1)
//!
//! security:
//!   - servers see only blinded values (random group elements)
//!   - client can't compute unlock_key without server participation
//!   - k-of-n threshold means k servers must cooperate
//!   - rate limiting enforced by each server independently

use curve25519_dalek::{
    constants::RISTRETTO_BASEPOINT_POINT,
    ristretto::{CompressedRistretto, RistrettoPoint},
    scalar::Scalar,
    traits::Identity,
};
use sha2::{Sha512, Digest};
use rand::RngCore;

use crate::{Error, Result};

/// compressed ristretto point (32 bytes)
pub type Point = [u8; 32];

/// scalar (32 bytes)
pub type ScalarBytes = [u8; 32];

/// DLEQ proof: proves log_G(public_key) == log_blinded(response)
/// this proves the server computed response = blinded * secret correctly
/// without revealing secret
#[derive(Clone, Debug)]
pub struct DleqProof {
    /// challenge (stored as bytes for serde)
    c: Scalar,
    /// response (stored as bytes for serde)
    s: Scalar,
}

impl serde::Serialize for DleqProof {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error> {
        serializer.serialize_bytes(&self.to_bytes())
    }
}

impl<'de> serde::Deserialize<'de> for DleqProof {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> std::result::Result<Self, D::Error> {
        let bytes: Vec<u8> = serde::Deserialize::deserialize(deserializer)?;
        if bytes.len() != 64 {
            return Err(serde::de::Error::custom("invalid DLEQ proof length"));
        }
        let mut arr = [0u8; 64];
        arr.copy_from_slice(&bytes);
        Ok(Self::from_bytes(&arr))
    }
}

impl DleqProof {
    /// create DLEQ proof that log_G(pk) == log_blinded(response)
    /// i.e., pk = G * secret and response = blinded * secret
    pub fn create(
        secret: &Scalar,
        blinded: &RistrettoPoint,
        response: &RistrettoPoint,
        public_key: &RistrettoPoint,
    ) -> Self {
        let mut rng = rand::thread_rng();
        let g = RISTRETTO_BASEPOINT_POINT;

        // commitment: pick random k
        let mut k_bytes = [0u8; 64];
        rng.fill_bytes(&mut k_bytes);
        let k = Scalar::from_bytes_mod_order_wide(&k_bytes);

        // R1 = G * k, R2 = blinded * k
        let r1 = g * k;
        let r2 = blinded * k;

        // challenge: c = H(G, pk, blinded, response, R1, R2)
        let c = Self::challenge(&g, public_key, blinded, response, &r1, &r2);

        // response: s = k - c * secret
        let s = k - c * secret;

        Self { c, s }
    }

    /// verify DLEQ proof
    pub fn verify(
        &self,
        blinded: &RistrettoPoint,
        response: &RistrettoPoint,
        public_key: &RistrettoPoint,
    ) -> bool {
        let g = RISTRETTO_BASEPOINT_POINT;

        // recompute R1 = G * s + pk * c
        let r1 = g * self.s + public_key * self.c;

        // recompute R2 = blinded * s + response * c
        let r2 = blinded * self.s + response * self.c;

        // recompute challenge
        let c_check = Self::challenge(&g, public_key, blinded, response, &r1, &r2);

        // verify c == c_check
        self.c == c_check
    }

    /// compute challenge hash
    fn challenge(
        g: &RistrettoPoint,
        pk: &RistrettoPoint,
        blinded: &RistrettoPoint,
        response: &RistrettoPoint,
        r1: &RistrettoPoint,
        r2: &RistrettoPoint,
    ) -> Scalar {
        let mut hasher = Sha512::new();
        hasher.update(b"ghettobox:dleq:v1");
        hasher.update(g.compress().as_bytes());
        hasher.update(pk.compress().as_bytes());
        hasher.update(blinded.compress().as_bytes());
        hasher.update(response.compress().as_bytes());
        hasher.update(r1.compress().as_bytes());
        hasher.update(r2.compress().as_bytes());
        let hash = hasher.finalize();

        let mut wide = [0u8; 64];
        wide.copy_from_slice(&hash);
        Scalar::from_bytes_mod_order_wide(&wide)
    }

    /// serialize proof to bytes (64 bytes)
    pub fn to_bytes(&self) -> [u8; 64] {
        let mut out = [0u8; 64];
        out[..32].copy_from_slice(&self.c.to_bytes());
        out[32..].copy_from_slice(&self.s.to_bytes());
        out
    }

    /// deserialize proof from bytes
    pub fn from_bytes(bytes: &[u8; 64]) -> Self {
        let c = Scalar::from_bytes_mod_order(bytes[..32].try_into().unwrap());
        let s = Scalar::from_bytes_mod_order(bytes[32..].try_into().unwrap());
        Self { c, s }
    }
}

/// OPRF evaluation with proof
#[derive(Clone, Debug)]
pub struct OprfResponse {
    /// evaluated point
    pub point: Point,
    /// DLEQ proof of correct evaluation
    pub proof: DleqProof,
}

/// oprf client state (holds blinding factor)
pub struct OprfClient {
    /// blinding scalar (secret, never sent to server)
    r: Scalar,
    /// blinded input (sent to servers)
    blinded: RistrettoPoint,
}

impl OprfClient {
    /// create new oprf client with blinded input
    ///
    /// input should be stretched pin (argon2id output)
    pub fn new(input: &[u8]) -> Self {
        let mut rng = rand::thread_rng();

        // hash input to group element
        let point = hash_to_ristretto(input);

        // generate random blinding scalar
        let mut r_bytes = [0u8; 64];
        rng.fill_bytes(&mut r_bytes);
        let r = Scalar::from_bytes_mod_order_wide(&r_bytes);

        // blind: B = H(input) * r
        let blinded = point * r;

        Self { r, blinded }
    }

    /// get blinded point to send to servers
    pub fn blinded_point(&self) -> Point {
        self.blinded.compress().to_bytes()
    }

    /// finalize oprf after receiving server responses (unverified)
    ///
    /// responses: vec of (server_index, response_point)
    /// threshold: minimum responses needed
    ///
    /// returns unlock_key (32 bytes)
    pub fn finalize(&self, responses: &[(u8, Point)], threshold: usize) -> Result<[u8; 32]> {
        if responses.len() < threshold {
            return Err(Error::NotEnoughShares {
                have: responses.len(),
                need: threshold,
            });
        }

        // decompress response points
        let points: Vec<(u8, RistrettoPoint)> = responses
            .iter()
            .take(threshold)
            .map(|(idx, bytes)| {
                let point = CompressedRistretto::from_slice(bytes)
                    .map_err(|_| Error::InvalidPoint)?
                    .decompress()
                    .ok_or(Error::InvalidPoint)?;
                Ok((*idx, point))
            })
            .collect::<Result<Vec<_>>>()?;

        // lagrange interpolation in the exponent
        let combined = lagrange_interpolate_points(&points);

        // unblind: result = combined * r^-1
        let r_inv = self.r.invert();
        let unblinded = combined * r_inv;

        // derive unlock key
        let mut hasher = Sha512::new();
        hasher.update(b"ghettobox:oprf:v1");
        hasher.update(unblinded.compress().as_bytes());
        let hash = hasher.finalize();

        let mut unlock_key = [0u8; 32];
        unlock_key.copy_from_slice(&hash[..32]);

        Ok(unlock_key)
    }

    /// finalize oprf with DLEQ proof verification
    ///
    /// responses: vec of (server_index, response_with_proof)
    /// public_keys: map of server_index -> public_key for verification
    /// threshold: minimum responses needed
    ///
    /// returns unlock_key (32 bytes), or error if any proof fails
    pub fn finalize_verified(
        &self,
        responses: &[(u8, OprfResponse)],
        public_keys: &[(u8, Point)],
        threshold: usize,
    ) -> Result<[u8; 32]> {
        if responses.len() < threshold {
            return Err(Error::NotEnoughShares {
                have: responses.len(),
                need: threshold,
            });
        }

        let blinded_point = self.blinded;

        // verify each proof and collect points
        let points: Vec<(u8, RistrettoPoint)> = responses
            .iter()
            .take(threshold)
            .map(|(idx, resp)| {
                // find public key for this server
                let pk_bytes = public_keys
                    .iter()
                    .find(|(i, _)| *i == *idx)
                    .map(|(_, pk)| pk)
                    .ok_or_else(|| Error::VssFailed(format!("no public key for server {}", idx)))?;

                let public_key = CompressedRistretto::from_slice(pk_bytes)
                    .map_err(|_| Error::InvalidPoint)?
                    .decompress()
                    .ok_or(Error::InvalidPoint)?;

                let response_point = CompressedRistretto::from_slice(&resp.point)
                    .map_err(|_| Error::InvalidPoint)?
                    .decompress()
                    .ok_or(Error::InvalidPoint)?;

                // verify DLEQ proof
                if !resp.proof.verify(&blinded_point, &response_point, &public_key) {
                    return Err(Error::VssFailed(format!(
                        "DLEQ proof verification failed for server {}",
                        idx
                    )));
                }

                Ok((*idx, response_point))
            })
            .collect::<Result<Vec<_>>>()?;

        // lagrange interpolation in the exponent
        let combined = lagrange_interpolate_points(&points);

        // unblind: result = combined * r^-1
        let r_inv = self.r.invert();
        let unblinded = combined * r_inv;

        // derive unlock key
        let mut hasher = Sha512::new();
        hasher.update(b"ghettobox:oprf:v1");
        hasher.update(unblinded.compress().as_bytes());
        let hash = hasher.finalize();

        let mut unlock_key = [0u8; 32];
        unlock_key.copy_from_slice(&hash[..32]);

        Ok(unlock_key)
    }
}

/// oprf server share
#[derive(Clone)]
pub struct OprfShare {
    /// server index (1-indexed, must be non-zero)
    pub index: u8,
    /// share of oprf secret key
    pub scalar: Scalar,
    /// public key for this share (G * scalar) - used for DLEQ verification
    pub public_key: RistrettoPoint,
}

impl OprfShare {
    /// create share from scalar and compute public key
    pub fn new(index: u8, scalar: Scalar) -> Self {
        let public_key = RISTRETTO_BASEPOINT_POINT * scalar;
        Self { index, scalar, public_key }
    }

    /// create share from bytes
    pub fn from_bytes(index: u8, bytes: &ScalarBytes) -> Self {
        let scalar = Scalar::from_bytes_mod_order(*bytes);
        Self::new(index, scalar)
    }

    /// serialize share to bytes
    pub fn to_bytes(&self) -> ScalarBytes {
        self.scalar.to_bytes()
    }

    /// get public key for this share
    pub fn public_key(&self) -> Point {
        self.public_key.compress().to_bytes()
    }

    /// evaluate oprf on blinded input (unverified - for backwards compat)
    ///
    /// this is what the server does - multiply blinded point by share
    pub fn evaluate(&self, blinded: &Point) -> Result<Point> {
        let point = CompressedRistretto::from_slice(blinded)
            .map_err(|_| Error::InvalidPoint)?
            .decompress()
            .ok_or(Error::InvalidPoint)?;

        let result = point * self.scalar;
        Ok(result.compress().to_bytes())
    }

    /// evaluate oprf with DLEQ proof of correctness
    ///
    /// returns response with proof that response = blinded * share
    pub fn evaluate_with_proof(&self, blinded: &Point) -> Result<OprfResponse> {
        let blinded_point = CompressedRistretto::from_slice(blinded)
            .map_err(|_| Error::InvalidPoint)?
            .decompress()
            .ok_or(Error::InvalidPoint)?;

        let response_point = blinded_point * self.scalar;

        let proof = DleqProof::create(
            &self.scalar,
            &blinded_point,
            &response_point,
            &self.public_key,
        );

        Ok(OprfResponse {
            point: response_point.compress().to_bytes(),
            proof,
        })
    }
}

/// dealer for creating oprf key shares (for setup)
pub struct OprfDealer;

impl OprfDealer {
    /// generate oprf secret and split into shares
    ///
    /// returns (public_key, shares)
    /// public_key can be used to verify setup (optional)
    pub fn deal(threshold: usize, total: usize) -> Result<(Point, Vec<OprfShare>)> {
        if threshold > total || threshold == 0 {
            return Err(Error::InvalidThreshold);
        }

        let mut rng = rand::thread_rng();

        // generate random polynomial coefficients
        // f(x) = a_0 + a_1*x + a_2*x^2 + ... + a_{t-1}*x^{t-1}
        // where a_0 is the secret
        let mut coeffs = Vec::with_capacity(threshold);
        for _ in 0..threshold {
            let mut bytes = [0u8; 64];
            rng.fill_bytes(&mut bytes);
            coeffs.push(Scalar::from_bytes_mod_order_wide(&bytes));
        }

        // public key = g^secret (for verification)
        let public_key = (RISTRETTO_BASEPOINT_POINT * coeffs[0]).compress().to_bytes();

        // evaluate polynomial at x = 1, 2, ..., n
        let shares: Vec<OprfShare> = (1..=total)
            .map(|i| {
                let x = Scalar::from(i as u64);
                let mut share = Scalar::ZERO;
                let mut x_power = Scalar::ONE;

                for coeff in &coeffs {
                    share += coeff * x_power;
                    x_power *= x;
                }

                OprfShare::new(i as u8, share)
            })
            .collect();

        Ok((public_key, shares))
    }
}

/// hash arbitrary bytes to ristretto point
fn hash_to_ristretto(input: &[u8]) -> RistrettoPoint {
    let mut hasher = Sha512::new();
    hasher.update(b"ghettobox:hash_to_group:v1");
    hasher.update(input);
    let hash = hasher.finalize();

    let mut wide = [0u8; 64];
    wide.copy_from_slice(&hash);

    RistrettoPoint::from_uniform_bytes(&wide)
}

/// lagrange interpolation for points (in the exponent)
fn lagrange_interpolate_points(points: &[(u8, RistrettoPoint)]) -> RistrettoPoint {
    let mut result = RistrettoPoint::identity();

    let indices: Vec<Scalar> = points
        .iter()
        .map(|(i, _)| Scalar::from(*i as u64))
        .collect();

    for (i, (_, point)) in points.iter().enumerate() {
        let lambda = lagrange_coefficient(i, &indices);
        result += point * lambda;
    }

    result
}

/// compute lagrange coefficient for index i
fn lagrange_coefficient(i: usize, indices: &[Scalar]) -> Scalar {
    let xi = indices[i];
    let mut num = Scalar::ONE;
    let mut den = Scalar::ONE;

    for (j, &xj) in indices.iter().enumerate() {
        if i != j {
            num *= Scalar::ZERO - xj;  // (0 - x_j)
            den *= xi - xj;             // (x_i - x_j)
        }
    }

    num * den.invert()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_oprf_roundtrip() {
        // setup: dealer creates 2-of-3 shares
        let (_, shares) = OprfDealer::deal(2, 3).unwrap();

        // client creates blinded input
        let input = b"stretched_pin_from_argon2id";
        let client = OprfClient::new(input);
        let blinded = client.blinded_point();

        // servers evaluate
        let resp1 = (shares[0].index, shares[0].evaluate(&blinded).unwrap());
        let resp2 = (shares[1].index, shares[1].evaluate(&blinded).unwrap());

        // client finalizes with 2 responses
        let key1 = client.finalize(&[resp1.clone(), resp2.clone()], 2).unwrap();

        // different client with same input should get same key
        let client2 = OprfClient::new(input);
        let blinded2 = client2.blinded_point();
        let resp1_2 = (shares[0].index, shares[0].evaluate(&blinded2).unwrap());
        let resp2_2 = (shares[1].index, shares[1].evaluate(&blinded2).unwrap());
        let key2 = client2.finalize(&[resp1_2, resp2_2], 2).unwrap();

        assert_eq!(key1, key2);
    }

    #[test]
    fn test_oprf_any_threshold_subset() {
        let (_, shares) = OprfDealer::deal(2, 3).unwrap();

        let input = b"test_input";
        let client = OprfClient::new(input);
        let blinded = client.blinded_point();

        // evaluate all shares
        let resps: Vec<_> = shares
            .iter()
            .map(|s| (s.index, s.evaluate(&blinded).unwrap()))
            .collect();

        // any 2 of 3 should give same result
        let key_01 = client.finalize(&[resps[0].clone(), resps[1].clone()], 2).unwrap();

        let client2 = OprfClient::new(input);
        let blinded2 = client2.blinded_point();
        let resps2: Vec<_> = shares
            .iter()
            .map(|s| (s.index, s.evaluate(&blinded2).unwrap()))
            .collect();
        let key_02 = client2.finalize(&[resps2[0].clone(), resps2[2].clone()], 2).unwrap();

        let client3 = OprfClient::new(input);
        let blinded3 = client3.blinded_point();
        let resps3: Vec<_> = shares
            .iter()
            .map(|s| (s.index, s.evaluate(&blinded3).unwrap()))
            .collect();
        let key_12 = client3.finalize(&[resps3[1].clone(), resps3[2].clone()], 2).unwrap();

        assert_eq!(key_01, key_02);
        assert_eq!(key_02, key_12);
    }

    #[test]
    fn test_different_input_different_key() {
        let (_, shares) = OprfDealer::deal(2, 3).unwrap();

        let client1 = OprfClient::new(b"input1");
        let blinded1 = client1.blinded_point();
        let resp1: Vec<_> = shares[..2]
            .iter()
            .map(|s| (s.index, s.evaluate(&blinded1).unwrap()))
            .collect();
        let key1 = client1.finalize(&resp1, 2).unwrap();

        let client2 = OprfClient::new(b"input2");
        let blinded2 = client2.blinded_point();
        let resp2: Vec<_> = shares[..2]
            .iter()
            .map(|s| (s.index, s.evaluate(&blinded2).unwrap()))
            .collect();
        let key2 = client2.finalize(&resp2, 2).unwrap();

        assert_ne!(key1, key2);
    }

    #[test]
    fn test_not_enough_responses() {
        let (_, shares) = OprfDealer::deal(2, 3).unwrap();

        let client = OprfClient::new(b"input");
        let blinded = client.blinded_point();
        let resp = (shares[0].index, shares[0].evaluate(&blinded).unwrap());

        // 1 response not enough for threshold 2
        let result = client.finalize(&[resp], 2);
        assert!(result.is_err());
    }

    #[test]
    fn test_dleq_proof_roundtrip() {
        let (_, shares) = OprfDealer::deal(2, 3).unwrap();

        let client = OprfClient::new(b"test_dleq");
        let blinded = client.blinded_point();

        // server evaluates with proof
        let resp = shares[0].evaluate_with_proof(&blinded).unwrap();

        // verify proof manually
        let blinded_point = CompressedRistretto::from_slice(&blinded)
            .unwrap()
            .decompress()
            .unwrap();
        let response_point = CompressedRistretto::from_slice(&resp.point)
            .unwrap()
            .decompress()
            .unwrap();

        assert!(resp.proof.verify(&blinded_point, &response_point, &shares[0].public_key));
    }

    #[test]
    fn test_verified_finalize() {
        let (_, shares) = OprfDealer::deal(2, 3).unwrap();

        let input = b"verified_oprf_test";
        let client = OprfClient::new(input);
        let blinded = client.blinded_point();

        // servers evaluate with proofs
        let resp1 = shares[0].evaluate_with_proof(&blinded).unwrap();
        let resp2 = shares[1].evaluate_with_proof(&blinded).unwrap();

        let responses = vec![
            (shares[0].index, resp1),
            (shares[1].index, resp2),
        ];

        let public_keys = vec![
            (shares[0].index, shares[0].public_key()),
            (shares[1].index, shares[1].public_key()),
        ];

        // verified finalize should succeed
        let key = client.finalize_verified(&responses, &public_keys, 2).unwrap();

        // unverified finalize with same inputs should give same key
        let client2 = OprfClient::new(input);
        let blinded2 = client2.blinded_point();
        let unverified_resps: Vec<_> = shares[..2]
            .iter()
            .map(|s| (s.index, s.evaluate(&blinded2).unwrap()))
            .collect();
        let key2 = client2.finalize(&unverified_resps, 2).unwrap();

        assert_eq!(key, key2);
    }

    #[test]
    fn test_bad_proof_rejected() {
        let (_, shares) = OprfDealer::deal(2, 3).unwrap();

        let client = OprfClient::new(b"bad_proof_test");
        let blinded = client.blinded_point();

        // get valid response from server 0
        let mut resp = shares[0].evaluate_with_proof(&blinded).unwrap();

        // corrupt the proof
        resp.proof.c = resp.proof.c + Scalar::ONE;

        let responses = vec![(shares[0].index, resp)];
        let public_keys = vec![(shares[0].index, shares[0].public_key())];

        // verification should fail
        let result = client.finalize_verified(&responses, &public_keys, 1);
        assert!(result.is_err());
    }

    #[test]
    fn test_wrong_server_proof_rejected() {
        let (_, shares) = OprfDealer::deal(2, 3).unwrap();

        let client = OprfClient::new(b"wrong_server_test");
        let blinded = client.blinded_point();

        // get response from server 0
        let resp = shares[0].evaluate_with_proof(&blinded).unwrap();

        // but claim it's from server 1 (wrong public key)
        let responses = vec![(shares[0].index, resp)];
        let public_keys = vec![(shares[0].index, shares[1].public_key())]; // wrong!

        // verification should fail
        let result = client.finalize_verified(&responses, &public_keys, 1);
        assert!(result.is_err());
    }
}
