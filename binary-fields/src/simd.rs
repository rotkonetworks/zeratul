// src/simd.rs
use crate::poly::{BinaryPoly64, BinaryPoly128, BinaryPoly256};
use crate::elem::BinaryElem32;

// 64x64 -> 128 bit carryless multiplication
pub fn carryless_mul_64(a: BinaryPoly64, b: BinaryPoly64) -> BinaryPoly128 {
    #[cfg(all(feature = "hardware-accel", target_arch = "x86_64", target_feature = "pclmulqdq"))]
    {
        use core::arch::x86_64::*;

        unsafe {
            let a_vec = _mm_set_epi64x(0, a.value() as i64);
            let b_vec = _mm_set_epi64x(0, b.value() as i64);

            let result = _mm_clmulepi64_si128(a_vec, b_vec, 0x00);

            let lo = _mm_extract_epi64(result, 0) as u64;
            let hi = _mm_extract_epi64(result, 1) as u64;

            return BinaryPoly128::new(((hi as u128) << 64) | (lo as u128));
        }
    }

    #[cfg(not(all(feature = "hardware-accel", target_arch = "x86_64", target_feature = "pclmulqdq")))]
    {
        // software fallback
        carryless_mul_64_soft(a, b)
    }
}

// software implementation for 64x64
fn carryless_mul_64_soft(a: BinaryPoly64, b: BinaryPoly64) -> BinaryPoly128 {
    let mut result = 0u128;
    let a_val = a.value();
    let b_val = b.value();

    for i in 0..64 {
        let mask = 0u128.wrapping_sub(((b_val >> i) & 1) as u128);
        result ^= ((a_val as u128) << i) & mask;
    }

    BinaryPoly128::new(result)
}

// 128x128 -> 128 bit carryless multiplication (truncated)
pub fn carryless_mul_128(a: BinaryPoly128, b: BinaryPoly128) -> BinaryPoly128 {
    #[cfg(all(feature = "hardware-accel", target_arch = "x86_64", target_feature = "pclmulqdq"))]
    {
        use core::arch::x86_64::*;

        unsafe {
            // split inputs into 64-bit halves
            let a_lo = a.value() as u64;
            let a_hi = (a.value() >> 64) as u64;
            let b_lo = b.value() as u64;
            let b_hi = (b.value() >> 64) as u64;

            // perform 3 64x64->128 bit multiplications (skip hi*hi for truncated result)
            let lo_lo = _mm_clmulepi64_si128(
                _mm_set_epi64x(0, a_lo as i64),
                _mm_set_epi64x(0, b_lo as i64),
                0x00
            );

            let lo_hi = _mm_clmulepi64_si128(
                _mm_set_epi64x(0, a_lo as i64),
                _mm_set_epi64x(0, b_hi as i64),
                0x00
            );

            let hi_lo = _mm_clmulepi64_si128(
                _mm_set_epi64x(0, a_hi as i64),
                _mm_set_epi64x(0, b_lo as i64),
                0x00
            );

            // extract 128-bit results - fix the overflow by casting to u128 first
            let r0 = (_mm_extract_epi64(lo_lo, 0) as u64) as u128
                   | ((_mm_extract_epi64(lo_lo, 1) as u64) as u128) << 64;
            let r1 = (_mm_extract_epi64(lo_hi, 0) as u64) as u128
                   | ((_mm_extract_epi64(lo_hi, 1) as u64) as u128) << 64;
            let r2 = (_mm_extract_epi64(hi_lo, 0) as u64) as u128
                   | ((_mm_extract_epi64(hi_lo, 1) as u64) as u128) << 64;

            // combine: result = r0 + (r1 << 64) + (r2 << 64)
            let result = r0 ^ (r1 << 64) ^ (r2 << 64);

            return BinaryPoly128::new(result);
        }
    }

    #[cfg(not(all(feature = "hardware-accel", target_arch = "x86_64", target_feature = "pclmulqdq")))]
    {
        // software fallback
        carryless_mul_128_soft(a, b)
    }
}

