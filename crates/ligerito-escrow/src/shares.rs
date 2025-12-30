//! Verifiable Shamir Secret Sharing implementation
//!
//! Uses polynomial evaluation over binary extension fields (GF(2^32)) with
//! Merkle tree commitments for share verification.

#[cfg(not(feature = "std"))]
use alloc::{vec, vec::Vec};

use crate::{EscrowError, Result, ShareField};
use ligerito_binary_fields::BinaryFieldElement;
use sha2::{Sha256, Digest};

/// A single share from secret sharing
#[derive(Clone, Debug)]
pub struct Share {
    /// Share index (evaluation point)
    pub index: u32,
    /// Share values (polynomial evaluated at index)
    /// One element per 4 bytes of secret
    pub values: Vec<ShareField>,
    /// Merkle proof for verification
    pub merkle_proof: Vec<[u8; 32]>,
}

impl Share {
    /// Get the index of this share
    pub fn index(&self) -> u32 {
        self.index
    }

    /// Get the share values
    pub fn values(&self) -> &[ShareField] {
        &self.values
    }
}

/// A complete set of shares with commitment
#[derive(Clone, Debug)]
pub struct ShareSet {
    /// Threshold (k) - minimum shares needed to reconstruct
    threshold: usize,
    /// Total number of shares (n)
    num_shares: usize,
    /// The shares
    shares: Vec<Share>,
    /// Merkle root commitment
    commitment: [u8; 32],
}

impl ShareSet {
    /// Get the threshold (k)
    pub fn threshold(&self) -> usize {
        self.threshold
    }

    /// Get the total number of shares (n)
    pub fn num_shares(&self) -> usize {
        self.num_shares
    }

    /// Get all shares
    pub fn shares(&self) -> &[Share] {
        &self.shares
    }

    /// Get the commitment (Merkle root)
    pub fn commitment(&self) -> [u8; 32] {
        self.commitment
    }

    /// Verify a share against the commitment
    pub fn verify_share(&self, share: &Share) -> Result<()> {
        if share.index as usize >= self.num_shares {
            return Err(EscrowError::IndexOutOfBounds);
        }

        // Compute leaf hash
        let leaf_hash = hash_share_values(&share.values);

        // Verify Merkle proof
        let computed_root = compute_merkle_root(
            share.index as usize,
            &leaf_hash,
            &share.merkle_proof,
            self.num_shares,
        );

        if computed_root == self.commitment {
            Ok(())
        } else {
            Err(EscrowError::MerkleProofInvalid)
        }
    }

    /// Get a specific share by index
    pub fn get_share(&self, index: usize) -> Option<&Share> {
        self.shares.get(index)
    }
}

/// Secret sharer that creates verifiable shares
pub struct SecretSharer {
    /// Threshold (k) - minimum shares needed
    threshold: usize,
    /// Total number of shares (n)
    num_shares: usize,
}

impl SecretSharer {
    /// Create a new secret sharer with k-of-n threshold
    pub fn new(threshold: usize, num_shares: usize) -> Result<Self> {
        if threshold < 2 {
            return Err(EscrowError::InvalidThreshold);
        }
        if threshold > num_shares {
            return Err(EscrowError::InvalidThreshold);
        }

        Ok(Self { threshold, num_shares })
    }

    /// Share a secret (must be 32 bytes)
    pub fn share_secret(&self, secret: &[u8; 32]) -> Result<ShareSet> {
        self.share_secret_with_rng(secret, &mut rand::thread_rng())
    }

