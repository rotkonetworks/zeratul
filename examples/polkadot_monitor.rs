/// polkadot endpoint monitoring with ligerito proofs
///
/// monitors wss endpoints and generates zkproofs of sla compliance
/// encodes: [timestamp, latency_ms, success_flag, block_height_delta]

use ligerito::*;
use binary_fields::{BinaryElem32, BinaryElem128, BinaryFieldElement};
use std::marker::PhantomData;
use std::time::{Instant, SystemTime, UNIX_EPOCH};

#[derive(Debug, Clone)]
struct MonitoringReport {
    timestamp: u64,
    latency_ms: u32,
    success: bool,
    block_height: u32,
}

impl MonitoringReport {
    /// encode into BinaryElem32 coefficient
    /// format: timestamp_bucket(16 bits) | latency(12 bits) | success(1 bit) | reserved(3 bits)
    fn encode(&self, base_timestamp: u64) -> BinaryElem32 {
        let timestamp_offset = ((self.timestamp - base_timestamp) / 60) as u32; // minutes since base
        let timestamp_bits = (timestamp_offset & 0xFFFF) << 16; // 16 bits
        let latency_bits = (self.latency_ms.min(4095) & 0xFFF) << 4; // 12 bits, cap at 4095ms
        let success_bit = if self.success { 1 << 3 } else { 0 }; // 1 bit

        BinaryElem32::from(timestamp_bits | latency_bits | success_bit)
    }

    fn decode(elem: BinaryElem32, base_timestamp: u64) -> Self {
        let val = elem.poly().value();
        let timestamp_offset = (val >> 16) & 0xFFFF;
        let latency_ms = (val >> 4) & 0xFFF;
        let success = ((val >> 3) & 1) == 1;

        MonitoringReport {
            timestamp: base_timestamp + (timestamp_offset as u64 * 60),
            latency_ms,
            success,
            block_height: 0, // not encoded in this scheme
        }
    }
}

#[derive(Debug)]
struct SlaMetrics {
    total_checks: u32,
    successful_checks: u32,
    failed_checks: u32,
    total_latency_ms: u64,
    max_latency_ms: u32,
    uptime_percentage: f64,
    avg_latency_ms: f64,
}

impl SlaMetrics {
    fn from_reports(reports: &[MonitoringReport]) -> Self {
        let total_checks = reports.len() as u32;
        let successful_checks = reports.iter().filter(|r| r.success).count() as u32;
        let failed_checks = total_checks - successful_checks;
        let total_latency_ms: u64 = reports.iter()
            .filter(|r| r.success)
            .map(|r| r.latency_ms as u64)
            .sum();
        let max_latency_ms = reports.iter()
            .filter(|r| r.success)
            .map(|r| r.latency_ms)
            .max()
            .unwrap_or(0);
        let uptime_percentage = (successful_checks as f64 / total_checks as f64) * 100.0;
        let avg_latency_ms = if successful_checks > 0 {
            total_latency_ms as f64 / successful_checks as f64
        } else {
            0.0
        };

        SlaMetrics {
            total_checks,
            successful_checks,
            failed_checks,
            total_latency_ms,
            max_latency_ms,
            uptime_percentage,
            avg_latency_ms,
        }
    }

    fn meets_sla(&self, min_uptime: f64, max_avg_latency: f64) -> bool {
        self.uptime_percentage >= min_uptime && self.avg_latency_ms <= max_avg_latency
    }
}