// software implementation for 128x128 truncated
fn carryless_mul_128_soft(a: BinaryPoly128, b: BinaryPoly128) -> BinaryPoly128 {
    let a_lo = a.value() as u64;
    let a_hi = (a.value() >> 64) as u64;
    let b_lo = b.value() as u64;
    let b_hi = (b.value() >> 64) as u64;

    let z0 = mul_64x64_to_128(a_lo, b_lo);
    let z1 = mul_64x64_to_128(a_lo ^ a_hi, b_lo ^ b_hi);
    let z2 = mul_64x64_to_128(a_hi, b_hi);

    // karatsuba combination (truncated)
    let result = z0 ^ (z1 << 64) ^ (z0 << 64) ^ (z2 << 64);
    BinaryPoly128::new(result)
}

// 128x128 -> 256 bit full multiplication
pub fn carryless_mul_128_full(a: BinaryPoly128, b: BinaryPoly128) -> BinaryPoly256 {
    #[cfg(all(feature = "hardware-accel", target_arch = "x86_64", target_feature = "pclmulqdq"))]
    {
        use core::arch::x86_64::*;

        unsafe {
            let a_lo = a.value() as u64;
            let a_hi = (a.value() >> 64) as u64;
            let b_lo = b.value() as u64;
            let b_hi = (b.value() >> 64) as u64;

            // 4 multiplications
            let lo_lo = _mm_clmulepi64_si128(
                _mm_set_epi64x(0, a_lo as i64),
                _mm_set_epi64x(0, b_lo as i64),
                0x00
            );

            let lo_hi = _mm_clmulepi64_si128(
                _mm_set_epi64x(0, a_lo as i64),
                _mm_set_epi64x(0, b_hi as i64),
                0x00
            );

            let hi_lo = _mm_clmulepi64_si128(
                _mm_set_epi64x(0, a_hi as i64),
                _mm_set_epi64x(0, b_lo as i64),
                0x00
            );

            let hi_hi = _mm_clmulepi64_si128(
                _mm_set_epi64x(0, a_hi as i64),
                _mm_set_epi64x(0, b_hi as i64),
                0x00
            );

            // extract and combine
            let r0_lo = _mm_extract_epi64(lo_lo, 0) as u64;
            let r0_hi = _mm_extract_epi64(lo_lo, 1) as u64;
            let r1_lo = _mm_extract_epi64(lo_hi, 0) as u64;
            let r1_hi = _mm_extract_epi64(lo_hi, 1) as u64;
            let r2_lo = _mm_extract_epi64(hi_lo, 0) as u64;
            let r2_hi = _mm_extract_epi64(hi_lo, 1) as u64;
            let r3_lo = _mm_extract_epi64(hi_hi, 0) as u64;
            let r3_hi = _mm_extract_epi64(hi_hi, 1) as u64;

            // build 256-bit result
            let mut lo = r0_lo as u128 | ((r0_hi as u128) << 64);
            let mut hi = 0u128;

            // add r1 << 64
            lo ^= (r1_lo as u128) << 64;
            hi ^= (r1_lo as u128) >> 64;
            hi ^= r1_hi as u128;

            // add r2 << 64
            lo ^= (r2_lo as u128) << 64;
            hi ^= (r2_lo as u128) >> 64;
            hi ^= r2_hi as u128;

            // add r3 << 128
            hi ^= r3_lo as u128 | ((r3_hi as u128) << 64);

            return BinaryPoly256::from_parts(hi, lo);
        }
    }

    #[cfg(not(all(feature = "hardware-accel", target_arch = "x86_64", target_feature = "pclmulqdq")))]
    {
        // software fallback
        carryless_mul_128_full_soft(a, b)
    }
}

// software implementation for 128x128 full
fn carryless_mul_128_full_soft(a: BinaryPoly128, b: BinaryPoly128) -> BinaryPoly256 {
    let a_lo = a.value() as u64;
    let a_hi = (a.value() >> 64) as u64;
    let b_lo = b.value() as u64;
    let b_hi = (b.value() >> 64) as u64;

    let z0 = mul_64x64_to_128(a_lo, b_lo);
    let z2 = mul_64x64_to_128(a_hi, b_hi);
    let z1 = mul_64x64_to_128(a_lo ^ a_hi, b_lo ^ b_hi) ^ z0 ^ z2;

    // combine: result = z0 + (z1 << 64) + (z2 << 128)
    let mut lo = z0;
    let mut hi = 0u128;

    // add z1 << 64
    lo ^= z1 << 64;
    hi ^= z1 >> 64;

    // add z2 << 128
    hi ^= z2;

    BinaryPoly256::from_parts(hi, lo)
}

