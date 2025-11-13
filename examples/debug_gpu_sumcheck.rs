//! Debug GPU sumcheck to find bugs
//!
//! Usage: cargo run --release --example debug_gpu_sumcheck --features webgpu

use binary_fields::{BinaryElem128, BinaryFieldElement};

#[cfg(feature = "webgpu")]
use ligerito::gpu::{GpuDevice, sumcheck::GpuSumcheck};

use ligerito::sumcheck_polys::induce_sumcheck_poly as cpu_induce_sumcheck_poly;

#[tokio::main]
async fn main() {
    println!("GPU Sumcheck Debug");
    println!("==================\n");

    // Test with 16 queries
    let n = 8; // 2^8 = 256 basis elements (same as benchmark)
    let k = 4; // 2^4 = 16 row size (same as benchmark)
    let num_queries = 16; // Test with full set

    println!("Test parameters:");
    println!("  n={} (basis size: {})", n, 1 << n);
    println!("  k={} (row size: {})", k, 1 << k);
    println!("  num_queries={}\n", num_queries);

    // Generate simple test data
    let row_size = 1 << k;

    let sks_vks: Vec<BinaryElem128> = (0..=n)
        .map(|i| BinaryElem128::from_value((i as u128) + 1))
        .collect();

    let opened_rows: Vec<Vec<BinaryElem128>> = (0..num_queries)
        .map(|q| {
            (0..row_size)
                .map(|i| BinaryElem128::from_value(((q * 10 + i) as u128) + 1))
                .collect()
        })
        .collect();

    let v_challenges: Vec<BinaryElem128> = (0..k)
        .map(|i| BinaryElem128::from_value((i as u128) + 1))
        .collect();

    // Use same query pattern as benchmark
    let sorted_queries: Vec<usize> = (0..num_queries)
        .map(|i| i * 17 % (1 << n))
        .collect();

    let alpha = BinaryElem128::from_value(42);

    // Compute basis indices like GPU code does
    let basis_indices: Vec<usize> = sorted_queries
        .iter()
        .map(|&query| {
            let query_mod = query % (1 << n);
            let qf = BinaryElem128::from_bits(query_mod as u64);
            (0..(1 << n))
                .find(|&i| BinaryElem128::from_bits(i as u64) == qf)
                .unwrap_or(0)
        })
        .collect();

    println!("Input data:");
    println!("  sks_vks: {:?}", sks_vks);
    println!("  opened_rows[0]: {:?}", opened_rows[0]);
    if opened_rows.len() > 1 {
        println!("  opened_rows[1]: {:?}", opened_rows[1]);
    }
    println!("  v_challenges: {:?}", v_challenges);
    println!("  sorted_queries: {:?}", sorted_queries);
    println!("  basis_indices: {:?}", basis_indices);
    println!("  alpha: {:?}\n", alpha);

    // Compute expected dot products for ALL queries
    println!("Expected CPU dot products:");
    let mut cpu_dots: Vec<BinaryElem128> = Vec::new();
    for (q, row) in opened_rows.iter().enumerate() {
        let mut current: Vec<BinaryElem128> = row.clone();
        for &r in v_challenges.iter().rev() {
            let half = current.len() / 2;
            let one_minus_r = BinaryElem128::one().add(&r);
            for i in 0..half {
                current[i] = current[2*i].mul(&one_minus_r)
                            .add(&current[2*i+1].mul(&r));
            }
            current.truncate(half);
        }
        let dot = current[0];
        cpu_dots.push(dot);
        println!("  Query {}: {:?}", q, dot);
    }
    println!();

    // Compute expected contributions (dot * alpha^i)
    println!("Expected contributions (dot * alpha^i):");
    let mut alpha_pow = BinaryElem128::one();
    for (i, &dot) in cpu_dots.iter().enumerate().take(8) {
        let contribution = dot.mul(&alpha_pow);
        println!("  Query {}: {:?} * {:?} = {:?}", i, dot, alpha_pow, contribution);
        alpha_pow = alpha_pow.mul(&alpha);
    }
    println!();

    // CPU version
    println!("Running CPU version...");
    let (cpu_basis_poly, cpu_enforced_sum) = cpu_induce_sumcheck_poly(
        n,
        &sks_vks,
        &opened_rows,
        &v_challenges,
        &sorted_queries,
        alpha,
    );

    println!("CPU results:");
    println!("  enforced_sum: {:?}", cpu_enforced_sum);
    println!("  basis_poly[0]: {:?}", cpu_basis_poly[0]);
    println!("  basis_poly[1]: {:?}", cpu_basis_poly[1]);
    println!("  basis_poly length: {}\n", cpu_basis_poly.len());

    // GPU version
    #[cfg(feature = "webgpu")]
    {
        println!("Running GPU version...");
        let device = match GpuDevice::new().await {
            Ok(d) => d,
            Err(e) => {
                eprintln!("GPU not available: {}", e);
                return;
            }
        };

        let mut gpu_sumcheck = GpuSumcheck::new(device);

        match gpu_sumcheck
            .induce_sumcheck_poly(
                n,
                &sks_vks,
                &opened_rows,
                &v_challenges,
                &sorted_queries,
                alpha,
            )
            .await
        {
            Ok((gpu_basis_poly, gpu_enforced_sum)) => {
                println!("GPU results:");
                println!("  enforced_sum: {:?}", gpu_enforced_sum);
                println!("  basis_poly[0]: {:?}", gpu_basis_poly[0]);
                println!("  basis_poly[1]: {:?}", gpu_basis_poly[1]);
                println!("  basis_poly length: {}\n", gpu_basis_poly.len());

                println!("Comparison:");
                println!("  enforced_sum match: {}", cpu_enforced_sum == gpu_enforced_sum);
                println!("  basis_poly[0] match: {}", cpu_basis_poly[0] == gpu_basis_poly[0]);
                println!("  basis_poly[1] match: {}", cpu_basis_poly[1] == gpu_basis_poly[1]);

                let mut mismatches = 0;
                for i in 0..cpu_basis_poly.len() {
                    if cpu_basis_poly[i] != gpu_basis_poly[i] {
                        mismatches += 1;
                        if mismatches <= 5 {
                            println!("  Mismatch at [{}]: CPU={:?}, GPU={:?}",
                                i, cpu_basis_poly[i], gpu_basis_poly[i]);
                        }
                    }
                }
                println!("  Total mismatches: {} / {}", mismatches, cpu_basis_poly.len());
            }
            Err(e) => {
                eprintln!("GPU computation failed: {}", e);
            }
        }
    }

    #[cfg(not(feature = "webgpu"))]
    {
        println!("WebGPU feature not enabled. Rerun with --features webgpu");
    }
}
