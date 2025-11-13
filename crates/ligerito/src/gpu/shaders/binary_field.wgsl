// Binary Extension Field Operations for WebGPU
// GF(2^128) operations optimized for GPU execution

// GF(2^128) represented as 4 x 32-bit components (little-endian)
// This maps directly to vec4<u32> in WGSL
struct BinaryField128 {
    data: vec4<u32>
}

// Constants
const ZERO: vec4<u32> = vec4<u32>(0u, 0u, 0u, 0u);
const ONE: vec4<u32> = vec4<u32>(1u, 0u, 0u, 0u);

//
// Addition (XOR - native operation!)
//

fn gf128_add(a: vec4<u32>, b: vec4<u32>) -> vec4<u32> {
    return a ^ b;
}

fn gf128_sub(a: vec4<u32>, b: vec4<u32>) -> vec4<u32> {
    // In GF(2^n), subtraction is the same as addition
    return a ^ b;
}

//
// Multiplication (Carryless)
//

// Carryless 32x32 -> 64-bit multiplication
// Returns [low, high] as vec2<u32>
fn carryless_mul_32(a: u32, b: u32) -> vec2<u32> {
    var result_lo: u32 = 0u;
    var result_hi: u32 = 0u;

    // Process in 4-bit nibbles for better GPU performance
    for (var nibble = 0u; nibble < 8u; nibble++) {
        let shift_base = nibble * 4u;
        let b_nibble = (b >> shift_base) & 0xFu;

        // Unroll the 4 bits
        if ((b_nibble & 1u) != 0u) {
            let shifted = a << shift_base;
            result_lo ^= shifted;
            if (shift_base > 0u) {
                result_hi ^= a >> (32u - shift_base);
            }
        }

        if ((b_nibble & 2u) != 0u) {
            let shift = shift_base + 1u;
            let shifted = a << shift;
            result_lo ^= shifted;
            if (shift > 0u) {
                result_hi ^= a >> (32u - shift);
            }
        }

        if ((b_nibble & 4u) != 0u) {
            let shift = shift_base + 2u;
            let shifted = a << shift;
            result_lo ^= shifted;
            if (shift > 0u) {
                result_hi ^= a >> (32u - shift);
            }
        }

        if ((b_nibble & 8u) != 0u) {
            let shift = shift_base + 3u;
            let shifted = a << shift;
            result_lo ^= shifted;
            if (shift > 0u) {
                result_hi ^= a >> (32u - shift);
            }
        }
    }

    return vec2<u32>(result_lo, result_hi);
}

// Carryless 64x64 -> 128-bit multiplication using Karatsuba
// Input: a, b as vec2<u32> [low, high]
// Output: vec4<u32> [lo0, lo1, hi0, hi1]
fn carryless_mul_64(a: vec2<u32>, b: vec2<u32>) -> vec4<u32> {
    // Karatsuba decomposition
    let a_lo = a.x;
    let a_hi = a.y;
    let b_lo = b.x;
    let b_hi = b.y;

    // Three multiplications
    let z0 = carryless_mul_32(a_lo, b_lo);
    let z2 = carryless_mul_32(a_hi, b_hi);
    let z1 = carryless_mul_32(a_lo ^ a_hi, b_lo ^ b_hi);

    // Combine: middle = z1 ^ z0 ^ z2
    let middle_lo = z1.x ^ z0.x ^ z2.x;
    let middle_hi = z1.y ^ z0.y ^ z2.y;

    // result = z0 + (middle << 32) + (z2 << 64)
    // CRITICAL FIX: middle is shifted by 32, so bits 0-31 come ONLY from z0
    let result_lo = z0.x;  // Bits 0-31: only z0, no middle contribution
    let result_mi = z0.y ^ middle_lo;  // Bits 32-63: z0.y ^ middle_lo
    let result_hi = middle_hi ^ z2.x;  // Bits 64-95: middle_hi ^ z2.x
    let result_top = z2.y;  // Bits 96-127: only z2.y

    return vec4<u32>(result_lo, result_mi, result_hi, result_top);
}

