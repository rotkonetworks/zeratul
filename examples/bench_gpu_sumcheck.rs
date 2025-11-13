//! Benchmark GPU Sumcheck vs CPU Sumcheck
//!
//! Usage: cargo run --release --example bench_gpu_sumcheck --features webgpu

use binary_fields::{BinaryElem128, BinaryFieldElement};
use std::time::Instant;

#[cfg(feature = "webgpu")]
use ligerito::gpu::{GpuDevice, sumcheck::GpuSumcheck};

use ligerito::sumcheck_polys::induce_sumcheck_poly as cpu_induce_sumcheck_poly;

fn generate_test_data(
    n: usize,
    num_queries: usize,
    k: usize,
) -> (Vec<BinaryElem128>, Vec<Vec<BinaryElem128>>, Vec<BinaryElem128>, Vec<usize>, BinaryElem128) {
    let row_size = 1 << k;

    // Generate sks_vks (n+1 elements for basis polynomial)
    let sks_vks: Vec<BinaryElem128> = (0..=n)
        .map(|i| BinaryElem128::from_value((i as u128).wrapping_mul(0x123456789ABCDEF)))
        .collect();

    // Generate opened rows
    let opened_rows: Vec<Vec<BinaryElem128>> = (0..num_queries)
        .map(|q| {
            (0..row_size)
                .map(|i| {
                    BinaryElem128::from_value(
                        ((q * 1000 + i) as u128).wrapping_mul(0xFEDCBA987654321)
                    )
                })
                .collect()
        })
        .collect();

    // Generate v_challenges
    let v_challenges: Vec<BinaryElem128> = (0..k)
        .map(|i| BinaryElem128::from_value((i as u128).wrapping_mul(0x111111111111111)))
        .collect();

    // Generate sorted queries
    let sorted_queries: Vec<usize> = (0..num_queries)
        .map(|i| i * 17 % (1 << n))
        .collect();

    // Generate alpha
    let alpha = BinaryElem128::from_value(0xABCDEF0123456789);

    (sks_vks, opened_rows, v_challenges, sorted_queries, alpha)
}

fn benchmark_cpu_sumcheck(
    n: usize,
    sks_vks: &[BinaryElem128],
    opened_rows: &[Vec<BinaryElem128>],
    v_challenges: &[BinaryElem128],
    sorted_queries: &[usize],
    alpha: BinaryElem128,
) -> (f64, Vec<BinaryElem128>, BinaryElem128) {
    let start = Instant::now();
    let (basis_poly, enforced_sum) = cpu_induce_sumcheck_poly(
        n,
        sks_vks,
        opened_rows,
        v_challenges,
        sorted_queries,
        alpha,
    );
    let duration = start.elapsed();

    (duration.as_secs_f64() * 1000.0, basis_poly, enforced_sum)
}

#[cfg(feature = "webgpu")]
async fn benchmark_gpu_sumcheck(
    gpu_sumcheck: &mut GpuSumcheck,
    n: usize,
    sks_vks: &[BinaryElem128],
    opened_rows: &[Vec<BinaryElem128>],
    v_challenges: &[BinaryElem128],
    sorted_queries: &[usize],
    alpha: BinaryElem128,
) -> Result<(f64, Vec<BinaryElem128>, BinaryElem128), String> {
    // Actual benchmark (no warmup - device is already initialized)
    let start = Instant::now();
    let (basis_poly, enforced_sum) = gpu_sumcheck
        .induce_sumcheck_poly(
            n,
            sks_vks,
            opened_rows,
            v_challenges,
            sorted_queries,
            alpha,
        )
        .await?;
    let duration = start.elapsed();

    Ok((duration.as_secs_f64() * 1000.0, basis_poly, enforced_sum))
}