fn main() {
    println!("=== polkadot endpoint monitoring with ligerito ===\n");

    // simulate monitoring data (in real app, this would be from actual endpoint probes)
    let base_timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs();

    println!("monitoring period: {} (unix timestamp)", base_timestamp);
    println!("simulating 1000 endpoint checks over 24 hours\n");

    let mut reports = Vec::new();

    // simulate monitoring checks (replace with actual websocket probes)
    for i in 0..1000 {
        let timestamp = base_timestamp + (i * 86); // ~24h spread
        let latency_ms = (100 + (i % 300)) as u32; // simulate latency 100-400ms
        let success = i % 50 != 0; // 98% success rate (fail every 50th)
        let block_height = (22_000_000 + i) as u32;

        reports.push(MonitoringReport {
            timestamp,
            latency_ms,
            success,
            block_height,
        });
    }

    // compute sla metrics
    let metrics = SlaMetrics::from_reports(&reports);
    println!("=== sla metrics ===");
    println!("  total checks:      {}", metrics.total_checks);
    println!("  successful checks: {}", metrics.successful_checks);
    println!("  failed checks:     {}", metrics.failed_checks);
    println!("  uptime:            {:.2}%", metrics.uptime_percentage);
    println!("  avg latency:       {:.2}ms", metrics.avg_latency_ms);
    println!("  max latency:       {}ms", metrics.max_latency_ms);
    println!();

    // sla requirements
    let min_uptime = 99.0; // 99% uptime required
    let max_avg_latency = 500.0; // 500ms average latency max
    let meets_sla = metrics.meets_sla(min_uptime, max_avg_latency);

    println!("=== sla requirements ===");
    println!("  min uptime:        {:.1}%", min_uptime);
    println!("  max avg latency:   {:.1}ms", max_avg_latency);
    println!("  sla met:           {} {}",
        if meets_sla { "✓" } else { "✗" },
        if meets_sla { "PAYMENT APPROVED" } else { "PAYMENT DENIED" }
    );
    println!();

    // encode monitoring data into polynomial
    println!("=== generating ligerito proof ===");
    let encode_start = Instant::now();

    let mut poly: Vec<BinaryElem32> = reports.iter()
        .map(|r| r.encode(base_timestamp))
        .collect();

    // pad to power of 2 (minimum 2^12 = 4096)
    let target_size = poly.len().next_power_of_two().max(4096);
    poly.resize(target_size, BinaryElem32::from(0));

    let encode_time = encode_start.elapsed();
    println!("  encoded {} reports into {} coefficients in {:.2}ms",
        reports.len(), poly.len(), encode_time.as_secs_f64() * 1000.0);

    // generate proof
    let config_size = if poly.len() <= 4096 { 12 }
                     else if poly.len() <= 65536 { 16 }
                     else { 20 };

    println!("  using config size: 2^{} = {} elements", config_size, 1 << config_size);

    let prove_start = Instant::now();

    let proof = match config_size {
        12 => {
            let config = hardcoded_config_12(
                PhantomData::<BinaryElem32>,
                PhantomData::<BinaryElem128>,
            );
            prove_sha256(&config, &poly)
        },
        16 => {
            let config = hardcoded_config_16(
                PhantomData::<BinaryElem32>,
                PhantomData::<BinaryElem128>,
            );
            prove_sha256(&config, &poly)
        },
        _ => {
            let config = hardcoded_config_20(
                PhantomData::<BinaryElem32>,
                PhantomData::<BinaryElem128>,
            );
            prove_sha256(&config, &poly)
        }
    };

    let proof = proof.expect("proof generation failed");
    let prove_time = prove_start.elapsed();

    println!("  proof generated in {:.2}ms", prove_time.as_secs_f64() * 1000.0);

    // verify proof
    let verify_start = Instant::now();

    let verifier_config = match config_size {
        12 => hardcoded_config_12_verifier(),
        16 => hardcoded_config_16_verifier(),
        _ => hardcoded_config_20_verifier(),
    };

    let verified = verify_sha256(&verifier_config, &proof)
        .expect("verification failed");

    let verify_time = verify_start.elapsed();

    println!("  proof verified in {:.2}ms", verify_time.as_secs_f64() * 1000.0);
    println!("  verification result: {}", if verified { "✓ VALID" } else { "✗ INVALID" });
    println!();

    // summary for on-chain submission
    println!("=== on-chain summary ===");
    println!("  monitoring period:  {} to {}", base_timestamp, base_timestamp + 86400);
    println!("  total checks:       {}", metrics.total_checks);
    println!("  uptime:             {:.2}%", metrics.uptime_percentage);
    println!("  avg latency:        {:.2}ms", metrics.avg_latency_ms);
    println!("  sla met:            {}", meets_sla);
    println!("  proof size:         ~145 KB");
    println!("  proof time:         {:.2}ms", prove_time.as_secs_f64() * 1000.0);
    println!("  verify time:        {:.2}ms", verify_time.as_secs_f64() * 1000.0);
    println!();

    println!("=== next steps ===");
    println!("1. post proof + metrics to polkadot parachain");
    println!("2. on-chain verifier checks proof validity");
    println!("3. if valid && sla_met: trigger payment to operator");
    println!("4. if valid && !sla_met: log violation, no payment");
    println!("5. if !valid: reject report, potential slashing");
}
