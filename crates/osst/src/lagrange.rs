//! Lagrange interpolation coefficient computation
//!
//! Efficient computation using the common denominator technique
//! from Section 6.4 of the OSST paper.
//!
//! Given a set Q = {i_1, ..., i_k}, the Lagrange coefficient for index i is:
//!
//! λ_i = Π_{j ∈ Q, j ≠ i} (j / (j - i))
//!
//! This can be rewritten as:
//!
//! λ_i = ξ · ρ_i · d̄^{-1}
//!
//! Where:
//! - ξ = Π_{j ∈ Q} j
//! - d_i = i · Π_{j ∈ Q, j ≠ i} (j - i)
//! - ρ_i = Π_{j ∈ Q, j ≠ i} d_j
//! - d̄ = Π_{i ∈ Q} d_i
//!
//! This requires only ONE modular inversion instead of k.

use alloc::vec;
use alloc::vec::Vec;

use crate::curve::OsstScalar;
use crate::error::OsstError;

/// Compute Lagrange interpolation coefficients for a set of indices.
///
/// Uses the common denominator technique for efficiency:
/// - O(k²) field multiplications
/// - O(1) modular inversions
///
/// # Arguments
///
/// * `indices` - The set Q of 1-indexed participant indices
///
/// # Returns
///
/// Vector of Lagrange coefficients λ_i for each index in the input order
pub fn compute_lagrange_coefficients<S: OsstScalar>(indices: &[u32]) -> Result<Vec<S>, OsstError> {
    let k = indices.len();
    if k == 0 {
        return Err(OsstError::EmptyContributions);
    }

    // Check all indices are positive
    for &idx in indices {
        if idx == 0 {
            return Err(OsstError::InvalidIndex);
        }
    }

    // Check for duplicates
    let mut sorted = indices.to_vec();
    sorted.sort();
    for i in 1..sorted.len() {
        if sorted[i] == sorted[i - 1] {
            return Err(OsstError::DuplicateIndex(sorted[i]));
        }
    }

    // Special case: single element
    if k == 1 {
        return Ok(vec![S::one()]);
    }

    // Convert indices to scalars
    let scalars: Vec<S> = indices.iter().map(|&i| S::from_u32(i)).collect();

    // Compute ξ = Π_{j ∈ Q} j
    let xi: S = scalars.iter().fold(S::one(), |acc, x| acc.mul(x));

    // Compute d_i = i · Π_{j ≠ i} (j - i)
    let mut d_values: Vec<S> = Vec::with_capacity(k);
    for i in 0..k {
        let mut d = scalars[i].clone();
        for j in 0..k {
            if i != j {
                let diff = scalars[j].sub(&scalars[i]);
                d = d.mul(&diff);
            }
        }
        d_values.push(d);
    }

    // Compute ρ_i using forward-backward pass
    // ρ_i = Π_{j ≠ i} d_j
    let mut rho: Vec<S> = vec![S::one(); k];

    // Forward pass: rho[i] = Π_{j < i} d_j
    for i in 1..k {
        rho[i] = rho[i - 1].mul(&d_values[i - 1]);
    }

    // Backward pass: multiply by Π_{j > i} d_j
    let mut suffix = S::one();
    for i in (0..k).rev() {
        rho[i] = rho[i].mul(&suffix);
        suffix = suffix.mul(&d_values[i]);
    }

    // d̄ = suffix after full backward pass = Π d_i
    let d_bar = suffix;

    // d̄^{-1} (single inversion)
    let d_bar_inv = d_bar.invert();

    // λ_i = ξ · ρ_i · d̄^{-1}
    let delta = xi.mul(&d_bar_inv);
    let coefficients: Vec<S> = rho.iter().map(|rho_i| delta.mul(rho_i)).collect();

    Ok(coefficients)
}

#[cfg(all(test, feature = "ristretto255"))]
mod tests {
    use super::*;
    use curve25519_dalek::scalar::Scalar;

    #[test]
    fn test_lagrange_single() {
        let coeffs = compute_lagrange_coefficients::<Scalar>(&[1]).unwrap();
        assert_eq!(coeffs.len(), 1);
        assert_eq!(coeffs[0], Scalar::ONE);
    }

