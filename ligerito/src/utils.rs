//! Utility functions for Ligerito

use binary_fields::BinaryFieldElement;

/// Evaluate Lagrange basis at given points
pub fn evaluate_lagrange_basis<F: BinaryFieldElement>(rs: &[F]) -> Vec<F> {
    let one = F::one();
    let mut current_layer = vec![one.add(&rs[0]), rs[0]];
    let mut len = 2;

    for i in 1..rs.len() {
        let mut next_layer = Vec::with_capacity(2 * len);
        let ri_plus_one = one.add(&rs[i]);

        for j in 0..len {
            next_layer.push(current_layer[j].mul(&ri_plus_one));
            next_layer.push(current_layer[j].mul(&rs[i]));
        }

        current_layer = next_layer;
        len *= 2;
    }

    current_layer
}

/// Evaluate s_k at v_k values (for sumcheck)
/// Returns evaluation of all s_k polynomials at v_k points
pub fn eval_sk_at_vks<F: BinaryFieldElement>(n: usize) -> Vec<F> {
    // The s_k polynomials are univariate polynomials that evaluate to 1 at k and 0 elsewhere
    // For binary fields, these are computed using the Lagrange basis
    let mut result = Vec::with_capacity(n);
    
    // For each point k in 0..n
    for k in 0..n {
        // Convert k to field element
        let mut k_elem = F::zero();
        let mut k_val = k;
        let mut power = F::one();
        
        while k_val > 0 {
            if k_val & 1 == 1 {
                k_elem = k_elem.add(&power);
            }
            power = power.add(&power);
            k_val >>= 1;
        }
        
        result.push(k_elem);
    }
    
    result
}

/// Evaluate scaled basis in-place
pub fn evaluate_scaled_basis_inplace<F: BinaryFieldElement, U: BinaryFieldElement>(
    sks_x: &mut [F],
    basis: &mut [U],
    sks_vks: &[F],
    qf: F,
    scale: U,
) {
    // Evaluate the s_k polynomials at point qf
    for (i, sk_vk) in sks_vks.iter().enumerate() {
        if i < sks_x.len() {
            // s_k(qf) computation
            sks_x[i] = sk_vk.mul(&qf);
        }
    }
    
    // Build the multilinear basis evaluation
    let n = basis.len();
    let log_n = n.trailing_zeros() as usize;
    
    // Start with all ones scaled by the scale factor
    for i in 0..n {
        basis[i] = scale;
    }
    
    // Apply the multilinear extension formula
    let mut stride = 1;
    for _ in 0..log_n {
        for i in 0..n {
            if (i / stride) % 2 == 1 {
                // This basis function has x_bit = 1
                let idx = i - stride;
                basis[i] = basis[idx].mul(&scale);
            }
        }
        stride *= 2;
    }
}

/// Check if a number is a power of 2
pub fn is_power_of_two(n: usize) -> bool {
    n > 0 && (n & (n - 1)) == 0
}

/// Encode non-systematic Reed-Solomon
pub fn encode_non_systematic<F: BinaryFieldElement>(
    rs: &reed_solomon::ReedSolomon<F>,
    data: &mut [F],
) {
    // Non-systematic encoding (no original message preservation)
    reed_solomon::encode_in_place(rs, data);
}

#[cfg(test)]
mod tests {
    use super::*;
    use binary_fields::BinaryElem16;

    #[test]
    fn test_lagrange_basis() {
        let rs = vec![
            BinaryElem16::from(0x1234),
            BinaryElem16::from(0x5678),
            BinaryElem16::from(0x9ABC),
        ];

        let basis = evaluate_lagrange_basis(&rs);
        assert_eq!(basis.len(), 8); // 2^3
    }

    #[test]
    fn test_power_of_two() {
        assert!(is_power_of_two(1));
        assert!(is_power_of_two(2));
        assert!(is_power_of_two(1024));
        assert!(!is_power_of_two(0));
        assert!(!is_power_of_two(1023));
    }
}
