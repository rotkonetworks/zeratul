//! Lagrange interpolation for secret reconstruction
//!
//! Implements polynomial reconstruction over binary extension fields
//! to recover secrets from Shamir shares.

#[cfg(not(feature = "std"))]
use alloc::vec::Vec;

use crate::{EscrowError, Result, ShareField};
use crate::shares::{Share, field_elements_to_bytes};
use ligerito_binary_fields::BinaryFieldElement;

/// Reconstruct a secret from k shares using Lagrange interpolation
///
/// # Arguments
/// * `shares` - At least k shares (threshold) to reconstruct from
/// * `threshold` - The k value (minimum shares needed)
///
/// # Returns
/// The reconstructed 32-byte secret
pub fn reconstruct_secret(shares: &[Share], threshold: usize) -> Result<[u8; 32]> {
    if shares.len() < threshold {
        return Err(EscrowError::InsufficientShares {
            have: shares.len(),
            need: threshold,
        });
    }

    // Check for duplicate indices
    for i in 0..shares.len() {
        for j in (i + 1)..shares.len() {
            if shares[i].index == shares[j].index {
                return Err(EscrowError::DuplicateShareIndex);
            }
        }
    }

    // Use exactly threshold shares for reconstruction
    let shares_to_use = &shares[..threshold];

    // Get the x-coordinates (indices + 1, since we evaluate at 1, 2, ..., n)
    let x_coords: Vec<ShareField> = shares_to_use
        .iter()
        .map(|s| ShareField::from(s.index + 1))
        .collect();

    // Number of field elements per secret (32 bytes / 4 bytes per element = 8)
    let num_elements = shares_to_use[0].values.len();

    // Reconstruct each element independently
    let mut reconstructed = Vec::with_capacity(num_elements);

    for elem_idx in 0..num_elements {
        // Get the y-coordinates for this element
        let y_coords: Vec<ShareField> = shares_to_use
            .iter()
            .map(|s| s.values[elem_idx])
            .collect();

        // Lagrange interpolation to find f(0)
        let secret_elem = lagrange_interpolate_at_zero(&x_coords, &y_coords);
        reconstructed.push(secret_elem);
    }

    Ok(field_elements_to_bytes(&reconstructed))
}

/// Lagrange interpolation to evaluate polynomial at x = 0
///
/// Given points (x_0, y_0), (x_1, y_1), ..., (x_{k-1}, y_{k-1}),
/// compute f(0) where f is the unique polynomial of degree < k passing
/// through all points.
///
/// Formula: f(0) = Σ y_i * L_i(0)
/// where L_i(0) = Π_{j≠i} (0 - x_j) / (x_i - x_j)
///              = Π_{j≠i} x_j / (x_j - x_i)  [simplified for binary fields]
///
/// Note: In binary fields, subtraction = addition (XOR), so x_j - x_i = x_j + x_i
fn lagrange_interpolate_at_zero(x_coords: &[ShareField], y_coords: &[ShareField]) -> ShareField {
    let k = x_coords.len();
    let mut result = ShareField::zero();

    for i in 0..k {
        // Compute Lagrange basis polynomial L_i(0)
        let mut numerator = ShareField::one();
        let mut denominator = ShareField::one();

        for j in 0..k {
            if i != j {
                // In binary fields: subtraction = addition
                // L_i(0) = Π_{j≠i} (0 - x_j) / (x_i - x_j)
                //        = Π_{j≠i} x_j / (x_i + x_j)
                numerator = numerator.mul(&x_coords[j]);
                denominator = denominator.mul(&x_coords[i].add(&x_coords[j]));
            }
        }

        // L_i(0) = numerator / denominator = numerator * denominator^(-1)
        let basis = numerator.mul(&denominator.inv());

        // Add y_i * L_i(0) to result
        result = result.add(&y_coords[i].mul(&basis));
    }

    result
}

/// Reconstruct with explicit share indices (for when shares come from different sources)
pub fn reconstruct_from_indexed_shares(
    indexed_shares: &[(u32, Vec<ShareField>)],
    threshold: usize,
) -> Result<[u8; 32]> {
    if indexed_shares.len() < threshold {
        return Err(EscrowError::InsufficientShares {
            have: indexed_shares.len(),
            need: threshold,
        });
    }

    // Check for duplicate indices
    for i in 0..indexed_shares.len() {
        for j in (i + 1)..indexed_shares.len() {
            if indexed_shares[i].0 == indexed_shares[j].0 {
                return Err(EscrowError::DuplicateShareIndex);
            }
        }
    }

    let shares_to_use = &indexed_shares[..threshold];
    let x_coords: Vec<ShareField> = shares_to_use
        .iter()
        .map(|(idx, _)| ShareField::from(*idx + 1))
        .collect();

    let num_elements = shares_to_use[0].1.len();
    let mut reconstructed = Vec::with_capacity(num_elements);

    for elem_idx in 0..num_elements {
        let y_coords: Vec<ShareField> = shares_to_use
            .iter()
            .map(|(_, values)| values[elem_idx])
            .collect();

        let secret_elem = lagrange_interpolate_at_zero(&x_coords, &y_coords);
        reconstructed.push(secret_elem);
    }

    Ok(field_elements_to_bytes(&reconstructed))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_lagrange_interpolation() {
        // Test with simple polynomial p(x) = 5 + 3x over binary field
        // p(0) = 5
        // p(1) = 5 XOR 3 = 6
        // p(2) = 5 XOR (3 * 2) = 5 XOR 6 = 3

        let x = vec![
            ShareField::from(1u32),
            ShareField::from(2u32),
        ];
        let y = vec![
            ShareField::from(6u32), // p(1)
            ShareField::from(3u32), // p(2) = 5 XOR 6 = 3
        ];

        let result = lagrange_interpolate_at_zero(&x, &y);
        assert_eq!(result, ShareField::from(5u32));
    }

    #[test]
    fn test_lagrange_with_three_points() {
        // Quadratic polynomial p(x) = a + bx + cx^2
        // For threshold = 3, we need 3 points

        // Use p(x) = 1 + 2x + 3x^2 (in binary field operations)
        let a = ShareField::from(1u32);
        let b = ShareField::from(2u32);
        let c = ShareField::from(3u32);

        // Evaluate at x = 1, 2, 3
        let eval = |x: ShareField| -> ShareField {
            a.add(&b.mul(&x)).add(&c.mul(&x.mul(&x)))
        };

        let x1 = ShareField::from(1u32);
        let x2 = ShareField::from(2u32);
        let x3 = ShareField::from(3u32);

        let y1 = eval(x1);
        let y2 = eval(x2);
        let y3 = eval(x3);

        let result = lagrange_interpolate_at_zero(
            &[x1, x2, x3],
            &[y1, y2, y3],
        );

        // p(0) = a = 1
        assert_eq!(result, ShareField::from(1u32));
    }
}