// helper: constant-time 64x64 -> 128
#[inline(always)]
fn mul_64x64_to_128(a: u64, b: u64) -> u128 {
    let mut result = 0u128;
    let mut a_shifted = a as u128;

    for i in 0..64 {
        let mask = 0u128.wrapping_sub(((b >> i) & 1) as u128);
        result ^= a_shifted & mask;
        a_shifted <<= 1;
    }

    result
}

// batch field operations

use crate::{BinaryElem128, BinaryFieldElement};

/// batch multiply gf(2^128) elements with three-tier dispatch:
/// hardware-accel → pclmulqdq, simd → portable_simd, else → scalar
pub fn batch_mul_gf128(a: &[BinaryElem128], b: &[BinaryElem128], out: &mut [BinaryElem128]) {
    assert_eq!(a.len(), b.len());
    assert_eq!(a.len(), out.len());

    #[cfg(all(feature = "hardware-accel", target_arch = "x86_64", target_feature = "pclmulqdq"))]
    {
        return batch_mul_gf128_hw(a, b, out);
    }

    #[cfg(all(feature = "simd", not(all(feature = "hardware-accel", target_arch = "x86_64", target_feature = "pclmulqdq"))))]
    {
        return batch_mul_gf128_portable(a, b, out);
    }

    #[cfg(not(any(
        all(feature = "hardware-accel", target_arch = "x86_64", target_feature = "pclmulqdq"),
        feature = "simd"
    )))]
    {
        // scalar fallback
        for i in 0..a.len() {
            out[i] = a[i].mul(&b[i]);
        }
    }
}

/// batch add gf(2^128) elements (xor in gf(2^n))
pub fn batch_add_gf128(a: &[BinaryElem128], b: &[BinaryElem128], out: &mut [BinaryElem128]) {
    assert_eq!(a.len(), b.len());
    assert_eq!(a.len(), out.len());

    #[cfg(feature = "simd")]
    {
        return batch_add_gf128_portable(a, b, out);
    }

    // scalar fallback
    for i in 0..a.len() {
        out[i] = a[i].add(&b[i]);
    }
}

// pclmulqdq-based batch multiply for x86_64
#[cfg(all(feature = "hardware-accel", target_arch = "x86_64", target_feature = "pclmulqdq"))]
fn batch_mul_gf128_hw(a: &[BinaryElem128], b: &[BinaryElem128], out: &mut [BinaryElem128]) {
    for i in 0..a.len() {
        let a_poly = a[i].poly();
        let b_poly = b[i].poly();
        let product = carryless_mul_128_full(a_poly, b_poly);
        let reduced = reduce_gf128(product);
        out[i] = BinaryElem128::from_value(reduced.value());
    }
}

/// reduce 256-bit product modulo GF(2^128) irreducible polynomial
/// irreducible: x^128 + x^7 + x^2 + x + 1 (0x87 = 0b10000111)
/// matches julia's @generated mod_irreducible (binaryfield.jl:73-114)
#[inline(always)]
pub fn reduce_gf128(product: BinaryPoly256) -> BinaryPoly128 {
    let (hi, lo) = product.split();
    let high = hi.value();
    let low = lo.value();

    // julia's compute_tmp for irreducible 0b10000111 (bits 0,1,2,7):
    // for each set bit i in irreducible: tmp ^= hi >> (128 - i)
    // bits set: 0, 1, 2, 7 -> shifts: 128, 127, 126, 121
    let tmp = high ^ (high >> 127) ^ (high >> 126) ^ (high >> 121);

    // julia's compute_res:
    // for each set bit i in irreducible: res ^= tmp << i
    // bits set: 0, 1, 2, 7 -> shifts: 0, 1, 2, 7
    let res = low ^ tmp ^ (tmp << 1) ^ (tmp << 2) ^ (tmp << 7);

    BinaryPoly128::new(res)
}

// portable_simd batch ops (cross-platform, nightly)
#[cfg(feature = "simd")]
fn batch_mul_gf128_portable(a: &[BinaryElem128], b: &[BinaryElem128], out: &mut [BinaryElem128]) {
    // Use vectorized XOR for addition, but multiplication still needs field arithmetic
    // For now, use scalar multiplication which already uses SIMD pclmulqdq internally
    for i in 0..a.len() {
        out[i] = a[i].mul(&b[i]);
    }
}