// GF(2^128) multiplication with FULL reduction (consensus-critical!)
// Optimized 4-multiplication approach mirroring PCLMULQDQ hardware
// Faster than bit-by-bit, more auditable than Karatsuba
// Irreducible: x^128 + x^7 + x^2 + x + 1 (0b10000111)
fn gf128_mul(a: vec4<u32>, b: vec4<u32>) -> vec4<u32> {
    // Split into 64-bit halves for 4× 64x64 → 128-bit multiplications
    // This mirrors the PCLMULQDQ implementation (simd.rs:229-280)
    let a_lo = vec2<u32>(a.x, a.y);  // bits 0-63
    let a_hi = vec2<u32>(a.z, a.w);  // bits 64-127
    let b_lo = vec2<u32>(b.x, b.y);
    let b_hi = vec2<u32>(b.z, b.w);

    // Four 64x64 → 128-bit carryless multiplications
    let lo_lo = carryless_mul_64(a_lo, b_lo);  // a_lo * b_lo
    let lo_hi = carryless_mul_64(a_lo, b_hi);  // a_lo * b_hi
    let hi_lo = carryless_mul_64(a_hi, b_lo);  // a_hi * b_lo
    let hi_hi = carryless_mul_64(a_hi, b_hi);  // a_hi * b_hi

    // Combine into 256-bit result: lo_lo + (lo_hi << 64) + (hi_lo << 64) + (hi_hi << 128)
    // Following CPU implementation (simd.rs:264-279)
    var product_lo = lo_lo;
    var product_hi = vec4<u32>(0u, 0u, 0u, 0u);

    // Add (lo_hi << 64): shift left by 64 bits = 2 words
    product_lo.z ^= lo_hi.x;
    product_lo.w ^= lo_hi.y;
    product_hi.x ^= lo_hi.z;
    product_hi.y ^= lo_hi.w;

    // Add (hi_lo << 64)
    product_lo.z ^= hi_lo.x;
    product_lo.w ^= hi_lo.y;
    product_hi.x ^= hi_lo.z;
    product_hi.y ^= hi_lo.w;

    // Add (hi_hi << 128): entire value goes to high half
    product_hi.x ^= hi_hi.x;
    product_hi.y ^= hi_hi.y;
    product_hi.z ^= hi_hi.z;
    product_hi.w ^= hi_hi.w;

    // Step 2: Modular reduction by x^128 + x^7 + x^2 + x + 1
    // Verified correct in previous version
    let high = product_hi;
    let low = product_lo;

    let tmp_0 = high;
    let tmp_127 = vec4<u32>(high.w >> 31u, 0u, 0u, 0u);
    let tmp_126 = vec4<u32>(high.w >> 30u, 0u, 0u, 0u);
    let tmp_121 = vec4<u32>(high.w >> 25u, 0u, 0u, 0u);

    let tmp = vec4<u32>(
        tmp_0.x ^ tmp_127.x ^ tmp_126.x ^ tmp_121.x,
        tmp_0.y ^ tmp_127.y ^ tmp_126.y ^ tmp_121.y,
        tmp_0.z ^ tmp_127.z ^ tmp_126.z ^ tmp_121.z,
        tmp_0.w ^ tmp_127.w ^ tmp_126.w ^ tmp_121.w
    );

    // res = low ^ tmp ^ (tmp << 1) ^ (tmp << 2) ^ (tmp << 7)
    let tmp_shl_1 = vec4<u32>(
        (tmp.x << 1u),
        (tmp.y << 1u) | (tmp.x >> 31u),
        (tmp.z << 1u) | (tmp.y >> 31u),
        (tmp.w << 1u) | (tmp.z >> 31u)
    );

    let tmp_shl_2 = vec4<u32>(
        (tmp.x << 2u),
        (tmp.y << 2u) | (tmp.x >> 30u),
        (tmp.z << 2u) | (tmp.y >> 30u),
        (tmp.w << 2u) | (tmp.z >> 30u)
    );

    let tmp_shl_7 = vec4<u32>(
        (tmp.x << 7u),
        (tmp.y << 7u) | (tmp.x >> 25u),
        (tmp.z << 7u) | (tmp.y >> 25u),
        (tmp.w << 7u) | (tmp.z >> 25u)
    );

    let result = vec4<u32>(
        low.x ^ tmp.x ^ tmp_shl_1.x ^ tmp_shl_2.x ^ tmp_shl_7.x,
        low.y ^ tmp.y ^ tmp_shl_1.y ^ tmp_shl_2.y ^ tmp_shl_7.y,
        low.z ^ tmp.z ^ tmp_shl_1.z ^ tmp_shl_2.z ^ tmp_shl_7.z,
        low.w ^ tmp.w ^ tmp_shl_1.w ^ tmp_shl_2.w ^ tmp_shl_7.w
    );

    return result;
}

//
// Utility functions
//

fn gf128_is_zero(a: vec4<u32>) -> bool {
    return all(a == ZERO);
}

fn gf128_equal(a: vec4<u32>, b: vec4<u32>) -> bool {
    return all(a == b);
}

// Scalar multiplication by small constant (for sumcheck)
fn gf128_mul_by_2(a: vec4<u32>) -> vec4<u32> {
    // Left shift by 1 bit
    let carry0 = a.x >> 31u;
    let carry1 = a.y >> 31u;
    let carry2 = a.z >> 31u;

    return vec4<u32>(
        a.x << 1u,
        (a.y << 1u) | carry0,
        (a.z << 1u) | carry1,
        (a.w << 1u) | carry2
    );
}

// Reduction by irreducible polynomial (if needed)
// GF(2^128): x^128 + x^7 + x^2 + x + 1
// For most operations, reduction happens naturally due to truncation
fn gf128_reduce(a: vec4<u32>) -> vec4<u32> {
    // For truncated multiplication, this is a no-op
    // Full reduction would be needed for full 256-bit products
    return a;
}
