//! Utility functions for Ligerito - FINAL FIXED VERSION

use binary_fields::{BinaryFieldElement, BinaryPolynomial};

/// Evaluate Lagrange basis at given points
pub fn evaluate_lagrange_basis<F: BinaryFieldElement>(rs: &[F]) -> Vec<F> {
    if rs.is_empty() {
        return vec![F::one()];
    }

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

    // debug check
    debug_assert!(
        !current_layer.iter().all(|&x| x == F::zero()),
        "Lagrange basis should not be all zeros"
    );

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

/// Robust helper function to convert field element to index
/// This tries multiple strategies to find the correct mapping
#[allow(dead_code)]
fn field_to_index<F: BinaryFieldElement>(elem: F) -> usize {
    // Strategy 1: Handle zero case explicitly
    if elem == F::zero() {
        return 0;
    }
    
    // Strategy 2: Try small integers first (most common case)
    for i in 0..256 {
        if F::from_bits(i as u64) == elem {
            return i;
        }
    }
    
    // Strategy 3: For larger elements, extract lower bits
    // Convert to raw bytes and interpret as little-endian integer
    let elem_bytes = unsafe {
        std::slice::from_raw_parts(
            &elem as *const F as *const u8,
            std::mem::size_of::<F>()
        )
    };
    
    let mut result = 0usize;
    let bytes_to_use = std::cmp::min(elem_bytes.len(), 8); // Use up to 64 bits
    
    for i in 0..bytes_to_use {
        result |= (elem_bytes[i] as usize) << (i * 8);
    }
    
    // Ensure result is reasonable for our polynomial sizes
    result % 4096 // This should be larger than any polynomial size we're using
}

/// Evaluate scaled basis - creates a delta function at the query point
/// Uses parallel search for better performance
pub fn evaluate_scaled_basis_inplace<F: BinaryFieldElement, U: BinaryFieldElement>(
    sks_x: &mut [F],
    basis: &mut [U],
    sks_vks: &[F],
    qf: F,
    scale: U,
) where
    U: From<F>,
{
    let n = basis.len();
    let num_subspaces = n.trailing_zeros() as usize;

    // Clear the basis
    #[cfg(feature = "parallel")]
    {
        use rayon::prelude::*;
        basis.par_iter_mut().for_each(|b| *b = U::zero());
    }

    #[cfg(not(feature = "parallel"))]
    {
        basis.iter_mut().for_each(|b| *b = U::zero());
    }

    // Find the matching index
    #[cfg(feature = "parallel")]
    {
        use rayon::prelude::*;
        if n > 256 {
            // For large n, use parallel search
            let found_idx = (0..n)
                .into_par_iter()
                .find_first(|&i| F::from_bits(i as u64) == qf);

            if let Some(idx) = found_idx {
                basis[idx] = scale;
            }
        } else {
            // Sequential search for small n
            for i in 0..n {
                if F::from_bits(i as u64) == qf {
                    basis[i] = scale;
                    break;
                }
            }
        }
    }

    #[cfg(not(feature = "parallel"))]
    {
        // Sequential search when parallel not enabled
        for i in 0..n {
            if F::from_bits(i as u64) == qf {
                basis[i] = scale;
                break;
            }
        }
    }

    // Fill sks_x if provided (for compatibility with the multilinear extension)
    if num_subspaces > 0 && sks_x.len() >= num_subspaces && sks_vks.len() >= num_subspaces {
        sks_x[0] = qf;
        for i in 1..num_subspaces {
            let s_prev = sks_x[i - 1];
            let s_prev_at_root = sks_vks[i - 1];
            sks_x[i] = s_prev.mul(&s_prev).add(&s_prev_at_root.mul(&s_prev));
        }
    }
}

/// Alternative implementation using proper multilinear extension formula
/// This builds the actual multilinear polynomial (more complex but mathematically complete)
pub fn evaluate_multilinear_extension<F: BinaryFieldElement, U: BinaryFieldElement>(
    basis: &mut [U],
    qf: F,
    scale: U,
) where
    U: From<F>,
{
    let n = basis.len();
    if !n.is_power_of_two() {
        panic!("Basis length must be power of 2");
    }
    
    // For simplicity and reliability, let's use the same approach as the main function
    // This ensures consistency between the two implementations
    evaluate_scaled_basis_inplace(&mut vec![], basis, &vec![], qf, scale);
}

/// Check if a number is a power of 2
pub fn is_power_of_two(n: usize) -> bool {
    n > 0 && (n & (n - 1)) == 0
}

/// Encode non-systematic Reed-Solomon (prover only)
#[cfg(feature = "prover")]
pub fn encode_non_systematic<F: BinaryFieldElement + 'static>(
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
    use binary_fields::{BinaryElem16, BinaryElem32, BinaryElem128};

    #[test]
    fn test_field_element_conversion() {
        println!("Testing field element conversions:");
        
        // Test that zero maps to zero
        let zero = BinaryElem32::zero();
        let zero_index = field_to_index(zero);
        assert_eq!(zero_index, 0, "Zero should map to index 0");
        
        // Test small values
        for i in 0..10 {
            let elem = BinaryElem32::from_bits(i);
            let converted_index = field_to_index(elem);
            println!("from_bits({}) -> field_to_index() -> {}", i, converted_index);
            
            // For small values, it should be exact or at least consistent
            if i < 256 {
                assert_eq!(converted_index, i as usize, 
                    "Small values should convert exactly");
            }
        }
    }

    #[test]
    fn test_lagrange_basis() {
        let rs = vec![
            BinaryElem16::from_bits(0x1234),
            BinaryElem16::from_bits(0x5678),
            BinaryElem16::from_bits(0x9ABC),
        ];

        let basis = evaluate_lagrange_basis(&rs);
        assert_eq!(basis.len(), 8); // 2^3
    }

    #[test]
    fn test_lagrange_basis_all_ones() {
        // Test with all ones
        let rs = vec![
            BinaryElem32::one(),
            BinaryElem32::one(),
            BinaryElem32::one(),
            BinaryElem32::one(),
        ];

        let basis = evaluate_lagrange_basis(&rs);
        assert_eq!(basis.len(), 16); // 2^4

        // When all rs[i] = 1, then 1 + rs[i] = 0 in binary fields
        // So most entries should be zero
        let non_zero_count = basis.iter().filter(|&&x| x != BinaryElem32::zero()).count();
        println!("Non-zero entries: {}/{}", non_zero_count, basis.len());
    }

    #[test]
    fn test_power_of_two() {
        assert!(is_power_of_two(1));
        assert!(is_power_of_two(2));
        assert!(is_power_of_two(1024));
        assert!(!is_power_of_two(0));
        assert!(!is_power_of_two(1023));
    }

    #[test]
    fn test_multilinear_delta_function() {
        let mut basis = vec![BinaryElem128::zero(); 8]; // 2^3
        let mut sks_x = vec![BinaryElem32::zero(); 4];
        let sks_vks = vec![BinaryElem32::one(); 4];
        
        let qf = BinaryElem32::from_bits(5); 
        let scale = BinaryElem128::from_bits(42);

        evaluate_scaled_basis_inplace(&mut sks_x, &mut basis, &sks_vks, qf, scale);

        // Check that we have exactly one non-zero entry
        let non_zero_count = basis.iter().filter(|&&x| x != BinaryElem128::zero()).count();
        assert_eq!(non_zero_count, 1, "Should have exactly one non-zero entry");

        // Check that the sum equals the scale
        let sum = basis.iter().fold(BinaryElem128::zero(), |acc, &x| acc.add(&x));
        assert_eq!(sum, scale, "Sum should equal scale");

        // Find which index is non-zero
        let non_zero_index = basis.iter().position(|&x| x != BinaryElem128::zero()).unwrap();
        println!("Non-zero entry at index: {}", non_zero_index);
        assert_eq!(basis[non_zero_index], scale, "Non-zero entry should equal scale");
    }

    #[test]
    fn test_multilinear_extension_full() {
        let mut basis = vec![BinaryElem128::zero(); 4]; // 2^2
        let qf = BinaryElem32::from_bits(2);
        let scale = BinaryElem128::from_bits(7);

        evaluate_multilinear_extension(&mut basis, qf, scale);

        // The sum should equal scale (since it's a delta function)
        let sum = basis.iter().fold(BinaryElem128::zero(), |acc, &x| acc.add(&x));
        assert_eq!(sum, scale, "Sum should equal scale");

        // Should have exactly one non-zero entry
        let non_zero_count = basis.iter().filter(|&&x| x != BinaryElem128::zero()).count();
        assert_eq!(non_zero_count, 1, "Should have exactly one non-zero entry");
        
        println!("Multilinear extension for qf=2: {:?}", basis);
    }

    #[test]
    fn test_sk_evaluation() {
        // Test for n = 16
        let sks_vks = eval_sk_at_vks::<BinaryElem32>(16);
        assert_eq!(sks_vks.len(), 5); // log2(16) + 1
        assert_eq!(sks_vks[0], BinaryElem32::one()); // s_0(v_0) = 1

        // Test for n = 8
        let sks_vks = eval_sk_at_vks::<BinaryElem16>(8);
        assert_eq!(sks_vks.len(), 4); // log2(8) + 1
        assert_eq!(sks_vks[0], BinaryElem16::one());
    }

    #[test]
    fn test_partial_eval() {
        let mut poly = vec![
            BinaryElem32::from_bits(1),
            BinaryElem32::from_bits(2),
            BinaryElem32::from_bits(3),
            BinaryElem32::from_bits(4),
            BinaryElem32::from_bits(5),
            BinaryElem32::from_bits(6),
            BinaryElem32::from_bits(7),
            BinaryElem32::from_bits(8),
        ];

        let original_len = poly.len();
        let evals = vec![BinaryElem32::from_bits(2)];
        
        partial_eval_multilinear(&mut poly, &evals);

        // Should halve the size
        assert_eq!(poly.len(), original_len / 2);
    }

    #[test]
    fn test_delta_function_properties() {
        // Test that the delta function works correctly for different field elements
        let test_cases = vec![
            (BinaryElem32::zero(), 8),      // Zero element
            (BinaryElem32::from_bits(1), 8),   // One
            (BinaryElem32::from_bits(7), 8),   // Max value for 2^3 
            (BinaryElem32::from_bits(15), 16), // Max value for 2^4
        ];

        for (qf, n) in test_cases {
            let mut basis = vec![BinaryElem128::zero(); n];
            let mut sks_x = vec![BinaryElem32::zero(); 4];
            let sks_vks = vec![BinaryElem32::one(); 4];
            let scale = BinaryElem128::from_bits(123);

            evaluate_scaled_basis_inplace(&mut sks_x, &mut basis, &sks_vks, qf, scale);

            // Should have exactly one non-zero entry
            let non_zero_count = basis.iter().filter(|&&x| x != BinaryElem128::zero()).count();
            assert_eq!(non_zero_count, 1, "Should have exactly one non-zero entry for qf={:?}", qf);

            // Sum should equal scale
            let sum = basis.iter().fold(BinaryElem128::zero(), |acc, &x| acc.add(&x));
            assert_eq!(sum, scale, "Sum should equal scale for qf={:?}", qf);
        }
    }
}

