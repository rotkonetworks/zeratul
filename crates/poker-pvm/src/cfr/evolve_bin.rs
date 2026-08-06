//! Evolutionary CTM training binary.
//!
//! Usage: cfr-evolve <strategy.bin> [generations] [population] [hands_per_eval]
//!
//! Evolves CTM weights to maximize bb/100 against the blueprint strategy.
//! Saves best weights to ctm_evolved.bin after each generation.

use poker_pvm::cfr::brain::Brain;
use poker_pvm::cfr::ctm_native::{NativeCTMExpert, CTMConfig};
use poker_pvm::cfr::evolve::*;
use poker_pvm::cfr::strategy::import_strategy;
use poker_pvm::cfr::tournament::run_tournament_seeded;
use std::collections::HashMap;
use std::sync::Arc;

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let strategy_path = args.get(1).map(|s| s.as_str()).unwrap_or("strategy.bin");
    let generations: usize = args.get(2).and_then(|s| s.parse().ok()).unwrap_or(100);
    let pop_size: usize = args.get(3).and_then(|s| s.parse().ok()).unwrap_or(50);
    let hands: u32 = args.get(4).and_then(|s| s.parse().ok()).unwrap_or(1000);

    let strategy_bytes = std::fs::read(strategy_path)
        .unwrap_or_else(|e| { eprintln!("failed to load {}: {}", strategy_path, e); std::process::exit(1); });

    println!("=== CTM Evolution ===");
    println!("strategy: {} ({} bytes)", strategy_path, strategy_bytes.len());

    // Deserialize strategy ONCE
    let blueprint = import_strategy(&strategy_bytes);
    println!("info sets: {}", blueprint.len());
    drop(strategy_bytes); // free raw bytes

    let blueprint_arc = Arc::new(blueprint);

    println!("generations: {}", generations);
    println!("population: {}", pop_size);
    println!("hands per eval: {}", hands);

    // Create template CTM
    let template = NativeCTMExpert::new(33, 128, 64);
    let n_params = param_count(&template);
    println!("CTM params: {}", n_params);

    // Initialize population with random weights
    let mut rng_state: u64 = 0xDEAD_BEEF_CAFE_1234;
    let mut population: Vec<Vec<f32>> = (0..pop_size).map(|_| {
        (0..n_params).map(|_| {
            rng_state ^= rng_state << 13;
            rng_state ^= rng_state >> 7;
            rng_state ^= rng_state << 17;
            (((rng_state >> 16) as u32) as f32 / u32::MAX as f32 - 0.5) * 0.2
        }).collect()
    }).collect();

    let config = EvolutionConfig {
        population_size: pop_size,
        elite_count: pop_size / 5,
        mutation_sigma: 0.05,
        hands_per_eval: hands,
        generations,
    };

    println!("\nevolving...\n");

    for gen in 0..generations {
        // Evaluate ALL individuals in PARALLEL
        // Each thread gets Arc<HashMap> — no re-deserialization
        let fitnesses: Vec<f64> = {
            let handles: Vec<_> = population.iter().enumerate().map(|(idx, weights)| {
                let w = weights.clone();
                let bp = blueprint_arc.clone();
                let h = hands;
                let seed_base = (gen as u64) * 1000 + idx as u64;
                std::thread::spawn(move || {
                    let mut ctm_a = NativeCTMExpert::new(33, 128, 64);
                    unflatten_weights(&mut ctm_a, &w);
                    ctm_a.config.hebbian_enabled = true;
                    ctm_a.config.hebbian_lr = 0.001;
                    ctm_a.config.blend_weight = 0.05;

                    let seed = seed_base.wrapping_mul(0x9E3779B97F4A7C15);

                    // Seat 0: CTM vs Blueprint
                    let mut brain_ctm = Brain::from_shared_blueprint(&bp);
                    brain_ctm.native_ctm = Some(std::cell::RefCell::new(ctm_a));
                    let mut brain_bp = Brain::from_shared_blueprint(&bp);
                    let r1 = run_tournament_seeded(&mut brain_ctm, &mut brain_bp, h, 10, 1000, seed);

                    // Seat 1: Blueprint vs CTM (same seed)
                    let mut ctm_b = NativeCTMExpert::new(33, 128, 64);
                    unflatten_weights(&mut ctm_b, &w);
                    ctm_b.config.hebbian_enabled = true;
                    ctm_b.config.hebbian_lr = 0.001;
                    ctm_b.config.blend_weight = 0.05;
                    let mut brain_bp2 = Brain::from_shared_blueprint(&bp);
                    let mut brain_ctm2 = Brain::from_shared_blueprint(&bp);
                    brain_ctm2.native_ctm = Some(std::cell::RefCell::new(ctm_b));
                    let r2 = run_tournament_seeded(&mut brain_bp2, &mut brain_ctm2, h, 10, 1000, seed);

                    (r1.p0_bb_per_100 - r2.p0_bb_per_100) / 2.0
                })
            }).collect();

            handles.into_iter().map(|h| h.join().unwrap_or(0.0)).collect()
        };

        let (best_fitness, best_weights) = evolve_one_generation(
            &mut population, &fitnesses, &config, &mut rng_state,
        );

        let avg_fitness: f64 = fitnesses.iter().sum::<f64>() / fitnesses.len() as f64;

        if gen % 5 == 0 || gen == generations - 1 {
            println!("gen {}: best={:+.1} bb/100  avg={:+.1} bb/100  pop={}",
                gen, best_fitness, avg_fitness, population.len());
        }

        if gen % 10 == 0 || gen == generations - 1 {
            let out_path = format!("ctm_evolved_gen{}.bin", gen);
            if let Ok(bytes) = bincode_weights(&best_weights) {
                let _ = std::fs::write(&out_path, &bytes);
                println!("  saved {} ({} bytes)", out_path, bytes.len());
            }
        }
    }
}

fn bincode_weights(weights: &[f32]) -> Result<Vec<u8>, ()> {
    let mut bytes = Vec::with_capacity(weights.len() * 4 + 4);
    bytes.extend_from_slice(&(weights.len() as u32).to_le_bytes());
    for w in weights {
        bytes.extend_from_slice(&w.to_le_bytes());
    }
    Ok(bytes)
}
