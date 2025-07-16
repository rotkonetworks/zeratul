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
    assert!(n.is_power_of_two());
    let num_subspaces = n.trailing_zeros() as usize;
    
    let mut sks_vks = vec![F::zero(); num_subspaces + 1];
    sks_vks[0] = F::one(); // s_0(v_0) = 1
    
    // Initialize with powers of 2: 2^1, 2^2, ..., 2^num_subspaces
    let mut layer: Vec<F> = (1..=num_subspaces)
        .map(|i| F::from_bits(1u64 << i))
        .collect();
    
    let mut cur_len = num_subspaces;
    
    for i in 0..num_subspaces {
        for j in 0..cur_len {
            let sk_at_vk = if j == 0 {
                // s_{i+1}(v_{i+1}) computation
                let val = layer[0].mul(&layer[0]).add(&sks_vks[i].mul(&layer[0]));
                sks_vks[i + 1] = val;
                val
            } else {
                layer[j].mul(&layer[j]).add(&sks_vks[i].mul(&layer[j]))
            };
            
            if j > 0 {
                layer[j - 1] = sk_at_vk;
            }
        }
        cur_len -= 1;
    }
    
    sks_vks
}

/// Evaluate scaled basis in-place
pub fn evaluate_scaled_basis_inplace<F: BinaryFieldElement, U: BinaryFieldElement>(
    _sks_x: &mut [F],
    basis: &mut [U],
    _sks_vks: &[F],
    qf: F,
    scale: U,
) where
    U: From<F>,
{
    let n = basis.len();
    let log_n = n.trailing_zeros() as usize;

    // Initialize basis with scale
    for i in 0..n {
        basis[i] = scale;
    }

    // Convert qf to U for computations
    let qf_u = U::from(qf);
    let one_u = U::one();

    // Build the multilinear basis
    let mut stride = 1;
    for _bit_idx in 0..log_n {
        for i in 0..n {
            if (i / stride) % 2 == 1 {
                // Multiply by qf for this bit
                basis[i] = basis[i].mul(&qf_u);
            } else {
                // Multiply by (1 + qf) for this bit (in binary fields, 1 - x = 1 + x)
                let one_plus_qf = one_u.add(&qf_u);
                basis[i] = basis[i].mul(&one_plus_qf);
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

/// Multilinear polynomial partial evaluation
pub fn partial_eval_multilinear<F: BinaryFieldElement>(
    poly: &mut Vec<F>, 
    evals: &[F]
) {
    let mut n = poly.len();

    for &e in evals {
        n /= 2;

        for i in 0..n {
            let p0 = poly[2 * i];
            let p1 = poly[2 * i + 1];
            poly[i] = p0.add(&e.mul(&p1.add(&p0)));
        }
    }

    poly.truncate(n);
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
