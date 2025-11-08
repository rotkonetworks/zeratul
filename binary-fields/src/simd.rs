// src/simd.rs
use crate::poly::{BinaryPoly64, BinaryPoly128, BinaryPoly256};

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

/// Reduce 256-bit product modulo GF(2^128) irreducible polynomial
/// Irreducible: x^128 + x^7 + x^2 + x + 1 (0x87 = 0b10000111)
/// Matches Julia's @generated mod_irreducible implementation
pub fn reduce_gf128(product: BinaryPoly256) -> BinaryPoly128 {
    let (hi, lo) = product.split();
    let high = hi.value();
    let low = lo.value();

    // Julia's compute_tmp for irreducible 0b10000111 (bits 0,1,2,7):
    // tmp = hi ^ (hi >> 127) ^ (hi >> 126) ^ (hi >> 121)
    let tmp = high ^ (high >> 127) ^ (high >> 126) ^ (high >> 121);

    // Julia's compute_res:
    // res = lo ^ tmp ^ (tmp << 1) ^ (tmp << 2) ^ (tmp << 7)
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

#[cfg(test)]
mod tests {
    use super::*;

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
