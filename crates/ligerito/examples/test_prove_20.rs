use ligerito::*;
use ligerito_binary_fields::BinaryElem32;
use std::marker::PhantomData;
use std::time::Instant;

fn main() {
    let log_size = 20;
    let n = 1 << log_size;

    println!("=== Ligerito Prover Benchmark ===");
    println!("Polynomial size: 2^{} = {} elements", log_size, n);
    println!("Field: GF(2^32)");
    println!("Generating polynomial...");

    // Generate random polynomial
    let poly: Vec<BinaryElem32> = (0..n)
        .map(|i| BinaryElem32::from(i as u32))
        .collect();

    // Get config
    let config = hardcoded_config_20(PhantomData::<BinaryElem32>, PhantomData::<BinaryElem32>);

    // Time proving
    println!("\nProving...");
    let start = Instant::now();
    let proof = prove(&config, &poly).expect("prove failed");
    let prove_time = start.elapsed();

    let prove_ms = prove_time.as_secs_f64() * 1000.0;
    let throughput = (n as f64) / prove_time.as_secs_f64();

    println!("\n=== Results ===");
    println!("Proving time: {:.2} ms", prove_ms);
    println!("Throughput: {:.2} elements/sec", throughput);
    println!("Proof size: {} bytes", bincode::serialize(&proof).unwrap().len());
}
