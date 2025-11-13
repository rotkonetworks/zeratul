/// Comprehensive benchmark for k=6 configuration across CPU/GPU
///
/// Tests proving and verification performance at different scales
/// Records results with hardware specs for documentation

use ligerito_binary_fields::{BinaryElem32, BinaryElem128};
use ligerito::{prove, verify, configs::{
    hardcoded_config_20, hardcoded_config_20_verifier,
    hardcoded_config_24, hardcoded_config_24_verifier,
    hardcoded_config_28, hardcoded_config_28_verifier,
    hardcoded_config_30, hardcoded_config_30_verifier,
}};
use std::marker::PhantomData;
use std::time::Instant;

#[cfg(feature = "webgpu")]
use ligerito::gpu::GpuDevice;

fn main() {
    println!("╔═══════════════════════════════════════════════════════════════════════╗");
    println!("║  Ligerito Comprehensive Benchmark (k=6 default config)               ║");
    println!("╚═══════════════════════════════════════════════════════════════════════╝\n");

    // Detect hardware
    print_hardware_info();

    // Run benchmarks
    println!("\n╔═══════════════════════════════════════════════════════════════════════╗");
    println!("║  Benchmark: n=20 (2^20 = 1M elements, ~4 MB)                          ║");
    println!("╚═══════════════════════════════════════════════════════════════════════╝\n");
    benchmark_scale(
        20,
        hardcoded_config_20(PhantomData, PhantomData),
        hardcoded_config_20_verifier()
    );

    println!("\n╔═══════════════════════════════════════════════════════════════════════╗");
    println!("║  Benchmark: n=24 (2^24 = 16M elements, ~64 MB)                        ║");
    println!("╚═══════════════════════════════════════════════════════════════════════╝\n");
    benchmark_scale(
        24,
        hardcoded_config_24(PhantomData, PhantomData),
        hardcoded_config_24_verifier()
    );

    println!("\n╔═══════════════════════════════════════════════════════════════════════╗");
    println!("║  Benchmark: n=28 (2^28 = 268M elements, ~1 GB)                        ║");
    println!("╚═══════════════════════════════════════════════════════════════════════╝\n");
    benchmark_scale(
        28,
        hardcoded_config_28(PhantomData, PhantomData),
        hardcoded_config_28_verifier()
    );

    println!("\n╔═══════════════════════════════════════════════════════════════════════╗");
    println!("║  Benchmark: n=30 (2^30 = 1B elements, ~4 GB)                          ║");
    println!("╚═══════════════════════════════════════════════════════════════════════╝\n");
    benchmark_scale(
        30,
        hardcoded_config_30(PhantomData, PhantomData),
        hardcoded_config_30_verifier()
    );

    println!("\n╔═══════════════════════════════════════════════════════════════════════╗");
    println!("║  Benchmark Complete                                                   ║");
    println!("╚═══════════════════════════════════════════════════════════════════════╝");
}

fn print_hardware_info() {
    println!("Hardware Configuration:");

    // CPU info
    #[cfg(target_os = "linux")]
    {
        if let Ok(cpuinfo) = std::fs::read_to_string("/proc/cpuinfo") {
            if let Some(model) = cpuinfo.lines()
                .find(|l| l.starts_with("model name"))
                .and_then(|l| l.split(':').nth(1))
            {
                println!("  CPU: {}", model.trim());
            }

            let cpu_count = cpuinfo.lines()
                .filter(|l| l.starts_with("processor"))
                .count();
            println!("  Cores: {}", cpu_count);
        }
    }

    // Memory info
    #[cfg(target_os = "linux")]
    {
        if let Ok(meminfo) = std::fs::read_to_string("/proc/meminfo") {
            if let Some(total) = meminfo.lines()
                .find(|l| l.starts_with("MemTotal"))
                .and_then(|l| l.split_whitespace().nth(1))
                .and_then(|s| s.parse::<u64>().ok())
            {
                println!("  RAM: {:.1} GB", total as f64 / 1024.0 / 1024.0);
            }
        }
    }

    // GPU info
    #[cfg(feature = "webgpu")]
    {
        let rt = tokio::runtime::Runtime::new().unwrap();
        rt.block_on(async {
            match GpuDevice::new().await {
                Ok(device) => {
                    println!("  GPU: {}", device.capabilities.adapter_name);
                    println!("  Backend: {}", device.capabilities.backend);
                    println!("  Max Buffer: {} MB", device.capabilities.max_buffer_size / (1024 * 1024));
                }
                Err(_) => {
                    println!("  GPU: Not available");
                }
            }
        });
    }

    #[cfg(not(feature = "webgpu"))]
    {
        println!("  GPU: Not compiled (build with --features webgpu)");
    }
}

fn benchmark_scale(
    n: usize,
    prover_config: ligerito::data_structures::ProverConfig<BinaryElem32, BinaryElem128>,
    verifier_config: ligerito::data_structures::VerifierConfig,
) {
    let size = 1 << n;
    let size_mb = (size * 4) / (1024 * 1024);

    println!("Configuration:");
    println!("  n = {}", n);
    println!("  k = 6 (default)");
    println!("  Input size: {} MB", size_mb);

    // Check if this is too large
    if n >= 28 {
        let size_gb = size_mb as f64 / 1024.0;
        println!("  WARNING: ~{:.1} GB required", size_gb);

        if n >= 30 {
            println!("  Skipping n={} (requires ~{:.0} GB RAM)", n, size_gb);
            return;
        }
    }

    // Generate test data
    print!("Generating test data ({} elements)...", size);
    std::io::Write::flush(&mut std::io::stdout()).unwrap();

    let gen_start = Instant::now();
    let table: Vec<BinaryElem32> = (0..size)
        .map(|i| BinaryElem32::from((i % (u32::MAX as usize)) as u32))
        .collect();
    println!(" {:.2?}\n", gen_start.elapsed());

    // CPU Proving
    print!("Proving (CPU)...");
    std::io::Write::flush(&mut std::io::stdout()).unwrap();
    let start = Instant::now();
    let proof = match prove(&prover_config, &table) {
        Ok(p) => p,
        Err(e) => {
            println!(" Failed: {:?}", e);
            return;
        }
    };
    let prove_time = start.elapsed();
    println!(" {:.2?}", prove_time);

    let proof_size = proof.size_of();
    println!("  Proof size: {} bytes ({:.2} KB)", proof_size, proof_size as f64 / 1024.0);

    // CPU Verification
    print!("Verifying (CPU)...");
    std::io::Write::flush(&mut std::io::stdout()).unwrap();
    let start = Instant::now();
    let verify_result = verify(&verifier_config, &proof);
    let verify_time = start.elapsed();

    match verify_result {
        Ok(true) => println!(" {:.2?} ✓", verify_time),
        Ok(false) => println!(" {:.2?} ✗ INVALID", verify_time),
        Err(e) => println!(" Failed: {:?}", e),
    }

    println!("\nResults Summary (n={}):", n);
    println!("  Prove:  {:.2?}", prove_time);
    println!("  Verify: {:.2?}", verify_time);
    println!("  Proof:  {:.2} KB", proof_size as f64 / 1024.0);
}