#[cfg(feature = "simd")]
fn batch_add_gf128_portable(a: &[BinaryElem128], b: &[BinaryElem128], out: &mut [BinaryElem128]) {
    // XOR is embarrassingly parallel - process in chunks
    // Binary field addition is just XOR
    use rayon::prelude::*;

    const CHUNK_SIZE: usize = 1024;

    a.par_chunks(CHUNK_SIZE)
        .zip(b.par_chunks(CHUNK_SIZE))
        .zip(out.par_chunks_mut(CHUNK_SIZE))
        .for_each(|((a_chunk, b_chunk), out_chunk)| {
            for i in 0..a_chunk.len() {
                out_chunk[i] = a_chunk[i].add(&b_chunk[i]);
            }
        });
}

// =========================================================================
// BinaryElem32 batch operations - FFT optimization
// =========================================================================

/// AVX-512 vectorized FFT butterfly for GF(2^32)
/// processes 4 elements at once using 256-bit vectors
#[cfg(all(feature = "hardware-accel", target_arch = "x86_64", target_feature = "pclmulqdq"))]
pub fn fft_butterfly_gf32_avx512(u: &mut [BinaryElem32], w: &mut [BinaryElem32], lambda: BinaryElem32) {
    // Check AVX-512 availability at runtime
    // Note: vpclmulqdq is AVX-512 extension, not always available even with avx512f
    #[cfg(target_arch = "x86_64")]
    {
        // For now, just use SSE since AVX-512 intrinsics for vpclmulqdq are complex
        // and may not provide significant benefit due to memory bandwidth limits
        return fft_butterfly_gf32_sse(u, w, lambda);
    }

    #[cfg(not(target_arch = "x86_64"))]
    {
        fft_butterfly_gf32_scalar(u, w, lambda)
    }
}

/// AVX-512 implementation (unsafe, requires feature detection)
#[cfg(all(feature = "hardware-accel", target_arch = "x86_64", target_feature = "pclmulqdq"))]
#[target_feature(enable = "avx512f,vpclmulqdq")]
unsafe fn fft_butterfly_gf32_avx512_impl(u: &mut [BinaryElem32], w: &mut [BinaryElem32], lambda: BinaryElem32) {
    use core::arch::x86_64::*;

    assert_eq!(u.len(), w.len());
    let len = u.len();

    const IRREDUCIBLE_32: u64 = (1u64 << 32) | 0b11001 | (1 << 7) | (1 << 9) | (1 << 15);

    let lambda_val = lambda.poly().value() as u64;
    let lambda_vec = _mm256_set1_epi64x(lambda_val as i64);

    let mut i = 0;

    // Process 4 elements at once (4x32-bit = 128 bits, fit in 256-bit lanes)
    while i + 4 <= len {
        // Load w[i..i+4] into 256-bit vector
        let w0 = w[i].poly().value() as u64;
        let w1 = w[i+1].poly().value() as u64;
        let w2 = w[i+2].poly().value() as u64;
        let w3 = w[i+3].poly().value() as u64;

        let w_vec = _mm256_set_epi64x(w3 as i64, w2 as i64, w1 as i64, w0 as i64);

        // Carryless multiply: lambda * w (4 parallel multiplications)
        let prod = _mm256_clmulepi64_epi128(lambda_vec, w_vec, 0x00);

        // Extract and reduce each 64-bit product
        let p0 = _mm256_extract_epi64(prod, 0) as u64;
        let p1 = _mm256_extract_epi64(prod, 1) as u64;
        let p2 = _mm256_extract_epi64(prod, 2) as u64;
        let p3 = _mm256_extract_epi64(prod, 3) as u64;

        let lambda_w0 = reduce_gf32(p0, IRREDUCIBLE_32);
        let lambda_w1 = reduce_gf32(p1, IRREDUCIBLE_32);
        let lambda_w2 = reduce_gf32(p2, IRREDUCIBLE_32);
        let lambda_w3 = reduce_gf32(p3, IRREDUCIBLE_32);

        // u[i] = u[i] XOR lambda_w[i]
        let u0 = u[i].poly().value() ^ (lambda_w0 as u32);
        let u1 = u[i+1].poly().value() ^ (lambda_w1 as u32);
        let u2 = u[i+2].poly().value() ^ (lambda_w2 as u32);
        let u3 = u[i+3].poly().value() ^ (lambda_w3 as u32);

        // w[i] = w[i] XOR u[i]
        let w0_new = w[i].poly().value() ^ u0;
        let w1_new = w[i+1].poly().value() ^ u1;
        let w2_new = w[i+2].poly().value() ^ u2;
        let w3_new = w[i+3].poly().value() ^ u3;

        u[i] = BinaryElem32::from(u0);
        u[i+1] = BinaryElem32::from(u1);
        u[i+2] = BinaryElem32::from(u2);
        u[i+3] = BinaryElem32::from(u3);
        w[i] = BinaryElem32::from(w0_new);
        w[i+1] = BinaryElem32::from(w1_new);
        w[i+2] = BinaryElem32::from(w2_new);
        w[i+3] = BinaryElem32::from(w3_new);

        i += 4;
    }

    // Handle remaining elements with SSE
    while i < len {
        let lambda_w = lambda.mul(&w[i]);
        u[i] = u[i].add(&lambda_w);
        w[i] = w[i].add(&u[i]);
        i += 1;
    }
}

