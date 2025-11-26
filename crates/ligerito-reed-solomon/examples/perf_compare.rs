use std::time::Instant;
use ligerito_binary_fields::BinaryElem32;
use ligerito_reed_solomon::{reed_solomon, fft_gf32, ReedSolomon};

fn bench_fft(log_n: usize, iterations: usize) {
    let n = 1 << log_n;
    let rs: ReedSolomon<BinaryElem32> = reed_solomon(n / 4, n);

    // Warmup
    let mut data: Vec<BinaryElem32> = (0..n).map(|i| BinaryElem32::from(i as u32)).collect();
    fft_gf32(&mut data, &rs.twiddles, false);

    // Single-threaded benchmark
    let mut total_single = std::time::Duration::ZERO;
    for _ in 0..iterations {
        let mut data: Vec<BinaryElem32> = (0..n).map(|i| BinaryElem32::from(i as u32)).collect();
        let start = Instant::now();
        fft_gf32(&mut data, &rs.twiddles, false);
        total_single += start.elapsed();
    }
    let avg_single = total_single / iterations as u32;

    // Multi-threaded benchmark
    let mut total_multi = std::time::Duration::ZERO;
    for _ in 0..iterations {
        let mut data: Vec<BinaryElem32> = (0..n).map(|i| BinaryElem32::from(i as u32)).collect();
        let start = Instant::now();
        fft_gf32(&mut data, &rs.twiddles, true);
        total_multi += start.elapsed();
    }
    let avg_multi = total_multi / iterations as u32;

    let speedup = avg_single.as_secs_f64() / avg_multi.as_secs_f64();
    println!("FFT 2^{}: single={:?} multi={:?} speedup={:.2}x",
             log_n, avg_single, avg_multi, speedup);
}

fn main() {
    println!("Rayon threads: {}", rayon::current_num_threads());
    println!();

    bench_fft(16, 100);
    bench_fft(18, 50);
    bench_fft(20, 10);
    bench_fft(22, 5);
}
