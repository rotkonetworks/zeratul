// compression prototype for ligerito proofs
// demonstrates 45-60% size reduction with ~10ms overhead

use ligerito::{prove_sha256, verify_sha256, hardcoded_config_20, hardcoded_config_20_verifier};
use ligerito_binary_fields::{BinaryElem32, BinaryElem128};
use std::marker::PhantomData;
use std::time::Instant;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    println!("=== ligerito compression prototype ===\n");

    // create config for 2^20 elements
    let config = hardcoded_config_20(
        PhantomData::<BinaryElem32>,
        PhantomData::<BinaryElem128>,
    );

    // generate test polynomial
    let poly: Vec<BinaryElem32> = (0..1048576)
        .map(|i| BinaryElem32::from(i as u32))
        .collect();

    println!("polynomial size: 2^20 = 1,048,576 elements\n");

    // ===== uncompressed =====
    println!("--- uncompressed proof ---");

    let start = Instant::now();
    let proof = prove_sha256(&config, &poly)?;
    let prove_time = start.elapsed();

    let serialized = bincode::serialize(&proof)?;
    let uncompressed_size = serialized.len();

    println!("  prove time:  {:.2}ms", prove_time.as_secs_f64() * 1000.0);
    println!("  proof size:  {} bytes ({:.2} KB)", uncompressed_size, uncompressed_size as f64 / 1024.0);

    let verifier_config = hardcoded_config_20_verifier();
    let start = Instant::now();
    let valid = verify_sha256(&verifier_config, &proof)?;
    let verify_time = start.elapsed();

    println!("  verify time: {:.2}ms", verify_time.as_secs_f64() * 1000.0);
    println!("  valid:       {}", valid);
    println!("  total time:  {:.2}ms\n", (prove_time + verify_time).as_secs_f64() * 1000.0);

    // ===== compressed (zstd level 3) =====
    println!("--- compressed proof (zstd level 3) ---");

    // compress
    let start = Instant::now();
    let compressed = zstd::encode_all(&serialized[..], 3)?;
    let compress_time = start.elapsed();
    let compressed_size = compressed.len();

    println!("  compress time: {:.2}ms", compress_time.as_secs_f64() * 1000.0);
    println!("  compressed size: {} bytes ({:.2} KB)", compressed_size, compressed_size as f64 / 1024.0);
    println!("  compression ratio: {:.1}%", (1.0 - compressed_size as f64 / uncompressed_size as f64) * 100.0);

    // decompress
    let start = Instant::now();
    let decompressed = zstd::decode_all(&compressed[..])?;
    let decompress_time = start.elapsed();

    println!("  decompress time: {:.2}ms", decompress_time.as_secs_f64() * 1000.0);

    // verify decompressed proof
    let proof_decompressed: ligerito::FinalizedLigeritoProof<BinaryElem32, BinaryElem128> =
        bincode::deserialize(&decompressed)?;

    let start = Instant::now();
    let valid_compressed = verify_sha256(&verifier_config, &proof_decompressed)?;
    let verify_time_compressed = start.elapsed();

    println!("  verify time: {:.2}ms", verify_time_compressed.as_secs_f64() * 1000.0);
    println!("  valid:       {}", valid_compressed);

    let total_compressed = prove_time + compress_time + decompress_time + verify_time_compressed;
    println!("  total time:  {:.2}ms\n", total_compressed.as_secs_f64() * 1000.0);

    // ===== comparison =====
    println!("--- comparison ---");

    let size_saved = uncompressed_size - compressed_size;
    let time_overhead = (compress_time + decompress_time).as_secs_f64() * 1000.0;
    let total_overhead = (total_compressed - (prove_time + verify_time)).as_secs_f64() * 1000.0;

    println!("  size reduction:    {} bytes ({:.2} KB, {:.1}%)",
        size_saved,
        size_saved as f64 / 1024.0,
        (1.0 - compressed_size as f64 / uncompressed_size as f64) * 100.0
    );
    println!("  compression overhead: {:.2}ms", time_overhead);
    println!("  total time overhead:  {:.2}ms ({:.1}%)",
        total_overhead,
        (total_overhead / ((prove_time + verify_time).as_secs_f64() * 1000.0)) * 100.0
    );

    // ===== when to use compression =====
    println!("\n--- when to use compression ---");

    let bandwidths = vec![1.0, 5.0, 10.0, 100.0]; // Mbps
    for bw in bandwidths {
        let mbps = bw * 1_000_000.0 / 8.0; // convert to bytes per second
        let uncompressed_tx_time = (uncompressed_size as f64 / mbps) * 1000.0; // ms
        let compressed_tx_time = (compressed_size as f64 / mbps) * 1000.0; // ms
        let time_saved = uncompressed_tx_time - compressed_tx_time - time_overhead;

        println!("  @ {:.0} Mbps: uncompressed tx={:.0}ms, compressed tx={:.0}ms, net savings={:.0}ms {}",
            bw,
            uncompressed_tx_time,
            compressed_tx_time + time_overhead,
            time_saved,
            if time_saved > 0.0 { "✅ use compression" } else { "❌ skip compression" }
        );
    }

    // ===== try other compression levels =====
    println!("\n--- compression levels comparison ---");

    for level in [1, 3, 6, 9, 15] {
        let start = Instant::now();
        let compressed_level = zstd::encode_all(&serialized[..], level)?;
        let compress_time_level = start.elapsed();
        let size_level = compressed_level.len();

        let start = Instant::now();
        let _ = zstd::decode_all(&compressed_level[..])?;
        let decompress_time_level = start.elapsed();

        println!("  level {}: size={:.2} KB ({:.1}%), compress={:.2}ms, decompress={:.2}ms",
            level,
            size_level as f64 / 1024.0,
            (1.0 - size_level as f64 / uncompressed_size as f64) * 100.0,
            compress_time_level.as_secs_f64() * 1000.0,
            decompress_time_level.as_secs_f64() * 1000.0
        );
    }

    println!("\n=== recommendation ===");
    println!("level 3 (default): best balance of speed and size");
    println!("level 1 (fast): use if cpu-bound");
    println!("level 9+ (max): use if size-critical and have time\n");

    Ok(())
}
