//! Additional types for OSST integration

use alloc::vec::Vec;

use crate::curve::OsstPoint;
use crate::Contribution;

/// Aggregated OSST proof ready for on-chain verification
#[derive(Clone, Debug)]
pub struct OsstProof<P: OsstPoint> {
    /// Collected contributions from threshold custodians
    pub contributions: Vec<Contribution<P>>,
    /// The payload that was authorized
    pub payload: Vec<u8>,
}

impl<P: OsstPoint> OsstProof<P> {
    pub fn new(contributions: Vec<Contribution<P>>, payload: Vec<u8>) -> Self {
        Self {
            contributions,
            payload,
        }
    }

    /// Verify this proof against a group public key
    pub fn verify(&self, group_pubkey: &P, threshold: u32) -> Result<bool, crate::OsstError> {
        crate::verify(group_pubkey, &self.contributions, threshold, &self.payload)
    }

    /// Number of contributions
    pub fn contribution_count(&self) -> usize {
        self.contributions.len()
    }

    /// Serialize for transmission
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut buf = Vec::new();

        // Payload length and data
        buf.extend_from_slice(&(self.payload.len() as u32).to_le_bytes());
        buf.extend_from_slice(&self.payload);

        // Number of contributions
        buf.extend_from_slice(&(self.contributions.len() as u32).to_le_bytes());

        // Each contribution
        for c in &self.contributions {
            buf.extend_from_slice(&c.to_bytes());
        }

        buf
    }

    /// Deserialize
    pub fn from_bytes(bytes: &[u8]) -> Result<Self, crate::OsstError> {
        if bytes.len() < 8 {
            return Err(crate::OsstError::InvalidCommitment);
        }

        let mut offset = 0;

        // Payload length
        let payload_len =
            u32::from_le_bytes(bytes[offset..offset + 4].try_into().unwrap()) as usize;
        offset += 4;

        if bytes.len() < offset + payload_len + 4 {
            return Err(crate::OsstError::InvalidCommitment);
        }

        // Payload
        let payload = bytes[offset..offset + payload_len].to_vec();
        offset += payload_len;

        // Number of contributions
        let num_contributions =
            u32::from_le_bytes(bytes[offset..offset + 4].try_into().unwrap()) as usize;
        offset += 4;

        // Check we have enough bytes
        let expected_len = offset + num_contributions * 68;
        if bytes.len() < expected_len {
            return Err(crate::OsstError::InvalidCommitment);
        }

        // Parse contributions
        let mut contributions = Vec::with_capacity(num_contributions);
        for _ in 0..num_contributions {
            let contrib_bytes: [u8; 68] = bytes[offset..offset + 68].try_into().unwrap();
            contributions.push(Contribution::<P>::from_bytes(&contrib_bytes)?);
            offset += 68;
        }

        Ok(Self {
            contributions,
            payload,
        })
    }
}

/// Builder for collecting OSST contributions
#[derive(Clone, Debug)]
pub struct OsstBuilder<P: OsstPoint> {
    contributions: Vec<Contribution<P>>,
    payload: Vec<u8>,
}

impl<P: OsstPoint> Default for OsstBuilder<P> {
    fn default() -> Self {
        Self {
            contributions: Vec::new(),
            payload: Vec::new(),
        }
    }
}

impl<P: OsstPoint> OsstBuilder<P> {
    pub fn new(payload: Vec<u8>) -> Self {
        Self {
            contributions: Vec::new(),
            payload,
        }
    }

    /// Add a contribution
    pub fn add(&mut self, contribution: Contribution<P>) -> Result<(), crate::OsstError> {
        // Check for duplicates
        for c in &self.contributions {
            if c.index == contribution.index {
                return Err(crate::OsstError::DuplicateIndex(contribution.index));
            }
        }
        self.contributions.push(contribution);
        Ok(())
    }

    /// Current number of contributions
    pub fn count(&self) -> usize {
        self.contributions.len()
    }

    /// Check if threshold is reached
    pub fn threshold_reached(&self, threshold: u32) -> bool {
        self.contributions.len() >= threshold as usize
    }

    /// Try to verify current contributions
    pub fn try_verify(&self, group_pubkey: &P, threshold: u32) -> Result<bool, crate::OsstError> {
        crate::verify(group_pubkey, &self.contributions, threshold, &self.payload)
    }

    /// Finalize into a proof
    pub fn finalize(self) -> OsstProof<P> {
        OsstProof {
            contributions: self.contributions,
            payload: self.payload,
        }
    }

    /// Get reference to contributions
    pub fn contributions(&self) -> &[Contribution<P>] {
        &self.contributions
    }

    /// Get reference to payload
    pub fn payload(&self) -> &[u8] {
        &self.payload
    }
}

#[cfg(all(test, feature = "ristretto255"))]
mod tests {
    use super::*;
    use crate::SecretShare;
    use curve25519_dalek::{ristretto::RistrettoPoint, scalar::Scalar};
    use rand::rngs::OsRng;

    use crate::curve::OsstPoint;

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
    fn test_builder() {
        let mut rng = OsRng;
        let secret = Scalar::random(&mut rng);
        let group_pubkey: RistrettoPoint = RistrettoPoint::generator().mul_scalar(&secret);

        let n = 5u32;
        let t = 3u32;
        let shares = shamir_split(&secret, n, t);

        let payload = b"builder test".to_vec();
        let mut builder: OsstBuilder<RistrettoPoint> = OsstBuilder::new(payload.clone());

        // Add contributions one by one
        for share in &shares[0..t as usize] {
            let contrib: Contribution<RistrettoPoint> = share.contribute(&mut rng, &payload);
            builder.add(contrib).unwrap();
        }

        assert!(builder.threshold_reached(t));
        assert!(builder.try_verify(&group_pubkey, t).unwrap());

        // Finalize
        let proof = builder.finalize();
        assert!(proof.verify(&group_pubkey, t).unwrap());
    }

    #[test]
    fn test_proof_serialization() {
        let mut rng = OsRng;
        let secret = Scalar::random(&mut rng);
        let group_pubkey: RistrettoPoint = RistrettoPoint::generator().mul_scalar(&secret);

        let n = 5u32;
        let t = 3u32;
        let shares = shamir_split(&secret, n, t);

        let payload = b"serialization test".to_vec();

        let contributions: Vec<Contribution<RistrettoPoint>> = shares[0..t as usize]
            .iter()
            .map(|s| s.contribute(&mut rng, &payload))
            .collect();

        let original: OsstProof<RistrettoPoint> = OsstProof::new(contributions, payload);

        // Serialize and deserialize
        let bytes = original.to_bytes();
        let recovered = OsstProof::<RistrettoPoint>::from_bytes(&bytes).unwrap();

        // Verify recovered proof
        assert!(recovered.verify(&group_pubkey, t).unwrap());
        assert_eq!(original.payload, recovered.payload);
        assert_eq!(
            original.contributions.len(),
            recovered.contributions.len()
        );
    }
}
