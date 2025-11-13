// Optimized GF(2^128) multiplication using 4-multiplication approach
// Mirrors PCLMULQDQ hardware implementation for reliability
// This is faster than bit-by-bit but more auditable than Karatsuba

// GF(2^128) multiplication - PCLMULQDQ style (4 multiplications)
// Split into 64-bit halves, do 4 muls, combine carefully
fn gf128_mul_optimized(a: vec4<u32>, b: vec4<u32>) -> vec4<u32> {
    // Split into 64-bit halves
    // a = [a_lo_lo, a_lo_hi, a_hi_lo, a_hi_hi]
    //      0-31     32-63    64-95    96-127
    let a_lo = vec2<u32>(a.x, a.y);  // bits 0-63
    let a_hi = vec2<u32>(a.z, a.w);  // bits 64-127
    let b_lo = vec2<u32>(b.x, b.y);
    let b_hi = vec2<u32>(b.z, b.w);

    // Four 64x64 -> 128-bit multiplications
    // This mirrors the PCLMULQDQ implementation (simd.rs:229-252)
    let lo_lo = carryless_mul_64(a_lo, b_lo);  // a_lo * b_lo
    let lo_hi = carryless_mul_64(a_lo, b_hi);  // a_lo * b_hi
    let hi_lo = carryless_mul_64(a_hi, b_lo);  // a_hi * b_lo
    let hi_hi = carryless_mul_64(a_hi, b_hi);  // a_hi * b_hi

    // Combine into 256-bit result following simd.rs:264-279
    // result = lo_lo + (lo_hi << 64) + (hi_lo << 64) + (hi_hi << 128)
    //
    // Start with lo_lo as base (bits 0-127)
    var product_lo = lo_lo;
    var product_hi = vec4<u32>(0u, 0u, 0u, 0u);

    // Add (lo_hi << 64): shift left by 64 bits = shift by 2 words
    // lo_hi is [x, y, z, w], << 64 puts [z, w] in high, [x, y] in positions 2-3 of low
    product_lo.z ^= lo_hi.x;
    product_lo.w ^= lo_hi.y;
    product_hi.x ^= lo_hi.z;
    product_hi.y ^= lo_hi.w;

    // Add (hi_lo << 64)
    product_lo.z ^= hi_lo.x;
    product_lo.w ^= hi_lo.y;
    product_hi.x ^= hi_lo.z;
    product_hi.y ^= hi_lo.w;

    // Add (hi_hi << 128): shift left by 128 bits = entire value goes to high
    product_hi.x ^= hi_hi.x;
    product_hi.y ^= hi_hi.y;
    product_hi.z ^= hi_hi.z;
    product_hi.w ^= hi_hi.w;

    // Now reduce the 256-bit product modulo x^128 + x^7 + x^2 + x + 1
    // This is the same reduction as before (verified correct)
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
