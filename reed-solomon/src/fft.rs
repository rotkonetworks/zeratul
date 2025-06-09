// reed-solomon/src/fft.rs

use binary_fields::BinaryFieldElement;
use rayon::prelude::*;

/// Compute twiddle factors for FFT
pub fn compute_twiddles<F: BinaryFieldElement>(log_n: usize, beta: F) -> Vec<F> {
    if log_n == 0 {
        return vec![];
    }

    let n = 1 << log_n;
    let mut twiddles = vec![F::zero(); n - 1];
    
    // Layer 0 computation
    let mut layer = vec![F::zero(); n / 2];
    let mut s_prev_at_root = F::one();
    
    // Compute initial layer
    for i in 0..(n/2) {
        // In Julia: beta + F((i-1) << 1), but Julia is 1-indexed
        // So (i-1) in Julia is i in Rust, and we shift by 1
        let mut l0i = beta;
        let bits = i << 1;
        l0i = l0i.add(&F::from_bits(bits as u64));
        layer[i] = l0i;
    }
    
    // Copy to twiddles (Julia: twiddles[write_at:end], Rust: 0-indexed)
    let write_at = n / 2;
    if write_at > 0 {
        let start = write_at - 1;
        let copy_len = layer.len().min(twiddles.len() - start);
        twiddles[start..start + copy_len].copy_from_slice(&layer[..copy_len]);
    }
    
    // Subsequent layers
    let mut write_at = write_at / 2;
    while write_at > 0 {
        let layer_len = write_at.min(layer.len() / 2);
        
        // Compute s_at_root
        let s_at_root = compute_s_at_root(&layer, s_prev_at_root);
        
        if s_at_root == F::zero() {
            break;
        }
        
        // Update layer values - take every other element
        for idx in 0..layer_len {
            let s_prev = layer[idx * 2];
            layer[idx] = next_s(s_prev, s_prev_at_root);
        }
        
        // Normalize and store
        let s_inv = s_at_root.inv();
        let start = write_at - 1;
        for i in 0..layer_len {
            if start + i < twiddles.len() {
                twiddles[start + i] = s_inv.mul(&layer[i]);
            }
        }
        
        s_prev_at_root = s_at_root;
        write_at /= 2;
    }
    
    twiddles
}

fn next_s<F: BinaryFieldElement>(s_prev: F, s_prev_at_root: F) -> F {
    s_prev.mul(&s_prev).add(&s_prev_at_root.mul(&s_prev))
}

fn compute_s_at_root<F: BinaryFieldElement>(layer: &[F], s_prev_at_root: F) -> F {
    // s_i(beta + v_{i+1}) - s_i(beta) = s_i(v_{i+1})
    next_s(layer[1].add(&layer[0]), s_prev_at_root)
}

/// FFT butterfly operation
fn fft_mul<F: BinaryFieldElement>(v: &mut [F], lambda: F) {
    let mid = v.len() / 2;
    let (u, w) = v.split_at_mut(mid);
    
    for i in 0..mid {
        let lambda_w = lambda.mul(&w[i]);
        u[i] = u[i].add(&lambda_w);
        w[i] = w[i].add(&u[i]); // Uses updated u[i]
    }
}

/// In-place FFT with twiddle factors
pub fn fft<F: BinaryFieldElement>(v: &mut [F], twiddles: &[F], parallel: bool) {
    assert!(v.len().is_power_of_two());
    
    if twiddles.is_empty() {
        return;
    }
    
    if parallel && v.len() >= 1024 {
        fft_parallel(v, twiddles, 1);
    } else {
        fft_sequential(v, twiddles, 1);
    }
}

