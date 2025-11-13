/// Benchmark different k values at n=20 to find optimal GPU parametrization
///
/// Tests:
/// - k=6:  64-element dot products (current default)
/// - k=8:  256-element dot products (common GPU workgroup size)
/// - k=10: 1024-element dot products (max common workgroup size)

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

fn main() {
    println!("╔═══════════════════════════════════════════════════════════════════════╗");
    println!("║  GPU K-Value Optimization Benchmark (n=20, varying k)                ║");
    println!("╚═══════════════════════════════════════════════════════════════════════╝\n");

    let n = 20;
    let num_queries = 148;

    // Test k=6 (current default)
    println!("═══ Test 1: k=6 (64-element dot products) ═══");
    test_config(n, num_queries, 6);

    // Test k=8 (256-element dot products)
    println!("\n═══ Test 2: k=8 (256-element dot products) ═══");
    test_config(n, num_queries, 8);

    // Test k=10 (1024-element dot products, square matrix)
    println!("\n═══ Test 3: k=10 (1024-element dot products, SQUARE) ═══");
    test_config(n, num_queries, 10);

    println!("\n╔═══════════════════════════════════════════════════════════════════════╗");
    println!("║  Benchmark Complete                                                   ║");
    println!("╚═══════════════════════════════════════════════════════════════════════╝");
}

fn test_config(n: usize, num_queries: usize, k: usize) {
    println!("Config: k={}", k);
    println!("  Dims: {} × {} (2^{} × 2^{})", 1 << (n - k), 1 << k, n - k, k);
    println!("  Dot product size: {} elements", 1 << k);
    println!("  Queries: {}", num_queries);

    let (sks_vks, opened_rows, v_challenges, sorted_queries, alpha) =
        generate_test_data(n, num_queries, k);

    // CPU sumcheck
    let start = Instant::now();
    let (cpu_result, _enforced_sum) = cpu_induce_sumcheck_poly(
        n,
        &sks_vks,
        &opened_rows,
        &v_challenges,
        &sorted_queries,
        alpha,
    );
    let cpu_time = start.elapsed();
    println!("  CPU sumcheck: {:.2?}", cpu_time);

    // GPU sumcheck
    #[cfg(feature = "webgpu")]
    {
        let rt = tokio::runtime::Runtime::new().unwrap();
        rt.block_on(async {
            match GpuDevice::new().await {
                Ok(device) => {
                    println!("  GPU: {} ({})",
                        device.capabilities.adapter_name,
                        device.capabilities.backend);

                    let mut gpu_sumcheck = GpuSumcheck::new(device);

                    let start = Instant::now();
                    let gpu_result = gpu_sumcheck.induce_sumcheck_poly(
                        n,
                        &sks_vks,
                        &opened_rows,
                        &v_challenges,
                        &sorted_queries,
                        alpha,
                    ).await;
                    let gpu_time = start.elapsed();

                    match gpu_result {
                        Ok((result, _enforced_sum)) => {
                            println!("  GPU sumcheck: {:.2?}", gpu_time);

                            // Verify results match
                            let matches = cpu_result.iter().zip(result.iter())
                                .all(|(a, b)| a == b);

                            if matches {
                                let speedup = cpu_time.as_secs_f64() / gpu_time.as_secs_f64();
                                println!("  ✓ Results match");
                                println!("  Speedup: {:.2}x {}",
                                    speedup,
                                    if speedup > 1.0 { "🚀" } else { "⚠️" });
                            } else {
                                println!("  ✗ Results MISMATCH!");
                            }
                        }
                        Err(e) => println!("  GPU error: {}", e),
                    }
                }
                Err(e) => {
                    println!("  GPU unavailable: {}", e);
                    println!("  (CPU-only mode)");
                }
            }
        });
    }

    #[cfg(not(feature = "webgpu"))]
    {
        println!("  GPU: Not compiled (build with --features webgpu)");
    }
}