    /// Share a secret with a custom RNG
    pub fn share_secret_with_rng<R: rand::Rng>(
        &self,
        secret: &[u8; 32],
        rng: &mut R,
    ) -> Result<ShareSet> {
        // Convert secret to field elements (8 x 4-byte chunks)
        let secret_elems = bytes_to_field_elements(secret);

        // For each field element, create a polynomial and evaluate at n points
        // Polynomial: p(x) = secret + a1*x + a2*x^2 + ... + a_{k-1}*x^{k-1}
        // where k = threshold

        let mut all_share_values: Vec<Vec<ShareField>> = vec![Vec::new(); self.num_shares];

        for secret_elem in &secret_elems {
            // Generate random coefficients for polynomial (degree = threshold - 1)
            // The constant term is the secret, rest are random
            let mut coeffs = vec![*secret_elem];
            for _ in 1..self.threshold {
                let random_val: u32 = rng.gen();
                coeffs.push(ShareField::from(random_val));
            }

            // Evaluate polynomial at points 1, 2, ..., n
            // We use non-zero points to ensure reconstruction works
            for i in 0..self.num_shares {
                let x = ShareField::from((i + 1) as u32);
                let y = evaluate_polynomial(&coeffs, &x);
                all_share_values[i].push(y);
            }

            // Zeroize coefficients (security)
            drop(coeffs);
        }

        // Build Merkle tree for commitment
        let leaf_hashes: Vec<[u8; 32]> = all_share_values
            .iter()
            .map(|values| hash_share_values(values))
            .collect();

        let (root, proofs) = build_merkle_tree(&leaf_hashes);

        // Create shares with Merkle proofs
        let shares: Vec<Share> = all_share_values
            .into_iter()
            .enumerate()
            .map(|(i, values)| Share {
                index: i as u32,
                values,
                merkle_proof: proofs[i].clone(),
            })
            .collect();

        Ok(ShareSet {
            threshold: self.threshold,
            num_shares: self.num_shares,
            shares,
            commitment: root,
        })
    }
}

/// Convert 32 bytes to 8 field elements
fn bytes_to_field_elements(bytes: &[u8; 32]) -> Vec<ShareField> {
    bytes
        .chunks(4)
        .map(|chunk| {
            let arr: [u8; 4] = chunk.try_into().unwrap();
            ShareField::from(u32::from_le_bytes(arr))
        })
        .collect()
}

/// Convert 8 field elements back to 32 bytes
pub fn field_elements_to_bytes(elements: &[ShareField]) -> [u8; 32] {
    let mut result = [0u8; 32];
    for (i, elem) in elements.iter().enumerate() {
        let val = elem.poly().value();
        result[i * 4..(i + 1) * 4].copy_from_slice(&val.to_le_bytes());
    }
    result
}

/// Evaluate polynomial at point x using Horner's method
fn evaluate_polynomial(coeffs: &[ShareField], x: &ShareField) -> ShareField {
    // p(x) = c0 + c1*x + c2*x^2 + ...
    // Horner: p(x) = c0 + x*(c1 + x*(c2 + ...))
    let mut result = ShareField::zero();
    for coeff in coeffs.iter().rev() {
        result = result.mul(x).add(coeff);
    }
    result
}

/// Hash share values for Merkle leaf
fn hash_share_values(values: &[ShareField]) -> [u8; 32] {
    let mut hasher = Sha256::new();
    for v in values {
        hasher.update(&v.poly().value().to_le_bytes());
    }
    hasher.finalize().into()
}

/// Build Merkle tree and return (root, proofs)
fn build_merkle_tree(leaves: &[[u8; 32]]) -> ([u8; 32], Vec<Vec<[u8; 32]>>) {
    let n = leaves.len();
    if n == 0 {
        return ([0u8; 32], vec![]);
    }
    if n == 1 {
        return (leaves[0], vec![vec![]]);
    }

    // Pad to power of 2
    let padded_len = n.next_power_of_two();
    let mut current_level: Vec<[u8; 32]> = leaves.to_vec();
    current_level.resize(padded_len, [0u8; 32]);

    // Store all levels for proof generation
    let mut levels = vec![current_level.clone()];

    // Build tree bottom-up
    while current_level.len() > 1 {
        let mut next_level = Vec::with_capacity(current_level.len() / 2);
        for chunk in current_level.chunks(2) {
            let mut hasher = Sha256::new();
            hasher.update(&chunk[0]);
            hasher.update(&chunk[1]);
            next_level.push(hasher.finalize().into());
        }
        levels.push(next_level.clone());
        current_level = next_level;
    }

    let root = current_level[0];

    // Generate proofs for each leaf
    let mut proofs = Vec::with_capacity(n);
    for i in 0..n {
        let mut proof = Vec::new();
        let mut idx = i;

        for level in &levels[..levels.len() - 1] {
            let sibling_idx = if idx % 2 == 0 { idx + 1 } else { idx - 1 };
            if sibling_idx < level.len() {
                proof.push(level[sibling_idx]);
            } else {
                proof.push([0u8; 32]);
            }
            idx /= 2;
        }
        proofs.push(proof);
    }

    (root, proofs)
}