    #[test]
    fn test_lagrange_two_points() {
        // For Q = {1, 2}, evaluating at x = 0:
        // λ_1 = 2 / (2 - 1) = 2
        // λ_2 = 1 / (1 - 2) = -1
        let coeffs = compute_lagrange_coefficients::<Scalar>(&[1, 2]).unwrap();

        assert_eq!(coeffs[0], Scalar::from(2u32));
        assert_eq!(coeffs[1], -Scalar::ONE);
    }

    #[test]
    fn test_lagrange_three_points() {
        // For Q = {1, 2, 3}, evaluating at x = 0:
        // λ_1 = (2 * 3) / ((2-1) * (3-1)) = 6 / 2 = 3
        // λ_2 = (1 * 3) / ((1-2) * (3-2)) = 3 / (-1) = -3
        // λ_3 = (1 * 2) / ((1-3) * (2-3)) = 2 / 2 = 1
        let coeffs = compute_lagrange_coefficients::<Scalar>(&[1, 2, 3]).unwrap();

        assert_eq!(coeffs[0], Scalar::from(3u32));
        assert_eq!(coeffs[1], -Scalar::from(3u32));
        assert_eq!(coeffs[2], Scalar::ONE);
    }

    #[test]
    fn test_lagrange_non_consecutive() {
        // For Q = {1, 3, 5}
        // λ_1 = (3 * 5) / ((3-1) * (5-1)) = 15 / 8
        // λ_3 = (1 * 5) / ((1-3) * (5-3)) = 5 / (-4) = -5/4
        // λ_5 = (1 * 3) / ((1-5) * (3-5)) = 3 / 8
        let coeffs = compute_lagrange_coefficients::<Scalar>(&[1, 3, 5]).unwrap();

        // Verify sum of coefficients * point values = secret at x=0
        // Using a test polynomial f(x) = 1 + 2x + 3x²
        // f(0) = 1, f(1) = 6, f(3) = 34, f(5) = 86
        let f_1 = Scalar::from(6u32);
        let f_3 = Scalar::from(34u32);
        let f_5 = Scalar::from(86u32);

        let interpolated = coeffs[0] * f_1 + coeffs[1] * f_3 + coeffs[2] * f_5;
        assert_eq!(interpolated, Scalar::ONE); // f(0) = 1
    }

    #[test]
    fn test_lagrange_duplicate_error() {
        let result = compute_lagrange_coefficients::<Scalar>(&[1, 2, 2]);
        assert!(matches!(result, Err(OsstError::DuplicateIndex(2))));
    }

    #[test]
    fn test_lagrange_zero_index_error() {
        let result = compute_lagrange_coefficients::<Scalar>(&[0, 1, 2]);
        assert!(matches!(result, Err(OsstError::InvalidIndex)));
    }

    #[test]
    fn test_lagrange_empty_error() {
        let result = compute_lagrange_coefficients::<Scalar>(&[]);
        assert!(matches!(result, Err(OsstError::EmptyContributions)));
    }

    #[test]
    fn test_lagrange_interpolation_property() {
        // Verify: Σ λ_i * f(i) = f(0) for any polynomial of degree < k
        // Using f(x) = 5 (constant)

        for k in 2..=10 {
            let indices: Vec<u32> = (1..=k).collect();
            let coeffs = compute_lagrange_coefficients::<Scalar>(&indices).unwrap();

            // Sum of coefficients for constant function
            let sum: Scalar = coeffs.iter().fold(Scalar::ZERO, |acc, c| acc + c);

            // For any constant function, interpolation gives same constant
            // Σ λ_i = 1 (partition of unity for interpolation at 0)
            assert_eq!(
                sum,
                Scalar::ONE,
                "sum of lagrange coeffs should be 1 for k={}",
                k
            );
        }
    }

    #[test]
    fn test_lagrange_large_set() {
        // Test with 100 participants (simulating realistic threshold)
        let indices: Vec<u32> = (1..=100).collect();
        let coeffs = compute_lagrange_coefficients::<Scalar>(&indices).unwrap();

        assert_eq!(coeffs.len(), 100);

        // Verify partition of unity
        let sum: Scalar = coeffs.iter().fold(Scalar::ZERO, |acc, c| acc + c);
        assert_eq!(sum, Scalar::ONE);
    }
}
