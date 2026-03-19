//! standalone CFR training binary.
//! compile with: cargo build --release --features gpu --bin cfr-train
//! link with: -L src/cfr -lgpu_eval

use poker_pvm::cfr::solver::Solver;
use poker_pvm::cfr::strategy::export_strategy;
use poker_pvm::Rules;
use std::time::Instant;

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let iterations: u64 = args.get(1).and_then(|s| s.parse().ok()).unwrap_or(100_000);
    let output = args.get(2).map(|s| s.as_str()).unwrap_or("strategy.bin");

    println!("=== MCCFR Solver ===");
    println!("iterations: {}", iterations);
    #[cfg(feature = "gpu")]
    println!("GPU: enabled (ROCm HIP)");
    #[cfg(not(feature = "gpu"))]
    println!("GPU: disabled (CPU fallback)");

    let rules = Rules {
        buyin: 1000,
        small_blind: 5,
        big_blind: 10,
        turn_timeout_blocks: 6,
        rake_bps: 0,
        rake_cap: 0,
    };

    let mut solver = Solver::new(rules);

    let start = Instant::now();
    solver.train(iterations);
    let elapsed = start.elapsed();

    println!("\n=== Results ===");
    println!("time: {:.1}s", elapsed.as_secs_f64());
    println!("info sets: {}", solver.nodes.len());
    println!("exploitability: {:.2}", solver.exploitability_estimate());
    println!("speed: {:.0} iterations/sec", iterations as f64 / elapsed.as_secs_f64());

    // export strategy
    let data = export_strategy(&solver.nodes);
    std::fs::write(output, &data).unwrap();
    println!("strategy saved to {} ({} bytes, {} KB)", output, data.len(), data.len() / 1024);
}
