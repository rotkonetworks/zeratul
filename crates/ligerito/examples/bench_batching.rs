//! Benchmark constraint batching: GF(2^32) vs GF(2^128)

use ligerito_binary_fields::{BinaryElem32, BinaryElem128, BinaryFieldElement};
use std::time::Instant;

fn main() {
    let num_constraints = 1_000_000;
    
    println!("=== Constraint Batching Benchmark ===");
    println!("Number of constraints: {}", num_constraints);
    println!();
    
    // Generate random constraints
    let constraints: Vec<u32> = (0..num_constraints)
        .map(|i| (i * 0x12345678) as u32)
        .collect();
    
    // Benchmark GF(2^32) batching
    let challenge_32 = BinaryElem32::from(0xDEADBEEFu32);
    let start = Instant::now();
    
    let mut acc_32 = BinaryElem32::zero();
    let mut power_32 = BinaryElem32::one();
    for &c in &constraints {
        let term = BinaryElem32::from(c).mul(&power_32);
        acc_32 = acc_32.add(&term);
        power_32 = power_32.mul(&challenge_32);
    }
    
    let time_32 = start.elapsed();
    println!("GF(2^32) batching:  {:?}", time_32);
    println!("  Result: {:?}", acc_32);
    
    // Benchmark GF(2^128) batching
    let challenge_128 = BinaryElem128::from(0xDEADBEEFCAFEBABEu128);
    let start = Instant::now();
    
    let mut acc_128 = BinaryElem128::zero();
    let mut power_128 = BinaryElem128::one();
    for &c in &constraints {
        let c_ext = BinaryElem128::from(BinaryElem32::from(c));
        let term = c_ext.mul(&power_128);
        acc_128 = acc_128.add(&term);
        power_128 = power_128.mul(&challenge_128);
    }
    
    let time_128 = start.elapsed();
    println!("GF(2^128) batching: {:?}", time_128);
    println!("  Result: {:?}", acc_128);
    
    println!();
    let slowdown = time_128.as_nanos() as f64 / time_32.as_nanos() as f64;
    println!("Slowdown factor: {:.2}x", slowdown);
    println!();
    println!("Throughput:");
    println!("  GF(2^32):  {:.2} M constraints/sec", num_constraints as f64 / time_32.as_secs_f64() / 1_000_000.0);
    println!("  GF(2^128): {:.2} M constraints/sec", num_constraints as f64 / time_128.as_secs_f64() / 1_000_000.0);
}
