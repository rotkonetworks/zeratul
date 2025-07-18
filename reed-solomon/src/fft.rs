// reed-solomon/src/fft.rs
//! Binary field FFT implementation for Reed-Solomon encoding.
//! Based on recursive subspace polynomial evaluation over GF(2^m).
//! Ported from Julia reference in BinaryReedSolomon/src/binaryfft.jl.

use binary_fields::BinaryFieldElement;
use rayon::prelude::*;

/// Compute next s value: s_i(x) = s_{i-1}(x)^2 + s_{i-1}(v_{i-1}) * s_{i-1}(x)
fn next_s<F: BinaryFieldElement>(s_prev: F, s_prev_at_root: F) -> F {
    s_prev.mul(&s_prev).add(&s_prev_at_root.mul(&s_prev))
}

/// Layer 0 initialization: Fill layer with beta + F::from_bits(i << 1) for i=0..2^{k-1}-1
/// Returns s_prev_at_root = F::one()
fn layer_0<F: BinaryFieldElement>(layer: &mut [F], beta: F, k: usize) -> F {
    let len = 1 << (k - 1);
    for i in 0..len {
        layer[i] = beta.add(&F::from_bits((i as u64) << 1));
    }
    F::one()
}

/// Layer i update: Compute next s values in-place (halves effective layer), return s_at_root
fn layer_i<F: BinaryFieldElement>(layer: &mut [F], layer_len: usize, s_prev_at_root: F) -> F {
    let prev_len = 2 * layer_len;
    let s_at_root = next_s(layer[1].add(&layer[0]), s_prev_at_root);
    for idx in (0..prev_len).step_by(2) {
        let s_prev = layer[idx];
        layer[idx / 2] = next_s(s_prev, s_prev_at_root);
    }
    s_at_root
}

/// Compute twiddle factors for binary FFT
pub fn compute_twiddles<F: BinaryFieldElement>(log_n: usize, beta: F) -> Vec<F> {
    if log_n == 0 {
        return vec![];
    }

    let n = 1 << log_n;
    let mut twiddles = vec![F::zero(); n];

    let mut layer = vec![F::zero(); 1 << (log_n - 1)];
    let mut write_at = 1 << (log_n - 1);

    let mut s_prev_at_root = layer_0(&mut layer, beta, log_n);
    twiddles[write_at..write_at + layer.len()].copy_from_slice(&layer);

    for _ in 1..log_n {
        write_at >>= 1;
        let layer_len = write_at;
        s_prev_at_root = layer_i(&mut layer, layer_len, s_prev_at_root);

        let s_inv = s_prev_at_root.inv();
        for i in 0..layer_len {
            twiddles[write_at + i] = s_inv.mul(&layer[i]);
        }
    }

    twiddles[1..n].to_vec()  // Remove dummy [0], len = n-1
}

/// FFT butterfly in-place: u' = u + λ*w; w' = w + u' (char 2: + = add)
fn fft_mul<F: BinaryFieldElement>(v: &mut [F], lambda: F) {
    let (u, w) = v.split_at_mut(v.len() / 2);
    for i in 0..u.len() {
        let temp = lambda.mul(&w[i]);
        u[i] = u[i].add(&temp);
        w[i] = w[i].add(&u[i]);
    }
}

/// In-place recursive FFT step with twiddles, idx starts at 1
fn fft_twiddles<F: BinaryFieldElement>(v: &mut [F], twiddles: &[F], idx: usize) {
    if v.len() == 1 {
        return;
    }

    fft_mul(v, twiddles[idx - 1]);  // Adjust for 0-based

    let mid = v.len() / 2;
    let (u, w) = v.split_at_mut(mid);

    fft_twiddles(u, twiddles, 2 * idx);
    fft_twiddles(w, twiddles, 2 * idx + 1);
}

/// Parallel in-place recursive FFT step with twiddles, idx starts at 1
fn fft_twiddles_parallel<F: BinaryFieldElement + Send + Sync>(v: &mut [F], twiddles: &[F], idx: usize, thread_depth: usize) {
    if v.len() == 1 {
        return;
    }

    fft_mul(v, twiddles[idx - 1]);

    let mid = v.len() / 2;
    let (u, w) = v.split_at_mut(mid);

    if thread_depth > 0 {
        rayon::join(
            || fft_twiddles_parallel(u, twiddles, 2 * idx, thread_depth - 1),
            || fft_twiddles_parallel(w, twiddles, 2 * idx + 1, thread_depth - 1),
        );
    } else {
        fft_twiddles(u, twiddles, 2 * idx);
        fft_twiddles(w, twiddles, 2 * idx + 1);
    }
}

/// In-place FFT over binary field
pub fn fft<F: BinaryFieldElement + Send + Sync>(v: &mut [F], twiddles: &[F], parallel: bool) {
    if v.len() == 1 {
        return;
    }

    if parallel {
        let thread_depth = rayon::current_num_threads().ilog2() as usize;
        fft_twiddles_parallel(v, twiddles, 1, thread_depth);
    } else {
        fft_twiddles(v, twiddles, 1);
    }
}

/// IFFT butterfly in-place: hi += lo; lo += λ*hi (char 2)
fn ifft_mul<F: BinaryFieldElement>(v: &mut [F], lambda: F) {
    let (lo, hi) = v.split_at_mut(v.len() / 2);
    for i in 0..lo.len() {
        hi[i] = hi[i].add(&lo[i]);
        lo[i] = lo[i].add(&lambda.mul(&hi[i]));
    }
}

/// In-place recursive IFFT step with twiddles, idx starts at 1
fn ifft_twiddles<F: BinaryFieldElement>(v: &mut [F], twiddles: &[F], idx: usize) {
    if v.len() == 1 {
        return;
    }

    let mid = v.len() / 2;
    let (lo, hi) = v.split_at_mut(mid);

    ifft_twiddles(lo, twiddles, 2 * idx);
    ifft_twiddles(hi, twiddles, 2 * idx + 1);

    ifft_mul(v, twiddles[idx - 1]);
}

/// In-place IFFT over binary field
pub fn ifft<F: BinaryFieldElement>(v: &mut [F], twiddles: &[F]) {
    if v.len() == 1 {
        return;
    }

    ifft_twiddles(v, twiddles, 1);
}
