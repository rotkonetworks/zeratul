use ligerito_binary_fields::{BinaryElem32, BinaryElem128};
use ligerito::{prove_sha256, hardcoded_config_20};
use std::marker::PhantomData;
use std::time::Instant;

fn main() {
    let config = hardcoded_config_20(
        PhantomData::<BinaryElem32>,
        PhantomData::<BinaryElem128>,
    );

    let poly: Vec<BinaryElem32> = (0..(1 << 20))
        .map(|i| BinaryElem32::from(i as u32))
        .collect();

    println!("Warming up...");
    let _ = prove_sha256(&config, &poly).unwrap();

    println!("Running benchmark...");
    let mut times = Vec::new();

    for i in 0..5 {
        let start = Instant::now();
        let proof = prove_sha256(&config, &poly).unwrap();
        let elapsed = start.elapsed();
        times.push(elapsed);
        println!("  Run {}: {:?} (proof size: {} bytes)", i + 1, elapsed, proof.size_of());
    }

    let avg = times.iter().map(|d| d.as_millis()).sum::<u128>() / times.len() as u128;
    let min = times.iter().map(|d| d.as_millis()).min().unwrap();
    let max = times.iter().map(|d| d.as_millis()).max().unwrap();

    println!("\nResults for 2^20 proving:");
    println!("  Average: {}ms", avg);
    println!("  Min: {}ms", min);
    println!("  Max: {}ms", max);
    println!("  Threads: {}", rayon::current_num_threads());
}
