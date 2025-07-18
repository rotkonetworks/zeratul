// src/simd.rs
use crate::poly::{BinaryPoly64, BinaryPoly128, BinaryPoly256};

// 64x64 -> 128 bit carryless multiplication
pub fn carryless_mul_64(a: BinaryPoly64, b: BinaryPoly64) -> BinaryPoly128 {
    #[cfg(all(target_arch = "x86_64", any(target_feature = "pclmulqdq", target_feature = "sse2")))]
    {
        use core::arch::x86_64::*;
        
        unsafe {
            if is_x86_feature_detected!("pclmulqdq") {
                let a_vec = _mm_set_epi64x(0, a.value() as i64);
                let b_vec = _mm_set_epi64x(0, b.value() as i64);

                let result = _mm_clmulepi64_si128(a_vec, b_vec, 0x00);

                let lo = _mm_extract_epi64(result, 0) as u64;
                let hi = _mm_extract_epi64(result, 1) as u64;

                return BinaryPoly128::new(((hi as u128) << 64) | (lo as u128));
            }
        }
    }

    // software fallback
    carryless_mul_64_soft(a, b)
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
    #[cfg(all(target_arch = "x86_64", any(target_feature = "pclmulqdq", target_feature = "sse2")))]
    {
        use core::arch::x86_64::*;
        
        unsafe {
            if is_x86_feature_detected!("pclmulqdq") {
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
    }

    // software fallback
    carryless_mul_128_soft(a, b)
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
    #[cfg(all(target_arch = "x86_64", any(target_feature = "pclmulqdq", target_feature = "sse2")))]
    {
        use core::arch::x86_64::*;
        
        unsafe {
            if is_x86_feature_detected!("pclmulqdq") {
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
    }

    // software fallback
    carryless_mul_128_full_soft(a, b)
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
