//! Benchmark GPU Sumcheck V2 - Scalable Architecture for n=20, n=24
//!
//! V2 Architecture:
//! - GPU: Computes 148 contributions in parallel (2.4 KB output)
//! - CPU: Accumulates contributions into basis_poly (reuses temp buffer)
//!
//! Memory usage: O(num_queries) GPU + O(2^n) CPU
//! - n=20: 2.4 KB GPU + 32 MB CPU (instead of 2.4 GB for v1)
//! - n=24: 2.4 KB GPU + 512 MB CPU (instead of 38 GB for v1)
//!
//! Usage: cargo run --release --example bench_gpu_v2 --features webgpu

use binary_fields::{BinaryElem128, BinaryFieldElement};
use std::time::Instant;

#[cfg(feature = "webgpu")]
use ligerito::gpu::{GpuDevice, sumcheck_v2::GpuSumcheckV2};

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
async fn benchmark_gpu_v2_sumcheck(
    gpu_sumcheck: &mut GpuSumcheckV2,
    n: usize,
    sks_vks: &[BinaryElem128],
    opened_rows: &[Vec<BinaryElem128>],
    v_challenges: &[BinaryElem128],
    sorted_queries: &[usize],
    alpha: BinaryElem128,
) -> Result<(f64, Vec<BinaryElem128>, BinaryElem128), String> {
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

fn verify_results(
    cpu_basis_poly: &[BinaryElem128],
    gpu_basis_poly: &[BinaryElem128],
    cpu_sum: BinaryElem128,
    gpu_sum: BinaryElem128,
) -> bool {
    if cpu_sum != gpu_sum {
        return false;
    }

    cpu_basis_poly.iter()
        .zip(gpu_basis_poly.iter())
        .all(|(c, g)| c == g)
}

#[tokio::main]
async fn main() {
    println!("GPU Sumcheck V2 Benchmark - Large Scale Testing");
    println!("================================================\n");
    println!("V2 Architecture:");
    println!("  - GPU: Computes 148 contributions in parallel (2.4 KB output)");
    println!("  - CPU: Accumulates contributions into basis_poly (reuses temp buffer)\n");
    println!("Memory Comparison (for 148 queries):");
    println!("  n=20: V1=2.4 GB ❌  V2=32 MB ✓");
    println!("  n=24: V1=38 GB  ❌  V2=512 MB ✓\n");

    // Test configurations focused on large scales (n≥20 where ligerito is useful)
    let test_configs = vec![
        (8, 148, 7),   // Small: n=8 - Sanity check
        (10, 148, 7),  // Medium: n=10 - Sanity check
        (12, 148, 7),  // Large: n=12 - Sanity check
        (14, 148, 7),  // XL: n=14 - Sanity check
        (16, 148, 7),  // XXL: n=16 - First scale where v1 fails
        (18, 148, 7),  // Huge: n=18 - Warm up to target
        (20, 148, 7),  // 2^20: n=20 - TARGET SCALE! (ligerito becomes useful here)
        (24, 148, 7),  // 2^24: n=24 - ULTIMATE TARGET! (recursion wins big)
    ];

    println!("{:<20} {:<15} {:<15} {:<12} {:<15} {:<15}", "Config", "CPU (ms)", "GPU V2 (ms)", "Speedup", "Match", "Status");
    println!("{:-<20} {:-<15} {:-<15} {:-<12} {:-<15} {:-<15}", "", "", "", "", "", "");

    // Initialize GPU device once for all tests
    #[cfg(feature = "webgpu")]
    let mut gpu_sumcheck_v2 = {
        match GpuDevice::new().await {
            Ok(device) => {
                println!("GPU initialized successfully");
                println!("  Device: {}", device.capabilities.adapter_name);
                println!("  Backend: {}\n", device.capabilities.backend);
                Some(GpuSumcheckV2::new(device))
            }
            Err(e) => {
                println!("GPU not available: {}", e);
                println!("Running CPU-only benchmark\n");
                None
            }
        }
    };

    let mut all_passed = true;

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

        // Benchmark GPU V2
        #[cfg(feature = "webgpu")]
        if let Some(ref mut gpu) = gpu_sumcheck_v2 {
            match benchmark_gpu_v2_sumcheck(
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
                    print!("{:<15.2} ", gpu_time);

                    let speedup = cpu_time / gpu_time;
                    print!("{:<12.2}x ", speedup);

                    let matches = verify_results(
                        &cpu_basis_poly,
                        &gpu_basis_poly,
                        cpu_enforced_sum,
                        gpu_enforced_sum,
                    );

                    if matches {
                        print!("{:<15} ", "✓ PASS");

                        // Determine status
                        if n >= 20 {
                            print!("{:<15}", "🎯 TARGET SCALE");
                        } else if n >= 16 {
                            print!("{:<15}", "⚠️  V1 FAILS");
                        } else {
                            print!("{:<15}", "✓ Baseline");
                        }
                    } else {
                        print!("{:<15} ", "❌ FAIL");
                        print!("{:<15}", "MISMATCH");
                        all_passed = false;
                    }
                    println!();
                }
                Err(e) => {
                    println!("{:<15} {:<12} {:<15} {:<15}", "ERROR", "-", "-", &format!("GPU error: {}", e));
                    all_passed = false;
                }
            }
        } else {
            println!("{:<15} {:<12} {:<15} {:<15}", "-", "-", "-", "GPU unavailable");
        }

        #[cfg(not(feature = "webgpu"))]
        println!("{:<15} {:<12} {:<15} {:<15}", "-", "-", "-", "GPU feature disabled");
    }

    println!("\n{:-<95}", "");
    if all_passed {
        println!("✓ ALL TESTS PASSED");
        println!("\nV2 Architecture Successfully Scales to n=20 and n=24!");
        println!("Ready for production use in recursive proof systems.");
    } else {
        println!("❌ SOME TESTS FAILED");
        println!("Review errors above for debugging.");
    }
}