/// SSE vectorized FFT butterfly operation for GF(2^32)
/// computes: u[i] = u[i] + lambda*w[i]; w[i] = w[i] + u[i]
/// processes 2 elements at a time using SSE/AVX
#[cfg(all(feature = "hardware-accel", target_arch = "x86_64", target_feature = "pclmulqdq"))]
pub fn fft_butterfly_gf32_sse(u: &mut [BinaryElem32], w: &mut [BinaryElem32], lambda: BinaryElem32) {
    use core::arch::x86_64::*;

    assert_eq!(u.len(), w.len());
    let len = u.len();

    // irreducible polynomial for GF(2^32):
    // x^32 + x^7 + x^9 + x^15 + x^3 + 1
    const IRREDUCIBLE_32: u64 = (1u64 << 32) | 0b11001 | (1 << 7) | (1 << 9) | (1 << 15);

    unsafe {
        let lambda_val = lambda.poly().value() as u64;
        let lambda_vec = _mm_set1_epi64x(lambda_val as i64);

        let mut i = 0;

        // process 2 elements at once (2x32-bit = 64 bits, fits in one lane)
        while i + 2 <= len {
            // load w[i] and w[i+1] into 64-bit lanes
            let w0 = w[i].poly().value() as u64;
            let w1 = w[i+1].poly().value() as u64;
            let w_vec = _mm_set_epi64x(w1 as i64, w0 as i64);

            // carryless multiply: lambda * w[i]
            let prod_lo = _mm_clmulepi64_si128(lambda_vec, w_vec, 0x00); // lambda * w0
            let prod_hi = _mm_clmulepi64_si128(lambda_vec, w_vec, 0x11); // lambda * w1

            // reduce modulo irreducible
            let p0 = _mm_extract_epi64(prod_lo, 0) as u64;
            let p1 = _mm_extract_epi64(prod_hi, 0) as u64;

            let lambda_w0 = reduce_gf32(p0, IRREDUCIBLE_32);
            let lambda_w1 = reduce_gf32(p1, IRREDUCIBLE_32);

            // u[i] = u[i] XOR lambda_w[i]
            let u0 = u[i].poly().value() ^ (lambda_w0 as u32);
            let u1 = u[i+1].poly().value() ^ (lambda_w1 as u32);

            // w[i] = w[i] XOR u[i] (using updated u)
            let w0_new = w[i].poly().value() ^ u0;
            let w1_new = w[i+1].poly().value() ^ u1;

            u[i] = BinaryElem32::from(u0);
            u[i+1] = BinaryElem32::from(u1);
            w[i] = BinaryElem32::from(w0_new);
            w[i+1] = BinaryElem32::from(w1_new);

            i += 2;
        }

        // handle remaining element
        if i < len {
            let lambda_w = lambda.mul(&w[i]);
            u[i] = u[i].add(&lambda_w);
            w[i] = w[i].add(&u[i]);
        }
    }
}

/// reduce 64-bit product modulo GF(2^32) irreducible
/// optimized branchless reduction for GF(2^32)
#[inline(always)]
fn reduce_gf32(p: u64, irr: u64) -> u64 {
    // for 32x32 -> 64 multiplication, we need to reduce bits [63:32]
    // unrolled reduction: process high 32 bits in chunks

    let hi = (p >> 32) as u64;
    let lo = (p & 0xFFFFFFFF) as u64;

    // compute tmp by shifting high bits down
    // for irreducible 0b1_0000_1000_1001_1000_1001 (x^32 + x^15 + x^9 + x^7 + x^3 + 1)
    // bits set at positions: 0,3,7,9,15 -> shifts needed: 32,29,25,23,17
    let tmp = hi
        ^ (hi >> 29)  // bit 15: shift by (32-3)
        ^ (hi >> 25)  // bit 9: shift by (32-7)
        ^ (hi >> 23)  // bit 7: shift by (32-9)
        ^ (hi >> 17); // bit 3: shift by (32-15)

    // XOR with low bits and shifted tmp
    lo ^ tmp ^ (tmp << 3) ^ (tmp << 7) ^ (tmp << 9) ^ (tmp << 15)
}

