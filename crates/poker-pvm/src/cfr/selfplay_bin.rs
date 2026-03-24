//! cfr-selfplay: generate training data via self-play + measure exploitability
//!
//! usage:
//!   cfr-selfplay --strategy strategy.bin --hands 500000 --output data.bin
//!   cfr-selfplay --strategy strategy.bin --hands 50000 --measure-only

use poker_pvm::cfr::selfplay::{Arena, export_samples};
use std::time::Instant;

fn main() {
    let args: Vec<String> = std::env::args().collect();

    let mut strategy_path = "strategy.bin".to_string();
    let mut hands: u64 = 100_000;
    let mut output_path: Option<String> = None;
    let mut measure_only = false;

    let mut i = 1;
    while i < args.len() {
        match args[i].as_str() {
            "--strategy" => { if i+1 < args.len() { strategy_path = args[i+1].clone(); i += 1; } }
            "--hands" => { if i+1 < args.len() { hands = args[i+1].parse().unwrap_or(100_000); i += 1; } }
            "--output" => { if i+1 < args.len() { output_path = Some(args[i+1].clone()); i += 1; } }
            "--measure-only" => { measure_only = true; }
            _ => {}
        }
        i += 1;
    }

    let strategy = std::fs::read(&strategy_path)
        .unwrap_or_else(|e| { eprintln!("failed to load {}: {}", strategy_path, e); std::process::exit(1); });

    println!("strategy: {} ({} bytes)", strategy_path, strategy.len());

    let arena = Arena::new(&strategy);

    let t0 = Instant::now();
    let result = arena.play(hands);
    let elapsed = t0.elapsed();

    let rate = hands as f64 / elapsed.as_secs_f64();
    println!("hands: {}  samples: {}  time: {:.1}s  rate: {:.0}/sec",
        hands, result.samples.len(), elapsed.as_secs_f64(), rate);
    println!("p0 winnings: {:+}  p1 winnings: {:+}", result.player_0_winnings, result.player_1_winnings);

    // exploitability estimate: how much the weaker player loses per hand
    let bb_per_hand = (result.player_0_winnings.abs().max(result.player_1_winnings.abs()) as f64)
        / hands as f64 / 10.0; // normalize to bb/hand (bb=10)
    println!("exploitability estimate: {:.2} bb/hand", bb_per_hand);

    if measure_only {
        return;
    }

    if let Some(path) = output_path {
        let data = export_samples(&result.samples);
        std::fs::write(&path, &data).unwrap();
        println!("exported {} samples ({} KB) to {}", result.samples.len(), data.len() / 1024, path);
    }
}
