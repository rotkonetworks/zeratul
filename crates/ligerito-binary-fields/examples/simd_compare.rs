//! Compare AVX-512 vs AVX2 vs SSE for FFT butterfly at different sizes
use ligerito_binary_fields::BinaryElem32;
use ligerito_binary_fields::simd::{
    fft_butterfly_gf32_avx512_only,
    fft_butterfly_gf32_avx2_only,
    fft_butterfly_gf32_sse
};
use std::time::Instant;

fn main() {
    // Check CPU features
    println!("CPU Features:");
    println!("  AVX-512F: {}", is_x86_feature_detected!("avx512f"));
    println!("  AVX2: {}", is_x86_feature_detected!("avx2"));
    println!("  VPCLMULQDQ: {}", is_x86_feature_detected!("vpclmulqdq"));
    println!("  PCLMULQDQ: {}", is_x86_feature_detected!("pclmulqdq"));
    println!();

    let has_avx512 = is_x86_feature_detected!("avx512f") && is_x86_feature_detected!("vpclmulqdq");
    let has_avx2 = is_x86_feature_detected!("avx2") && is_x86_feature_detected!("vpclmulqdq");

    let sizes = [20, 24, 26, 28];

    for &log_n in &sizes {
        let n = 1usize << log_n;

        let mut u: Vec<BinaryElem32> = (0..n).map(|i| BinaryElem32::from(i as u32)).collect();
        let mut w: Vec<BinaryElem32> = (0..n).map(|i| BinaryElem32::from((i * 7) as u32)).collect();
        let lambda = BinaryElem32::from(0x12345678u32);

        let iterations = if log_n >= 26 { 3 } else if log_n >= 24 { 10 } else { 50 };

        println!("2^{} ({} elements, {} iterations):", log_n, n, iterations);

        // Warmup
        fft_butterfly_gf32_sse(&mut u, &mut w, lambda);

        // AVX-512
        if has_avx512 {
            let start = Instant::now();
            for _ in 0..iterations {
                fft_butterfly_gf32_avx512_only(&mut u, &mut w, lambda);
            }
            let time = start.elapsed();
            let ns = time.as_nanos() as f64 / iterations as f64;
            println!("  AVX-512 (8 elem/iter): {:.2} ms ({:.2} ns/elem)", ns / 1_000_000.0, ns / n as f64);
        }

        // AVX2
        if has_avx2 {
            let start = Instant::now();
            for _ in 0..iterations {
                fft_butterfly_gf32_avx2_only(&mut u, &mut w, lambda);
            }
            let time = start.elapsed();
            let ns = time.as_nanos() as f64 / iterations as f64;
            println!("  AVX2 (4 elem/iter):    {:.2} ms ({:.2} ns/elem)", ns / 1_000_000.0, ns / n as f64);
        }

        // SSE
        {
            let start = Instant::now();
            for _ in 0..iterations {
                fft_butterfly_gf32_sse(&mut u, &mut w, lambda);
            }
            let time = start.elapsed();
            let ns = time.as_nanos() as f64 / iterations as f64;
            println!("  SSE (2 elem/iter):     {:.2} ms ({:.2} ns/elem)", ns / 1_000_000.0, ns / n as f64);
        }

        println!();
    }
}