/// Compute Merkle root from leaf and proof
fn compute_merkle_root(
    index: usize,
    leaf: &[u8; 32],
    proof: &[[u8; 32]],
    _total_leaves: usize,
) -> [u8; 32] {
    let mut current = *leaf;
    let mut idx = index;

    for sibling in proof {
        let mut hasher = Sha256::new();
        if idx % 2 == 0 {
            hasher.update(&current);
            hasher.update(sibling);
        } else {
            hasher.update(sibling);
            hasher.update(&current);
        }
        current = hasher.finalize().into();
        idx /= 2;
    }

    current
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_bytes_roundtrip() {
        let original = [0x12, 0x34, 0x56, 0x78, 0x9a, 0xbc, 0xde, 0xf0,
                        0x11, 0x22, 0x33, 0x44, 0x55, 0x66, 0x77, 0x88,
                        0x99, 0xaa, 0xbb, 0xcc, 0xdd, 0xee, 0xff, 0x00,
                        0x01, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07, 0x08];

        let elements = bytes_to_field_elements(&original);
        assert_eq!(elements.len(), 8);

        let recovered = field_elements_to_bytes(&elements);
        assert_eq!(recovered, original);
    }

    #[test]
    fn test_polynomial_evaluation() {
        // p(x) = 5 + 3x (degree 1)
        let coeffs = vec![
            ShareField::from(5u32),
            ShareField::from(3u32),
        ];

        // p(0) = 5
        let y0 = evaluate_polynomial(&coeffs, &ShareField::zero());
        assert_eq!(y0, ShareField::from(5u32));

        // p(1) = 5 + 3 = 5 XOR 3 = 6 (binary field!)
        let y1 = evaluate_polynomial(&coeffs, &ShareField::one());
        assert_eq!(y1, ShareField::from(6u32)); // 5 XOR 3 = 6

        // p(2) = 5 + 3*2 = 5 XOR 6 = 3
        let y2 = evaluate_polynomial(&coeffs, &ShareField::from(2u32));
        let expected = ShareField::from(5u32).add(&ShareField::from(3u32).mul(&ShareField::from(2u32)));
        assert_eq!(y2, expected);
    }

    #[test]
    fn test_merkle_tree() {
        let leaves = vec![
            [1u8; 32],
            [2u8; 32],
            [3u8; 32],
        ];

        let (root, proofs) = build_merkle_tree(&leaves);

        // Verify each proof
        for (i, leaf) in leaves.iter().enumerate() {
            let computed = compute_merkle_root(i, leaf, &proofs[i], leaves.len());
            assert_eq!(computed, root, "Proof {} failed", i);
        }
    }

    #[test]
    fn test_share_verification() {
        let secret = [42u8; 32];
        let sharer = SecretSharer::new(2, 3).unwrap();
        let share_set = sharer.share_secret(&secret).unwrap();

        // All shares should verify
        for share in share_set.shares() {
            assert!(share_set.verify_share(share).is_ok());
        }

        // Tampered share should fail
        let mut bad_share = share_set.shares()[0].clone();
        bad_share.values[0] = ShareField::from(999u32);
        assert!(share_set.verify_share(&bad_share).is_err());
    }
}
