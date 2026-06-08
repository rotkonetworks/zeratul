// reed-solomon/src/fft.rs
use binary_fields::BinaryFieldElement;

/// Compute twiddle factors for FFT
pub fn compute_twiddles<F: BinaryFieldElement>(log_n: usize, beta: F) -> Vec<F> {
    if log_n == 0 {
        return vec![];
    }

    let n = 1 << log_n;
    let mut twiddles = vec![F::zero(); n - 1];
    
    // Initialize first layer
    let mut layer = vec![F::zero(); n / 2];
    
    // Layer 0: s_0(beta + 2i) for i = 0..n/2
    // Julia: for i in 1:2^(k-1), l0i = beta + F((i-1) << 1)
    // Rust: for i in 0..2^(k-1), l0i = beta + F(i << 1)
    for i in 0..(n/2) {
        let val = (i as u64) << 1; // 2*i
        layer[i] = beta.add(&F::from_bits(val));
    }
    
    // Julia: twiddles[write_at:end] .= layer
    // In Julia, write_at = 2^(k-1), and arrays are 1-indexed
    // In Rust, we need write_at - 1 for 0-based indexing
    let mut write_at = n / 2;
    if write_at > 0 {
        let start_idx = write_at - 1; // Convert to 0-based
        let copy_len = layer.len().min(twiddles.len() - start_idx);
        twiddles[start_idx..start_idx + copy_len].copy_from_slice(&layer[..copy_len]);
    }
    
    // s_0(v_0) = 1
    let mut s_prev_at_root = F::one();
    
    // Process remaining layers
    for layer_num in 1..log_n {
        write_at >>= 1;
        let layer_len = write_at;
        
        // Compute s_at_root
        // Julia: compute_s_at_root uses layer[2] + layer[1] (1-indexed)
        // Rust: use layer[1] + layer[0] (0-indexed)
        let s_at_root = if layer.len() >= 2 {
            let sum = layer[1].add(&layer[0]);
            next_s(sum, s_prev_at_root)
        } else {
            F::zero()
        };
        
        if s_at_root == F::zero() {
            // This shouldn't happen with valid parameters
            // But we'll handle it gracefully
            break;
        }
        
        // Update layer values
        // Julia: for (idx, s_prev) in enumerate(@views layer[1:2:prev_layer_len])
        // This iterates over odd indices in Julia (1, 3, 5, ...)
        // In Rust, this corresponds to even indices (0, 2, 4, ...)
        for idx in 0..layer_len {
            if idx * 2 < layer.len() {
                let s_prev = layer[idx * 2];
                layer[idx] = next_s(s_prev, s_prev_at_root);
            }
        }
        
        // Normalize and store
        let s_inv = s_at_root.inv();
        
        // Julia: twiddles[write_at:write_at+layer_len-1] = s_inv * layer[1:layer_len]
        // Convert to 0-based indexing
        let start_idx = write_at - 1;
        for i in 0..layer_len {
            if start_idx + i < twiddles.len() && i < layer.len() {
                twiddles[start_idx + i] = s_inv.mul(&layer[i]);
            }
        }
        
        s_prev_at_root = s_at_root;
    }
    
    twiddles
}

fn next_s<F: BinaryFieldElement>(s_prev: F, s_prev_at_root: F) -> F {
    // s_i(x) = s_{i-1}(x)^2 + s_{i-1}(v_{i-1}) * s_{i-1}(x)
    s_prev.mul(&s_prev).add(&s_prev_at_root.mul(&s_prev))
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
pub fn fft<F: BinaryFieldElement>(v: &mut [F], twiddles: &[F], _parallel: bool) {
    assert!(v.len().is_power_of_two());
    
    if v.len() == 1 {
        return;
    }
    
    fft_recursive(v, twiddles, 1);
}

fn fft_recursive<F: BinaryFieldElement>(v: &mut [F], twiddles: &[F], idx: usize) {
    if v.len() == 1 {
        return;
    }
    
    // Apply twiddle
    // Julia uses 1-based indexing for twiddles[idx]
    // In Rust, we need twiddles[idx - 1]
    if idx > 0 && idx - 1 < twiddles.len() {
        fft_mul(v, twiddles[idx - 1]);
    }
    
    let mid = v.len() / 2;
    let (u, w) = v.split_at_mut(mid);
    
    fft_recursive(u, twiddles, 2 * idx);
    fft_recursive(w, twiddles, 2 * idx + 1);
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
    
    if v.len() == 1 {
        return;
    }
    
    ifft_recursive(v, twiddles, 1);
}

fn ifft_recursive<F: BinaryFieldElement>(v: &mut [F], twiddles: &[F], idx: usize) {
    if v.len() == 1 {
        return;
    }
    
    let mid = v.len() / 2;
    let (lo, hi) = v.split_at_mut(mid);
    
    ifft_recursive(lo, twiddles, 2 * idx);
    ifft_recursive(hi, twiddles, 2 * idx + 1);
    
    // Apply twiddle
    // Julia uses 1-based indexing for twiddles[idx]
    // In Rust, we need twiddles[idx - 1]
    if idx > 0 && idx - 1 < twiddles.len() {
        ifft_mul(v, twiddles[idx - 1]);
    }
}
