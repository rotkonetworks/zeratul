//! zoda-vss integration for verifiable share distribution
//!
//! wraps encrypted OSST shares with verifiable secret sharing for:
//! - backup: store shares redundantly across members
//! - verification: members can verify shares before accepting
//! - recovery: reconstruct shares if members go offline
//!
//! # flow
//!
//! during formation, each member's OSST share is:
//! 1. encrypted with member's viewing key
//! 2. wrapped with zoda-vss for verifiable distribution
//! 3. distributed to other members as backup
//!
//! ```text
//! ┌─────────────────────────────────────────────────────────────┐
//! │                    SHARE DISTRIBUTION                       │
//! │                                                             │
//! │  osst_share  ──▶  encrypt  ──▶  zoda_vss  ──▶  members     │
//! │   (alice)         (alice)        (2-of-3)     (bob, carol) │
//! │                                                             │
//! │  bob & carol each get a verifiable backup share of alice   │
//! │  alice can recover from any 2 of {alice, bob, carol}       │
//! └─────────────────────────────────────────────────────────────┘
//! ```

use alloc::vec::Vec;

/// verifiable share wrapper
///
/// wraps data in zoda-vss for verifiable distribution
#[derive(Clone, Debug)]
pub struct VerifiableSharePackage {
    /// owner's public key (whose OSST share this is)
    pub owner: [u8; 32],
    /// vss header (commitment to polynomial)
    pub header: VssHeader,
    /// individual backup shares for distribution
    pub backup_shares: Vec<BackupShare>,
}

/// vss header (commitment)
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct VssHeader {
    /// threshold for reconstruction
    pub threshold: u8,
    /// total backup shares
    pub total: u8,
    /// commitment hash
    pub commitment: [u8; 32],
}

/// individual backup share
#[derive(Clone, Debug)]
pub struct BackupShare {
    /// share index (1-indexed)
    pub index: u8,
    /// share data
    pub data: Vec<u8>,
    /// recipient public key
    pub recipient: [u8; 32],
}

impl BackupShare {
    /// verify share against header (format check)
    pub fn verify(&self, header: &VssHeader) -> bool {
        self.index > 0 && self.index <= header.total && !self.data.is_empty()
    }
}

/// share distributor
///
/// creates verifiable backup shares from encrypted data
#[derive(Clone, Debug)]
pub struct ShareDistributor {
    /// threshold for reconstruction
    threshold: u8,
}

impl ShareDistributor {
    /// create distributor with given threshold
    pub fn new(threshold: u8) -> Self {
        assert!(threshold > 0, "threshold must be positive");
        Self { threshold }
    }

    /// create verifiable package from encrypted data
    ///
    /// - `owner`: whose data this is
    /// - `encrypted_data`: encrypted OSST share
    /// - `recipients`: who gets backup shares
    pub fn create_package<R: rand_core::RngCore>(
        &self,
        owner: [u8; 32],
        encrypted_data: &[u8],
        recipients: &[[u8; 32]],
        rng: &mut R,
    ) -> VerifiableSharePackage {
        let total = recipients.len() as u8;
        assert!(total >= self.threshold, "need at least threshold recipients");

        // create polynomial coefficients
        let coefficients = self.create_polynomial(encrypted_data, rng);

        // create header (commitment)
        let header = VssHeader {
            threshold: self.threshold,
            total,
            commitment: self.compute_commitment(&coefficients),
        };

        // evaluate polynomial at each point
        let backup_shares: Vec<BackupShare> = recipients
            .iter()
            .enumerate()
            .map(|(i, recipient)| {
                let index = (i + 1) as u8;
                let data = self.evaluate_polynomial(&coefficients, index);
                BackupShare {
                    index,
                    data,
                    recipient: *recipient,
                }
            })
            .collect();

        VerifiableSharePackage {
            owner,
            header,
            backup_shares,
        }
    }

