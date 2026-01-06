//! polynomial encoding for shuffle proofs
//!
//! encodes deck permutations as multilinear polynomials over binary fields

#[cfg(not(feature = "std"))]
use alloc::vec::Vec;

use ligerito_binary_fields::{BinaryElem32, BinaryElem128, BinaryFieldElement};

use crate::{Permutation, Result, ShuffleError};

/// encode a permutation as a polynomial
///
/// WARNING: this function directly encodes π(i) values in the polynomial.
/// when ligerito opens rows, the permutation is revealed to verifier.
/// use constraints::encode_grand_product_constraints for ZK proofs.
#[deprecated(note = "leaks permutation to verifier - use grand product")]
pub fn encode_permutation(
    perm: &Permutation,
    input_commitments: &[u64],
    output_commitments: &[u64],
) -> Result<Vec<BinaryElem32>> {
    let n = perm.len();
    if input_commitments.len() != n || output_commitments.len() != n {
        return Err(ShuffleError::DeckSizeMismatch {
            expected: n,
            got: input_commitments.len(),
        });
    }

    let padded_n = n.next_power_of_two();
    let mut poly = Vec::with_capacity(padded_n * 4);

    // WARNING: this encoding leaks π(i) to verifier on row opening
    for i in 0..n {
        let pi_i = perm.get(i);

        poly.push(BinaryElem32::from_bits(i as u64));
        poly.push(BinaryElem32::from_bits(pi_i as u64)); // LEAKS π(i)
        poly.push(BinaryElem32::from_bits(input_commitments[pi_i] & 0xFFFFFFFF));
        poly.push(BinaryElem32::from_bits(output_commitments[i] & 0xFFFFFFFF));
    }

    let target_len = poly.len().next_power_of_two();
    poly.resize(target_len, BinaryElem32::zero());

    Ok(poly)
}

/// encode deck state as commitment values from raw bytes
pub fn encode_deck_from_bytes(deck_bytes: &[&[u8]]) -> Vec<u64> {
    deck_bytes
        .iter()
        .map(|bytes| {
            // take first 8 bytes as u64
            let mut arr = [0u8; 8];
            let len = bytes.len().min(8);
            arr[..len].copy_from_slice(&bytes[..len]);
            u64::from_le_bytes(arr)
        })
        .collect()
}

/// encode full shuffle constraint polynomial
///
/// WARNING: this function leaks permutation indices and masking factors.
/// use constraints::encode_grand_product_constraints for ZK proofs.
#[deprecated(note = "leaks permutation and masks - use grand product")]
pub fn encode_shuffle_polynomial(
    perm: &Permutation,
    input_deck: &[(u64, u64)],
    output_deck: &[(u64, u64)],
    masking_factors: &[u64],
) -> Result<Vec<BinaryElem32>> {
    let n = perm.len();
    if input_deck.len() != n || output_deck.len() != n {
        return Err(ShuffleError::DeckSizeMismatch {
            expected: n,
            got: input_deck.len(),
        });
    }
    if masking_factors.len() != n {
        return Err(ShuffleError::DeckSizeMismatch {
            expected: n,
            got: masking_factors.len(),
        });
    }

    // 8 elements per card (leaky encoding)
    let elems_per_card = 8;
    let total_elems = n * elems_per_card;
    let padded_size = total_elems.next_power_of_two();

    let mut poly = Vec::with_capacity(padded_size);

    for i in 0..n {
        let pi_i = perm.get(i);
        let (in_c0, in_c1) = input_deck[pi_i];
        let (out_c0, out_c1) = output_deck[i];
        let mask = masking_factors[i];

        poly.push(BinaryElem32::from_bits(in_c0 & 0xFFFFFFFF));
        poly.push(BinaryElem32::from_bits(in_c1 & 0xFFFFFFFF));
        poly.push(BinaryElem32::from_bits(out_c0 & 0xFFFFFFFF));
        poly.push(BinaryElem32::from_bits(out_c1 & 0xFFFFFFFF));
        poly.push(BinaryElem32::from_bits(pi_i as u64));        // LEAKS π(i)
        poly.push(BinaryElem32::from_bits(mask & 0xFFFFFFFF));  // LEAKS mask
        poly.push(BinaryElem32::from_bits((i as u64) ^ (pi_i as u64)));
        poly.push(BinaryElem32::from_bits(0));
    }

    poly.resize(padded_size, BinaryElem32::zero());

    Ok(poly)
}

/// compute multilinear extension evaluation
///
/// given polynomial coefficients and a point, evaluate the MLE
pub fn evaluate_mle(poly: &[BinaryElem32], point: &[BinaryElem128]) -> BinaryElem128 {
    let n = poly.len();
    let num_vars = n.ilog2() as usize;

    assert_eq!(point.len(), num_vars, "point dimension mismatch");
    assert!(n.is_power_of_two(), "polynomial length must be power of 2");

    // compute MLE: f(x) = Σᵢ poly[i] · eq(x, i)
    // where eq(x, i) = Πⱼ (xⱼ · iⱼ + (1-xⱼ) · (1-iⱼ))

    let mut result = BinaryElem128::zero();

    for (i, &coeff) in poly.iter().enumerate() {
        let coeff_ext = BinaryElem128::from(coeff);

        // compute eq(point, i)
        let mut eq_val = BinaryElem128::one();
        for (j, &xj) in point.iter().enumerate() {
            let ij = ((i >> j) & 1) as u64;
            let ij_elem = BinaryElem128::from_bits(ij);

            // eq_j = xj * ij + (1 - xj) * (1 - ij)
            // in binary field: 1 - x = 1 + x (since -1 = 1)
            let one = BinaryElem128::one();
            let one_minus_xj = one.add(&xj);
            let one_minus_ij = one.add(&ij_elem);

            let term = xj.mul(&ij_elem).add(&one_minus_xj.mul(&one_minus_ij));
            eq_val = eq_val.mul(&term);
        }

        result = result.add(&coeff_ext.mul(&eq_val));
    }

    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[allow(deprecated)]
    fn test_permutation_encoding() {
        let perm = Permutation::new(vec![2, 0, 1]).unwrap();
        let input = vec![100, 200, 300];
        let output = vec![300, 100, 200]; // permuted

        let poly = encode_permutation(&perm, &input, &output).unwrap();
        assert!(poly.len().is_power_of_two());
    }

    #[test]
    #[allow(deprecated)]
    fn test_shuffle_polynomial() {
        let perm = Permutation::new(vec![1, 2, 0, 3]).unwrap();
        let input = vec![(10, 11), (20, 21), (30, 31), (40, 41)];
        let output = vec![(20, 21), (30, 31), (10, 11), (40, 41)];
        let masks = vec![1, 2, 3, 4];

        let poly = encode_shuffle_polynomial(&perm, &input, &output, &masks).unwrap();
        assert!(poly.len().is_power_of_two());
        assert!(poly.len() >= 32); // 4 cards * 8 elems
    }

    #[test]
    fn test_mle_evaluation() {
        // simple test: polynomial [1, 2, 3, 4] over 2 variables
        let poly = vec![
            BinaryElem32::from_bits(1),
            BinaryElem32::from_bits(2),
            BinaryElem32::from_bits(3),
            BinaryElem32::from_bits(4),
        ];

        let point = vec![BinaryElem128::zero(), BinaryElem128::zero()];

        // at (0, 0), should get poly[0] = 1
        let result = evaluate_mle(&poly, &point);
        assert_eq!(result, BinaryElem128::from_bits(1));
    }
}