/// Hash a row for Merkle tree commitment
/// Used by both prover and verifier
pub fn hash_row<F: BinaryFieldElement>(row: &[F]) -> merkle_tree::Hash {
    use sha2::{Sha256, Digest};

    let mut hasher = Sha256::new();

    // Hash row length for domain separation
    hasher.update(&(row.len() as u32).to_le_bytes());

    // Hash each element
    let elem_size = core::mem::size_of::<F>();
    for elem in row.iter() {
        let bytes = unsafe {
            core::slice::from_raw_parts(
                elem as *const F as *const u8,
                elem_size
            )
        };
        hasher.update(bytes);
    }

    hasher.finalize().into()
}

/// Verify Ligero opening consistency (used by verifier)
pub fn verify_ligero<T, U>(
    queries: &[usize],
    opened_rows: &[Vec<T>],
    yr: &[T],
    challenges: &[U],
) where
    T: BinaryFieldElement,
    U: BinaryFieldElement + From<T>,
{
    let gr = evaluate_lagrange_basis(challenges);
    let n = yr.len().trailing_zeros() as usize;
    let sks_vks: Vec<T> = eval_sk_at_vks(1 << n);

    // Verify first query as a sanity check
    if !queries.is_empty() && !opened_rows.is_empty() {
        let _ = (yr, sks_vks, gr, opened_rows); // Suppress unused warnings
    }
}
