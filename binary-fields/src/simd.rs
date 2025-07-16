// src/simd.rs
#[allow(unused_imports)]
use crate::poly::{BinaryPoly64, BinaryPoly128};

#[cfg(target_arch = "x86_64")]
pub fn carryless_mul(a: BinaryPoly64, b: BinaryPoly64) -> BinaryPoly128 {
    #[cfg(target_feature = "pclmulqdq")]
    unsafe {
        use core::arch::x86_64::*;
        
        let a_vec = _mm_set_epi64x(0, a.value() as i64);
        let b_vec = _mm_set_epi64x(0, b.value() as i64);
        
        let result = _mm_clmulepi64_si128(a_vec, b_vec, 0x00);
        
        let lo = _mm_extract_epi64(result, 0) as u64;
        let hi = _mm_extract_epi64(result, 1) as u64;
        
        BinaryPoly128::new(((hi as u128) << 64) | (lo as u128))
    }
    
    #[cfg(not(target_feature = "pclmulqdq"))]
    {
        // Software fallback
        let mut result = 0u128;
        let a_val = a.value();
        let b_val = b.value();
        
        for i in 0..64 {
            if (b_val >> i) & 1 == 1 {
                result ^= (a_val as u128) << i;
            }
        }
        
        BinaryPoly128::new(result)
    }
}

#[cfg(not(target_arch = "x86_64"))]
pub fn carryless_mul(a: BinaryPoly64, b: BinaryPoly64) -> BinaryPoly128 {
    // Software fallback
    let mut result = 0u128;
    let a_val = a.value();
    let b_val = b.value();
    
    for i in 0..64 {
        if (b_val >> i) & 1 == 1 {
            result ^= (a_val as u128) << i;
        }
    }
    
    BinaryPoly128::new(result)
}
