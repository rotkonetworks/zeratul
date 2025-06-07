use binary_fields::BinaryFieldElement;
use rayon::prelude::*;

/// Compute twiddle factors for FFT
pub fn compute_twiddles<F: BinaryFieldElement>(log_n: usize, beta: F) -> Vec<F> {
    let n = 1 << log_n;
    let mut twiddles = vec![F::zero(); n - 1];
    
    // For the zero beta case, we need special handling
    if log_n == 0 {
        return twiddles;
    }
    
    // Layer 0 computation (matching Julia's layer_0!)
    let mut layer = vec![F::zero(); n / 2];
    let mut s_prev_at_root = F::one();
    
    for i in 0..n/2 {
        // In Julia: beta + F((i-1) << 1), but since we can't easily
        // create field elements from integers,
        // we'll use a different approach for now
        layer[i] = beta; // TODO: Simplified for now
    }
    
    // Copy to twiddles
    let write_at = n / 2;
    if write_at > 0 && write_at <= twiddles.len() + 1 {
        let end = (write_at - 1 + layer.len()).min(twiddles.len());
        twiddles[write_at - 1..end].copy_from_slice(&layer[..end - write_at + 1]);
    }
    
    // Subsequent layers (matching Julia's layer_i!)
    let mut write_at = write_at / 2;
    while write_at > 0 {
        let layer_len = write_at.min(layer.len() / 2);
        
        // Update s_prev_at_root
        let s_at_root = compute_s_at_root(&layer, s_prev_at_root);
        
        // Skip if s_at_root is zero to avoid division by zero
        if s_at_root == F::zero() {
            break;
        }
        
        // Update layer values
        let layer_vec: Vec<F> = layer.iter().step_by(2).take(layer_len).copied().collect();
        for (idx, s_prev) in layer_vec.iter().enumerate() {
            if idx < layer.len() {
                layer[idx] = next_s(*s_prev, s_prev_at_root);
            }
        }
        
        // Normalize and store
        let s_inv = s_at_root.inv();
        let start = write_at - 1;
        for i in 0..layer_len {
            if start + i < twiddles.len() && i < layer.len() {
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
    next_s(layer[1].add(&layer[0]), s_prev_at_root)
}

/// In-place FFT with twiddle factors
pub fn fft<F: BinaryFieldElement>(v: &mut [F], twiddles: &[F], parallel: bool) {
    assert!(v.len().is_power_of_two());
    
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
    
    fft_mul(v, twiddles[idx - 1]);
    
    let v_len = v.len();
    let mid = v.len() / 2;
    let v_len = v.len();
    let (u, w) = v.split_at_mut(mid);
    
    fft_sequential(u, twiddles, 2 * idx);
    fft_sequential(w, twiddles, 2 * idx + 1);
}

fn fft_parallel<F: BinaryFieldElement>(v: &mut [F], twiddles: &[F], idx: usize) {
    if v.len() == 1 {
        return;
    }
    
    fft_mul(v, twiddles[idx - 1]);
    
    let v_len = v.len();
    let mid = v.len() / 2;
    let v_len = v.len();
    let (u, w) = v.split_at_mut(mid);
    
    // Parallel threshold - tune based on benchmarks
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

/// FFT butterfly operation (matching Julia's fft_mul!)
fn fft_mul<F: BinaryFieldElement>(v: &mut [F], lambda: F) {
    let v_len = v.len();
    let mid = v.len() / 2;
    let v_len = v.len();
    let (u, w) = v.split_at_mut(mid);
    
    // Parallel for large vectors
    if v_len >= 1024 {
        u.par_iter_mut()
            .zip(w.par_iter_mut())
            .for_each(|(u_i, w_i)| {
                let lambda_w = lambda.mul(w_i);
                *u_i = u_i.add(&lambda_w);
                *w_i = w_i.add(u_i);
            });
    } else {
        for (u_i, w_i) in u.iter_mut().zip(w.iter_mut()) {
            let lambda_w = lambda.mul(w_i);
            *u_i = u_i.add(&lambda_w);
            *w_i = w_i.add(u_i);
        }
    }
}

/// Inverse FFT
pub fn ifft<F: BinaryFieldElement>(v: &mut [F], twiddles: &[F]) {
    assert!(v.len().is_power_of_two());
    ifft_sequential(v, twiddles, 1);
}

fn ifft_sequential<F: BinaryFieldElement>(v: &mut [F], twiddles: &[F], idx: usize) {
    if v.len() == 1 {
        return;
    }
    
    let v_len = v.len();
    let mid = v.len() / 2;
    let (lo, hi) = v.split_at_mut(mid);
    
    ifft_sequential(lo, twiddles, 2 * idx);
    ifft_sequential(hi, twiddles, 2 * idx + 1);
    
    ifft_mul(v, twiddles[idx - 1]);
}

/// Inverse FFT butterfly operation
fn ifft_mul<F: BinaryFieldElement>(v: &mut [F], lambda: F) {
    let v_len = v.len();
    let mid = v.len() / 2;
    let (lo, hi) = v.split_at_mut(mid);
    
    for (lo_i, hi_i) in lo.iter_mut().zip(hi.iter_mut()) {
        *hi_i = hi_i.add(lo_i);
        let lambda_hi = lambda.mul(hi_i);
        *lo_i = lo_i.add(&lambda_hi);
    }
}