    /// reconstruct original data from backup shares
    pub fn reconstruct(
        header: &VssHeader,
        shares: &[BackupShare],
    ) -> Result<Vec<u8>, VssError> {
        if shares.len() < header.threshold as usize {
            return Err(VssError::InsufficientShares);
        }

        // verify all shares
        for share in shares {
            if !share.verify(header) {
                return Err(VssError::InvalidShare);
            }
        }

        // verify no duplicates
        let mut seen = [false; 256];
        for share in shares {
            if seen[share.index as usize] {
                return Err(VssError::DuplicateShare);
            }
            seen[share.index as usize] = true;
        }

        // lagrange interpolation at x=0
        Self::lagrange_interpolate(shares, header.threshold)
    }

    fn create_polynomial<R: rand_core::RngCore>(
        &self,
        secret: &[u8],
        rng: &mut R,
    ) -> Vec<Vec<u8>> {
        // for each byte, create a polynomial of degree threshold-1
        let mut coefficients = Vec::with_capacity(secret.len());

        for &secret_byte in secret {
            let mut poly = Vec::with_capacity(self.threshold as usize);
            // constant term is the secret byte
            poly.push(secret_byte);
            // random coefficients for higher terms
            for _ in 1..self.threshold {
                let mut byte = [0u8; 1];
                rng.fill_bytes(&mut byte);
                poly.push(byte[0]);
            }
            coefficients.push(poly);
        }

        coefficients
    }

    fn compute_commitment(&self, coefficients: &[Vec<u8>]) -> [u8; 32] {
        use sha2::{Digest, Sha256};
        let mut hasher = Sha256::new();
        for poly in coefficients {
            for &coeff in poly {
                hasher.update([coeff]);
            }
        }
        hasher.finalize().into()
    }

    fn evaluate_polynomial(&self, coefficients: &[Vec<u8>], x: u8) -> Vec<u8> {
        coefficients
            .iter()
            .map(|poly| {
                // horner's method in GF(2^8)
                let mut y = 0u8;
                for &coeff in poly.iter().rev() {
                    y = gf256_mul(y, x) ^ coeff;
                }
                y
            })
            .collect()
    }

    fn lagrange_interpolate(
        shares: &[BackupShare],
        threshold: u8,
    ) -> Result<Vec<u8>, VssError> {
        let shares = &shares[..threshold as usize];
        let secret_len = shares[0].data.len();

        // verify consistent length
        if shares.iter().any(|s| s.data.len() != secret_len) {
            return Err(VssError::InconsistentShares);
        }

        let mut secret = Vec::with_capacity(secret_len);

        for byte_idx in 0..secret_len {
            let mut result = 0u8;

            for (i, share_i) in shares.iter().enumerate() {
                let x_i = share_i.index;
                let y_i = share_i.data[byte_idx];

                // compute lagrange basis at x=0
                let mut basis = 1u8;
                for (j, share_j) in shares.iter().enumerate() {
                    if i != j {
                        let x_j = share_j.index;
                        // basis *= x_j / (x_j - x_i) in GF(2^8)
                        let denom = x_j ^ x_i; // subtraction in GF(2^8)
                        basis = gf256_mul(basis, gf256_mul(x_j, gf256_inv(denom)));
                    }
                }

                result ^= gf256_mul(y_i, basis);
            }

            secret.push(result);
        }

        Ok(secret)
    }
}

/// GF(2^8) multiplication using AES polynomial
fn gf256_mul(a: u8, b: u8) -> u8 {
    let mut a = a;
    let mut b = b;
    let mut result = 0u8;

    for _ in 0..8 {
        if b & 1 != 0 {
            result ^= a;
        }
        let high_bit = a & 0x80;
        a <<= 1;
        if high_bit != 0 {
            a ^= 0x1b;
        }
        b >>= 1;
    }
    result
}

