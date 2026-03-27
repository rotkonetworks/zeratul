//! cfr-selfplay: generate training data via self-play + measure exploitability
//!
//! usage:
//!   cfr-selfplay --strategy strategy.bin --hands 500000 --output data.bin
//!   cfr-selfplay --strategy strategy.bin --hands 50000 --measure-only
//!   cfr-selfplay --strategy strategy.bin --with-search --checkpoint-dir /data/selfplay --checkpoint-every 100000

use poker_pvm::cfr::selfplay::{Arena, export_samples};
use std::time::Instant;

fn main() {
    let args: Vec<String> = std::env::args().collect();

    let mut strategy_path = "strategy.bin".to_string();
    let mut hands: u64 = 100_000;
    let mut output_path: Option<String> = None;
    let mut measure_only = false;
    let mut threads: usize = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(4);
    let mut fast = true;
    let mut checkpoint_dir: Option<String> = None;
    let mut checkpoint_every: u64 = 100_000;
    let mut moe_dir: Option<String> = None;
    let mut moe_weight: f32 = 0.3;
    let mut moe_a: Option<String> = None;
    let mut moe_b: Option<String> = None;

    let mut i = 1;
    while i < args.len() {
        match args[i].as_str() {
            "--strategy" => { if i+1 < args.len() { strategy_path = args[i+1].clone(); i += 1; } }
            "--hands" => { if i+1 < args.len() { hands = args[i+1].parse().unwrap_or(100_000); i += 1; } }
            "--output" => { if i+1 < args.len() { output_path = Some(args[i+1].clone()); i += 1; } }
            "--threads" => { if i+1 < args.len() { threads = args[i+1].parse().unwrap_or(4); i += 1; } }
            "--with-search" => { fast = false; }
            "--measure-only" => { measure_only = true; }
            "--checkpoint-dir" => { if i+1 < args.len() { checkpoint_dir = Some(args[i+1].clone()); i += 1; } }
            "--checkpoint-every" => { if i+1 < args.len() { checkpoint_every = args[i+1].parse().unwrap_or(100_000); i += 1; } }
            "--moe-dir" => { if i+1 < args.len() { moe_dir = Some(args[i+1].clone()); i += 1; } }
            "--moe-weight" => { if i+1 < args.len() { moe_weight = args[i+1].parse().unwrap_or(0.3); i += 1; } }
            "--moe-a" => { if i+1 < args.len() { moe_a = Some(args[i+1].clone()); i += 1; } }
            "--moe-b" => { if i+1 < args.len() { moe_b = Some(args[i+1].clone()); i += 1; } }
            _ => {}
        }
        i += 1;
    }

    let strategy = std::fs::read(&strategy_path)
        .unwrap_or_else(|e| { eprintln!("failed to load {}: {}", strategy_path, e); std::process::exit(1); });

    println!("strategy: {} ({} bytes)", strategy_path, strategy.len());
    println!("threads: {}  mode: {}", threads, if fast { "fast (L0 only)" } else { "full (L0-L3 + search)" });

    // A/B head-to-head mode
    #[cfg(feature = "onnx")]
    if let (Some(ref a), Some(ref b)) = (&moe_a, &moe_b) {
        let full = !fast; // --with-search enables full L0-L4 stack
        println!("A/B match: {} vs {}", a, b);
        println!("hands: {}  weight: {}  stack: {}", hands, moe_weight, if full { "L0-L4 (full)" } else { "L0+L4 (fast)" });
        let arena = Arena::new(&strategy);
        let result = if full {
            arena.play_ab_full(hands, a, b, moe_weight)
        } else {
            arena.play_ab(hands, a, b, moe_weight)
        };
        let bb_diff = result.player_0_winnings as f64 / hands as f64 / 10.0;
        println!("A winnings: {:+}  B winnings: {:+}", result.player_0_winnings, result.player_1_winnings);
        println!("A edge: {:+.3} bb/hand", bb_diff);
        if bb_diff > 0.0 { println!(">>> A wins") }
        else if bb_diff < 0.0 { println!(">>> B wins") }
        else { println!(">>> draw") }
        return;
    }

    let mut arena = Arena::new(&strategy);
    #[cfg(feature = "onnx")]
    if let Some(ref dir) = moe_dir {
        println!("MoE: {} (weight: {})", dir, moe_weight);
        arena = arena.with_moe(dir, moe_weight);
    }

    if measure_only {
        let t0 = Instant::now();
        let result = if threads > 1 {
            arena.play_parallel(hands, threads, fast)
        } else if fast {
            arena.play_fast(hands)
        } else {
            arena.play(hands)
        };
        let elapsed = t0.elapsed();
        let rate = hands as f64 / elapsed.as_secs_f64();
        println!("hands: {}  samples: {}  time: {:.1}s  rate: {:.0}/sec",
            hands, result.samples.len(), elapsed.as_secs_f64(), rate);
        println!("p0 winnings: {:+}  p1 winnings: {:+}", result.player_0_winnings, result.player_1_winnings);
        let bb_per_hand = (result.player_0_winnings.abs().max(result.player_1_winnings.abs()) as f64)
            / hands as f64 / 10.0;
        println!("exploitability estimate: {:.2} bb/hand", bb_per_hand);
        return;
    }

    // checkpoint mode: run in batches, save periodically
    if let Some(ref dir) = checkpoint_dir {
        std::fs::create_dir_all(dir).unwrap();
        let t0 = Instant::now();
        let mut total_hands: u64 = 0;
        let mut total_samples: u64 = 0;
        let mut checkpoint_num: u32 = 0;

        // find existing checkpoints to resume
        if let Ok(entries) = std::fs::read_dir(dir) {
            for entry in entries.flatten() {
                let name = entry.file_name().to_string_lossy().to_string();
                if name.starts_with("checkpoint_") && name.ends_with(".bin") {
                    if let Some(num_str) = name.strip_prefix("checkpoint_").and_then(|s| s.strip_suffix(".bin")) {
                        if let Ok(num) = num_str.parse::<u32>() {
                            checkpoint_num = checkpoint_num.max(num + 1);
                        }
                    }
                }
            }
        }
        if checkpoint_num > 0 {
            println!("resuming from checkpoint {}", checkpoint_num);
        }

        let target = if hands == 0 { u64::MAX } else { hands }; // 0 = infinite
        println!("checkpoint mode: save every {} hands to {}/", checkpoint_every, dir);

        while total_hands < target {
            let batch = checkpoint_every.min(target - total_hands);
            let batch_t = Instant::now();

            let result = if threads > 1 {
                arena.play_parallel(batch, threads, fast)
            } else if fast {
                arena.play_fast(batch)
            } else {
                arena.play(batch)
            };

            total_hands += batch;
            total_samples += result.samples.len() as u64;

            let elapsed = t0.elapsed();
            let batch_elapsed = batch_t.elapsed();
            let rate = batch as f64 / batch_elapsed.as_secs_f64();

            let path = format!("{}/checkpoint_{:04}.bin", dir, checkpoint_num);
            let data = export_samples(&result.samples);
            std::fs::write(&path, &data).unwrap();

            let bb_per_hand = (result.player_0_winnings.abs().max(result.player_1_winnings.abs()) as f64)
                / batch as f64 / 10.0;

            println!("[checkpoint {:04}] hands: {}  samples: {}  rate: {:.0}/sec  exploit: {:.3} bb/h  total: {} hands {} samples  elapsed: {:.0}s",
                checkpoint_num,
                batch, result.samples.len(), rate,
                bb_per_hand,
                total_hands, total_samples,
                elapsed.as_secs_f64());

            checkpoint_num += 1;
        }

        println!("done: {} total hands, {} total samples in {:.1}s",
            total_hands, total_samples, t0.elapsed().as_secs_f64());
        return;
    }

    // single-shot mode
    let t0 = Instant::now();
    let result = if threads > 1 {
        arena.play_parallel(hands, threads, fast)
    } else if fast {
        arena.play_fast(hands)
    } else {
        arena.play(hands)
    };
    let elapsed = t0.elapsed();

    let rate = hands as f64 / elapsed.as_secs_f64();
    println!("hands: {}  samples: {}  time: {:.1}s  rate: {:.0}/sec",
        hands, result.samples.len(), elapsed.as_secs_f64(), rate);
    println!("p0 winnings: {:+}  p1 winnings: {:+}", result.player_0_winnings, result.player_1_winnings);

    let bb_per_hand = (result.player_0_winnings.abs().max(result.player_1_winnings.abs()) as f64)
        / hands as f64 / 10.0;
    println!("exploitability estimate: {:.2} bb/hand", bb_per_hand);

    if let Some(path) = output_path {
        let data = export_samples(&result.samples);
        std::fs::write(&path, &data).unwrap();
        println!("exported {} samples ({} KB) to {}", result.samples.len(), data.len() / 1024, path);
    }
}
