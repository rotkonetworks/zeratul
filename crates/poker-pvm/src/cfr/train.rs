//! standalone CFR training binary.
//!
//! Usage:
//!   cfr-train <iterations> <output.bin> [flags]
//!
//! Flags:
//!   --variant vanilla|dcfr    CFR variant (default: dcfr)
//!   --resume checkpoint.ckpt  resume from checkpoint
//!   --players N               multiplayer mode (default: 2)
//!   --threads N               parallel mode (default: 1, use 0 for all cores)
//!
//! Examples:
//!   cfr-train 100000000 strategy_100m.bin --threads 0     # all cores
//!   cfr-train 5000000 strategy_5m_6max.bin --players 6    # 6-max
//!   cfr-train 10000000 strat.bin --threads 96             # EPYC 9654

use poker_pvm::cfr::solver::{Solver, CfrVariant};
use poker_pvm::cfr::multi_solver::MultiSolver;
use poker_pvm::cfr::strategy::export_strategy;
use poker_pvm::Rules;
use std::time::Instant;

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let iterations: u64 = args.get(1).and_then(|s| s.parse().ok()).unwrap_or(100_000);
    let output = args.get(2).map(|s| s.as_str()).unwrap_or("strategy.bin");

    // parse flags
    let mut variant_name = "dcfr";
    let mut resume_path: Option<String> = None;
    let mut num_players: u8 = 2;
    let mut num_threads: usize = 1;
    let mut i = 3;
    while i < args.len() {
        match args[i].as_str() {
            "--variant" => { if i + 1 < args.len() { variant_name = if args[i+1] == "vanilla" { "vanilla" } else { "dcfr" }; i += 1; } }
            "--resume" => { if i + 1 < args.len() { resume_path = Some(args[i + 1].clone()); i += 1; } }
            "--players" => { if i + 1 < args.len() { num_players = args[i+1].parse().unwrap_or(2); i += 1; } }
            "--threads" => {
                if i + 1 < args.len() {
                    let t: usize = args[i+1].parse().unwrap_or(1);
                    num_threads = if t == 0 { num_cpus() } else { t };
                    i += 1;
                }
            }
            _ => {}
        }
        i += 1;
    }

    let rules = Rules {
        buyin: 1000, small_blind: 5, big_blind: 10,
        turn_timeout_blocks: 6, rake_bps: 0, rake_cap: 0,
    };

    println!("=== MCCFR Solver ===");
    println!("target iterations: {}", iterations);
    println!("players: {}", num_players);
    println!("threads: {}", num_threads);
    println!("variant: {}", variant_name);
    #[cfg(feature = "gpu")]
    println!("GPU: enabled (ROCm HIP)");
    #[cfg(not(feature = "gpu"))]
    println!("GPU: disabled (CPU fallback)");

    // parallel heads-up mode
    #[cfg(feature = "parallel")]
    if num_threads > 1 && num_players == 2 {
        use poker_pvm::cfr::parallel::par::ParallelSolver;

        println!("\n[parallel mode: {} threads]", num_threads);
        let solver = ParallelSolver::new(rules, num_threads);

        let start = Instant::now();
        solver.train_with_saves(iterations, Some(output), 10_000_000);
        let elapsed = start.elapsed();

        let total = solver.iterations.load(std::sync::atomic::Ordering::Relaxed);
        println!("\n=== Results (parallel) ===");
        println!("total iterations: {}", total);
        println!("time: {:.1}s", elapsed.as_secs_f64());
        println!("info sets: {}", solver.nodes.len());
        println!("speed: {:.0} iterations/sec", total as f64 / elapsed.as_secs_f64());
        println!("throughput: {:.0} iterations/sec/core", total as f64 / elapsed.as_secs_f64() / num_threads as f64);

        let data = solver.export_strategy();
        std::fs::write(output, &data).unwrap();
        println!("strategy saved to {} ({} bytes, {} KB)", output, data.len(), data.len() / 1024);
        return;
    }

    #[cfg(not(feature = "parallel"))]
    if num_threads > 1 {
        eprintln!("parallel mode requires --features parallel. rebuild with:");
        eprintln!("  cargo build --release --features parallel --bin cfr-train");
        std::process::exit(1);
    }

    if num_players > 2 {
        // multiplayer mode
        let mut solver = MultiSolver::new(rules, num_players);

        let start = Instant::now();
        solver.train(iterations);
        let elapsed = start.elapsed();

        println!("\n=== Results ({}-player) ===", num_players);
        println!("iterations: {}", solver.iterations);
        println!("time: {:.1}s", elapsed.as_secs_f64());
        println!("info sets: {}", solver.nodes.len());
        println!("speed: {:.0} iterations/sec", iterations as f64 / elapsed.as_secs_f64());

        let data = solver.export_strategy();
        std::fs::write(output, &data).unwrap();
        println!("strategy saved to {} ({} bytes, {} KB)", output, data.len(), data.len() / 1024);
    } else {
        // heads-up single-threaded (with checkpoint support)
        let checkpoint_path = format!("{}.ckpt", output.trim_end_matches(".bin"));

        let mut solver = if let Some(ref ckpt) = resume_path {
            println!("resuming from checkpoint: {}", ckpt);
            match Solver::load_checkpoint(ckpt) {
                Ok(s) => {
                    println!("resumed at iteration {} with {} info sets", s.iterations, s.nodes.len());
                    s
                }
                Err(e) => {
                    eprintln!("failed to load checkpoint: {}", e);
                    std::process::exit(1);
                }
            }
        } else {
            let variant = match variant_name {
                "vanilla" => CfrVariant::Vanilla,
                _ => CfrVariant::DcfrPlus { alpha: 2.0, gamma: 2.0 },
            };
            Solver::with_variant(rules, variant)
        };

        let remaining = iterations.saturating_sub(solver.iterations);
        if remaining == 0 {
            println!("already at {} iterations, nothing to do", solver.iterations);
        } else {
            println!("running {} more iterations (from {} to {})", remaining, solver.iterations, iterations);

            let start = Instant::now();
            solver.train_with_checkpoint(remaining, Some(&checkpoint_path));
            let elapsed = start.elapsed();

            println!("\n=== Results ===");
            println!("total iterations: {}", solver.iterations);
            println!("time: {:.1}s", elapsed.as_secs_f64());
            println!("info sets: {}", solver.nodes.len());
            println!("exploitability: {:.2}", solver.exploitability_estimate());
            println!("speed: {:.0} iterations/sec", remaining as f64 / elapsed.as_secs_f64());
        }

        let data = export_strategy(&solver.nodes);
        std::fs::write(output, &data).unwrap();
        println!("strategy saved to {} ({} bytes, {} KB)", output, data.len(), data.len() / 1024);

        if let Err(e) = solver.save_checkpoint(&checkpoint_path) {
            eprintln!("final checkpoint save failed: {}", e);
        } else {
            println!("checkpoint saved to {}", checkpoint_path);
        }
    }
}

fn num_cpus() -> usize {
    std::thread::available_parallelism().map(|n| n.get()).unwrap_or(4)
}