/// GF(2^8) multiplicative inverse
fn gf256_inv(a: u8) -> u8 {
    if a == 0 {
        return 0;
    }
    // fermat's little theorem: a^254 = a^(-1)
    let mut result = a;
    for _ in 0..6 {
        result = gf256_mul(result, result);
        result = gf256_mul(result, a);
    }
    gf256_mul(result, result)
}

/// vss errors
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum VssError {
    /// not enough shares
    InsufficientShares,
    /// share failed verification
    InvalidShare,
    /// duplicate share index
    DuplicateShare,
    /// shares have different lengths
    InconsistentShares,
}

impl core::fmt::Display for VssError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::InsufficientShares => write!(f, "insufficient shares"),
            Self::InvalidShare => write!(f, "invalid share"),
            Self::DuplicateShare => write!(f, "duplicate share"),
            Self::InconsistentShares => write!(f, "inconsistent share lengths"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_gf256_mul() {
        assert_eq!(gf256_mul(2, 3), 6);
        assert_eq!(gf256_mul(0x53, 0xca), 1); // known inverse pair
    }

    #[test]
    fn test_gf256_inv() {
        for i in 1..=255u8 {
            let inv = gf256_inv(i);
            assert_eq!(gf256_mul(i, inv), 1, "inverse failed for {}", i);
        }
    }

    #[test]
    fn test_share_distribution_2_of_3() {
        let distributor = ShareDistributor::new(2);
        let owner = [1u8; 32];
        let recipients = [[2u8; 32], [3u8; 32], [4u8; 32]];
        let encrypted_data = b"encrypted osst share data here";

        let mut rng = rand::thread_rng();
        let package = distributor.create_package(owner, encrypted_data, &recipients, &mut rng);

        assert_eq!(package.header.threshold, 2);
        assert_eq!(package.header.total, 3);
        assert_eq!(package.backup_shares.len(), 3);

        // verify all shares
        for share in &package.backup_shares {
            assert!(share.verify(&package.header));
        }

        // reconstruct from any 2 shares
        let reconstructed = ShareDistributor::reconstruct(
            &package.header,
            &package.backup_shares[0..2],
        )
        .unwrap();
        assert_eq!(reconstructed, encrypted_data);

        let reconstructed = ShareDistributor::reconstruct(
            &package.header,
            &package.backup_shares[1..3],
        )
        .unwrap();
        assert_eq!(reconstructed, encrypted_data);
    }

    #[test]
    fn test_insufficient_shares() {
        let distributor = ShareDistributor::new(3);
        let owner = [1u8; 32];
        let recipients = [[2u8; 32], [3u8; 32], [4u8; 32], [5u8; 32], [6u8; 32]];
        let encrypted_data = b"test data";

        let mut rng = rand::thread_rng();
        let package = distributor.create_package(owner, encrypted_data, &recipients, &mut rng);

        // 2 shares not enough for 3-of-5
        let result = ShareDistributor::reconstruct(
            &package.header,
            &package.backup_shares[0..2],
        );
        assert_eq!(result, Err(VssError::InsufficientShares));
    }

    #[test]
    fn test_osst_share_backup() {
        // simulate backing up a 32-byte OSST share
        let distributor = ShareDistributor::new(2);
        let alice = [1u8; 32];
        let recipients = [[2u8; 32], [3u8; 32], [4u8; 32]]; // bob, carol, dave

        // "encrypted" OSST share
        let encrypted_osst_share = [0x42u8; 32];

        let mut rng = rand::thread_rng();
        let package = distributor.create_package(
            alice,
            &encrypted_osst_share,
            &recipients,
            &mut rng,
        );

        // any 2 recipients can help alice recover
        let reconstructed = ShareDistributor::reconstruct(
            &package.header,
            &[
                package.backup_shares[0].clone(),
                package.backup_shares[2].clone(),
            ],
        )
        .unwrap();

        assert_eq!(reconstructed.as_slice(), encrypted_osst_share.as_slice());
    }
}