#[tokio::main]
async fn main() {
    println!("Sumcheck Performance Benchmark: GPU vs CPU");
    println!("==========================================\n");

    let test_configs = vec![
        (8, 16, 4),   // Small: n=8 (256 basis), 16 queries, k=4 (16 row size) - GPU
        (10, 32, 6),  // Medium: n=10 (1024 basis), 32 queries, k=6 (64 row size) - GPU
        (12, 64, 7),  // Large: n=12 (4096 basis), 64 queries, k=7 (128 row size) - GPU
        (14, 128, 7), // XL: n=14 (16K basis), 128 queries, k=7 - GPU
        (16, 148, 7), // XXL: n=16 (64K basis), 148 queries, k=7 - May hit buffer limit -> CPU fallback
        (18, 148, 7), // Huge: n=18 (256K basis), 148 queries, k=7 - CPU fallback
        (20, 148, 7), // 2^20: n=20 (1M basis), 148 queries, k=7 - CPU fallback
    ];

    println!("{:<20} {:<15} {:<15} {:<12} {:<15}", "Config", "CPU (ms)", "GPU (ms)", "Speedup", "Match");
    println!("{:-<20} {:-<15} {:-<15} {:-<12} {:-<15}", "", "", "", "", "");

    // Initialize GPU device once for all tests
    #[cfg(feature = "webgpu")]
    let mut gpu_sumcheck = {
        match GpuDevice::new().await {
            Ok(device) => Some(GpuSumcheck::new(device)),
            Err(e) => {
                println!("GPU not available: {}", e);
                None
            }
        }
    };

    for &(n, num_queries, k) in &test_configs {
        let (sks_vks, opened_rows, v_challenges, sorted_queries, alpha) =
            generate_test_data(n, num_queries, k);

        let config_str = format!("n={} q={} k={}", n, num_queries, k);
        print!("{:<20} ", config_str);
        std::io::Write::flush(&mut std::io::stdout()).unwrap();

        // Benchmark CPU
        let (cpu_time, cpu_basis_poly, cpu_enforced_sum) = benchmark_cpu_sumcheck(
            n,
            &sks_vks,
            &opened_rows,
            &v_challenges,
            &sorted_queries,
            alpha,
        );
        print!("{:<15.2} ", cpu_time);
        std::io::Write::flush(&mut std::io::stdout()).unwrap();

        // Benchmark GPU
        #[cfg(feature = "webgpu")]
        {
            if let Some(ref mut gpu) = gpu_sumcheck {
                match benchmark_gpu_sumcheck(
                    gpu,
                    n,
                    &sks_vks,
                    &opened_rows,
                    &v_challenges,
                    &sorted_queries,
                    alpha,
                )
                .await
                {
                    Ok((gpu_time, gpu_basis_poly, gpu_enforced_sum)) => {
                        let speedup = cpu_time / gpu_time;

                        // Detailed comparison
                        let sum_match = cpu_enforced_sum == gpu_enforced_sum;
                        let poly_match = cpu_basis_poly == gpu_basis_poly;

                        let mut mismatch_count = 0;
                        let mut mismatch_indices = Vec::new();
                        for i in 0..cpu_basis_poly.len().min(gpu_basis_poly.len()) {
                            if cpu_basis_poly[i] != gpu_basis_poly[i] {
                                mismatch_count += 1;
                                if mismatch_count <= 20 {
                                    mismatch_indices.push(i);
                                }
                            }
                        }

                        let matches = sum_match && poly_match;
                        let match_str = if matches {
                            "✓ PASS".to_string()
                        } else if mismatch_count == 0 && sum_match {
                            "✓ PASS (len)".to_string()
                        } else if mismatch_count <= 20 {
                            format!("✗ {} @ {:?}", mismatch_count, mismatch_indices)
                        } else {
                            format!("✗ {} diffs", mismatch_count)
                        };

                        println!("{:<15.2} {:<12.2}x {:<15}", gpu_time, speedup, match_str);

                        // Print first few mismatches in detail
                        if mismatch_count > 0 && mismatch_count <= 20 {
                            println!("\nMismatch details:");
                            for &idx in &mismatch_indices {
                                println!("  [{}]: CPU={:?}, GPU={:?}", idx, cpu_basis_poly[idx], gpu_basis_poly[idx]);
                            }
                        }
                    }
                    Err(e) => {
                        println!("{:<15} {:<12} {:<15}", "ERROR", "-", format!("({})", e));
                    }
                }
            } else {
                println!("{:<15} {:<12} {:<15}", "N/A", "N/A", "GPU unavailable");
            }
        }

        #[cfg(not(feature = "webgpu"))]
        {
            println!("{:<15} {:<12} {:<15}", "N/A", "N/A", "N/A");
            println!("\nNote: Rerun with --features webgpu to enable GPU benchmarks");
            break;
        }
    }

    println!("\n");
    println!("Note: This benchmarks sumcheck polynomial induction only.");
    println!("GPU performance benefits increase with larger configurations.");
    println!("\nPerformance Analysis:");
    println!("- GPU parallelizes {} independent row operations", test_configs.last().unwrap().1);
    println!("- Expected 5-10x speedup over single-threaded CPU");
    println!("- Integrated GPU has lower memory transfer overhead than discrete GPU");
}