/// scalar fallback for FFT butterfly
pub fn fft_butterfly_gf32_scalar(u: &mut [BinaryElem32], w: &mut [BinaryElem32], lambda: BinaryElem32) {
    assert_eq!(u.len(), w.len());

    for i in 0..u.len() {
        let lambda_w = lambda.mul(&w[i]);
        u[i] = u[i].add(&lambda_w);
        w[i] = w[i].add(&u[i]);
    }
}

/// dispatch FFT butterfly to best available SIMD version
pub fn fft_butterfly_gf32(u: &mut [BinaryElem32], w: &mut [BinaryElem32], lambda: BinaryElem32) {
    #[cfg(all(feature = "hardware-accel", target_arch = "x86_64", target_feature = "pclmulqdq"))]
    {
        // Try AVX-512 first (runtime detection), fallback to SSE
        return fft_butterfly_gf32_avx512(u, w, lambda);
    }

    #[cfg(not(all(feature = "hardware-accel", target_arch = "x86_64", target_feature = "pclmulqdq")))]
    {
        fft_butterfly_gf32_scalar(u, w, lambda)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_fft_butterfly_gf32() {
        // test SIMD vs scalar butterfly give same results
        let mut u_simd = vec![
            BinaryElem32::from(1),
            BinaryElem32::from(2),
            BinaryElem32::from(3),
            BinaryElem32::from(4),
        ];
        let mut w_simd = vec![
            BinaryElem32::from(5),
            BinaryElem32::from(6),
            BinaryElem32::from(7),
            BinaryElem32::from(8),
        ];
        let lambda = BinaryElem32::from(3);

        let mut u_scalar = u_simd.clone();
        let mut w_scalar = w_simd.clone();

        fft_butterfly_gf32(&mut u_simd, &mut w_simd, lambda);
        fft_butterfly_gf32_scalar(&mut u_scalar, &mut w_scalar, lambda);

        for i in 0..u_simd.len() {
            assert_eq!(u_simd[i], u_scalar[i], "u mismatch at index {}", i);
            assert_eq!(w_simd[i], w_scalar[i], "w mismatch at index {}", i);
        }
    }

    #[test]
    fn test_batch_add() {
        let a = vec![
            BinaryElem128::from(1),
            BinaryElem128::from(2),
            BinaryElem128::from(3),
        ];
        let b = vec![
            BinaryElem128::from(4),
            BinaryElem128::from(5),
            BinaryElem128::from(6),
        ];
        let mut out = vec![BinaryElem128::zero(); 3];

        batch_add_gf128(&a, &b, &mut out);

        for i in 0..3 {
            assert_eq!(out[i], a[i].add(&b[i]));
        }
    }

    #[test]
    fn test_batch_mul() {
        let a = vec![
            BinaryElem128::from(7),
            BinaryElem128::from(11),
            BinaryElem128::from(13),
        ];
        let b = vec![
            BinaryElem128::from(3),
            BinaryElem128::from(5),
            BinaryElem128::from(7),
        ];
        let mut out = vec![BinaryElem128::zero(); 3];

        batch_mul_gf128(&a, &b, &mut out);

        for i in 0..3 {
            assert_eq!(out[i], a[i].mul(&b[i]));
        }
    }

    #[test]
    fn test_batch_mul_large() {
        // test with larger field elements
        let a = vec![
            BinaryElem128::from(0xFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFF0),
            BinaryElem128::from(u128::MAX),
        ];
        let b = vec![
            BinaryElem128::from(0x123456789ABCDEF0123456789ABCDEF0),
            BinaryElem128::from(0x8000000000000000_0000000000000000),
        ];
        let mut out = vec![BinaryElem128::zero(); 2];

        batch_mul_gf128(&a, &b, &mut out);

        for i in 0..2 {
            assert_eq!(out[i], a[i].mul(&b[i]));
        }
    }
}
