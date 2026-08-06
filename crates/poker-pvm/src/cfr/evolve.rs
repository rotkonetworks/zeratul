//! Evolutionary strategy for CTM weight optimization (Ha/hardmaru style).
//!
//! Instead of gradients: evolve weights using fitness = bb/100 vs blueprint.
//! Population of N weight vectors, evaluate each over K hands, select best, mutate.
//! Embarrassingly parallel on 192 cores.
//!
//! Also adds opponent prediction head (world model) so CTM can simulate
//! opponent actions in its thinking steps.

use super::ctm_native::{NativeCTMExpert, CTMConfig};
use super::brain::Brain;

/// One individual in the population
struct Individual {
    /// Flattened weight vector
    weights: Vec<f32>,
    /// Fitness: bb/100 against reference opponent
    fitness: f64,
}

/// Evolutionary parameters
pub struct EvolutionConfig {
    pub population_size: usize,    // default 50
    pub elite_count: usize,        // default 10 (top survivors)
    pub mutation_sigma: f32,       // default 0.02 (noise scale)
    pub hands_per_eval: u32,       // default 1000
    pub generations: usize,        // default 100
}

impl Default for EvolutionConfig {
    fn default() -> Self {
        Self {
            population_size: 50,
            elite_count: 10,
            mutation_sigma: 0.02,
            hands_per_eval: 1000,
            generations: 100,
        }
    }
}

/// Extract all weights from a CTM expert into a flat vector
pub fn flatten_weights(ctm: &NativeCTMExpert) -> Vec<f32> {
    let mut weights = Vec::new();
    weights.extend_from_slice(&ctm.step_net_0.weight);
    weights.extend_from_slice(&ctm.step_net_0.bias);
    weights.extend_from_slice(&ctm.step_net_2.weight);
    weights.extend_from_slice(&ctm.step_net_2.bias);
    weights.extend_from_slice(&ctm.halt_net.weight);
    weights.extend_from_slice(&ctm.halt_net.bias);
    weights.extend_from_slice(&ctm.value_head.weight);
    weights.extend_from_slice(&ctm.value_head.bias);
    weights.extend_from_slice(&ctm.policy_head.weight);
    weights.extend_from_slice(&ctm.policy_head.bias);
    weights.extend_from_slice(&ctm.init_hidden);
    weights.extend_from_slice(&ctm.decay);
    weights
}

/// Load flat weight vector back into a CTM expert
pub fn unflatten_weights(ctm: &mut NativeCTMExpert, weights: &[f32]) {
    let mut pos = 0;
    let mut copy = |dst: &mut [f32], src: &[f32], p: &mut usize| {
        let n = dst.len();
        dst.copy_from_slice(&src[*p..*p + n]);
        *p += n;
    };
    copy(&mut ctm.step_net_0.weight, weights, &mut pos);
    copy(&mut ctm.step_net_0.bias, weights, &mut pos);
    copy(&mut ctm.step_net_2.weight, weights, &mut pos);
    copy(&mut ctm.step_net_2.bias, weights, &mut pos);
    copy(&mut ctm.halt_net.weight, weights, &mut pos);
    copy(&mut ctm.halt_net.bias, weights, &mut pos);
    copy(&mut ctm.value_head.weight, weights, &mut pos);
    copy(&mut ctm.value_head.bias, weights, &mut pos);
    copy(&mut ctm.policy_head.weight, weights, &mut pos);
    copy(&mut ctm.policy_head.bias, weights, &mut pos);
    copy(&mut ctm.init_hidden, weights, &mut pos);
    copy(&mut ctm.decay, weights, &mut pos);
}

/// Count total parameters
pub fn param_count(ctm: &NativeCTMExpert) -> usize {
    flatten_weights(ctm).len()
}

/// Run one generation of evolution
/// Returns (best_fitness, best_weights)
pub fn evolve_one_generation(
    population: &mut Vec<Vec<f32>>,
    fitnesses: &[f64],
    config: &EvolutionConfig,
    rng_state: &mut u64,
) -> (f64, Vec<f32>) {
    // Sort by fitness (descending)
    let mut indexed: Vec<(usize, f64)> = fitnesses.iter().copied().enumerate().collect();
    indexed.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap());

    let best_fitness = indexed[0].1;
    let best_weights = population[indexed[0].0].clone();

    // Select elite (at least 1)
    let elite_count = config.elite_count.max(1);
    let elite: Vec<Vec<f32>> = indexed[..elite_count]
        .iter()
        .map(|(i, _)| population[*i].clone())
        .collect();

    // Generate new population from elite + mutation
    let mut new_pop = Vec::with_capacity(config.population_size);

    // Keep elite unchanged
    for e in &elite {
        new_pop.push(e.clone());
    }

    // Fill rest with mutated elite
    while new_pop.len() < config.population_size {
        let parent = &elite[new_pop.len() % elite.len()];
        let mut child = parent.clone();
        for w in child.iter_mut() {
            // Gaussian noise via Box-Muller
            let u1 = xorshift_f32(rng_state);
            let u2 = xorshift_f32(rng_state);
            let noise = (-2.0 * u1.ln()).sqrt() * (2.0 * std::f32::consts::PI * u2).cos();
            *w += config.mutation_sigma * noise;
        }
        new_pop.push(child);
    }

    *population = new_pop;
    (best_fitness, best_weights)
}

fn xorshift_f32(state: &mut u64) -> f32 {
    *state ^= *state << 13;
    *state ^= *state >> 7;
    *state ^= *state << 17;
    ((*state >> 16) as u32) as f32 / u32::MAX as f32
}