fn fft_sequential<F: BinaryFieldElement>(v: &mut [F], twiddles: &[F], idx: usize) {
    if v.len() == 1 {
        return;
    }
    
    // Apply twiddle if available (0-indexed)
    if idx > 0 && idx <= twiddles.len() {
        fft_mul(v, twiddles[idx - 1]);
    }
    
    let mid = v.len() / 2;
    let (u, w) = v.split_at_mut(mid);
    
    fft_sequential(u, twiddles, 2 * idx);
    fft_sequential(w, twiddles, 2 * idx + 1);
}

fn fft_parallel<F: BinaryFieldElement>(v: &mut [F], twiddles: &[F], idx: usize) {
    if v.len() == 1 {
        return;
    }
    
    if idx > 0 && idx <= twiddles.len() {
        fft_mul(v, twiddles[idx - 1]);
    }
    
    let v_len = v.len(); // Capture length before splitting
    let mid = v_len / 2;
    let (u, w) = v.split_at_mut(mid);
    
    // Parallel threshold
    if v_len >= 4096 {
        rayon::join(
            || fft_parallel(u, twiddles, 2 * idx),
            || fft_parallel(w, twiddles, 2 * idx + 1),
        );
    } else {
        fft_sequential(u, twiddles, 2 * idx);
        fft_sequential(w, twiddles, 2 * idx + 1);
    }
}

/// Inverse FFT butterfly operation
fn ifft_mul<F: BinaryFieldElement>(v: &mut [F], lambda: F) {
    let mid = v.len() / 2;
    let (lo, hi) = v.split_at_mut(mid);
    
    for i in 0..mid {
        hi[i] = hi[i].add(&lo[i]);
        let lambda_hi = lambda.mul(&hi[i]);
        lo[i] = lo[i].add(&lambda_hi);
    }
}

/// Inverse FFT
pub fn ifft<F: BinaryFieldElement>(v: &mut [F], twiddles: &[F]) {
    assert!(v.len().is_power_of_two());
    
    if twiddles.is_empty() {
        return;
    }
    
    ifft_sequential(v, twiddles, 1);
}

fn ifft_sequential<F: BinaryFieldElement>(v: &mut [F], twiddles: &[F], idx: usize) {
    if v.len() == 1 {
        return;
    }
    
    let mid = v.len() / 2;
    let (lo, hi) = v.split_at_mut(mid);
    
    ifft_sequential(lo, twiddles, 2 * idx);
    ifft_sequential(hi, twiddles, 2 * idx + 1);
    
    if idx > 0 && idx <= twiddles.len() {
        ifft_mul(v, twiddles[idx - 1]);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::reed_solomon;
    use crate::encode;
    use binary_fields::BinaryElem16;

    #[test]
    fn test_reed_solomon_creation() {
        let rs = reed_solomon::<BinaryElem16>(256, 1024);
        assert_eq!(rs.message_length(), 256);
        assert_eq!(rs.block_length(), 1024);
    }

    #[test]
    fn test_reed_solomon_twiddles() {
        let rs = reed_solomon::<BinaryElem16>(256, 1024);
        assert_eq!(rs.message_length(), 256);
        assert_eq!(rs.block_length(), 1024);
        assert_eq!(rs.twiddles.len(), 1023); // 2^10 - 1
    }

    #[test]
    fn test_reed_solomon_encoding() {
        let rs = reed_solomon::<BinaryElem16>(4, 16);
        
        // Simple message
        let message = vec![
            BinaryElem16::from(1),
            BinaryElem16::from(2),
            BinaryElem16::from(3),
            BinaryElem16::from(4),
        ];
        
        let encoded = encode(&rs, &message);
        
        // The encoded message should be different from just padding with zeros
        assert_eq!(encoded.len(), 16);
        assert_eq!(&encoded[..4], &message[..]); // Systematic encoding
        
        // Check that the parity symbols are not all zero
        let parity_all_zero = encoded[4..].iter().all(|&x| x == BinaryElem16::zero());
        assert!(!parity_all_zero, "Reed-Solomon encoding produced all-zero parity");
        
        println!("Message: {:?}", message);
        println!("Encoded: {:?}", encoded);
    }
}
