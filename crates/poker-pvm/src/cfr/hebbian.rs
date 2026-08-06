//! Hebbian plasticity strategies — composable, swappable at runtime.
//!
//! Three strategies from CTM research (continuous-thought-machines/utils/bounds/training.py):
//!   1. CoinBetting — parameter-free via KT estimator (Orabona 2016)
//!   2. BoundGuided — SDP diagnostics steer which dimensions to update
//!   3. Homeostatic — HAG-inspired structural plasticity (Cazalets 2026)
//!
//! All operate on the policy_head weights of a CTM expert.
//! Eriksen filter pattern: each is independent, composable, toggleable.

use super::ctm::NUM_ACTIONS;

/// Hebbian update signal — richer than binary win/loss
#[derive(Clone, Debug)]
pub struct HebbianSignal {
    /// Per-action regret from search (best signal — what CFR computes)
    pub regrets: Option<[f32; NUM_ACTIONS]>,
    /// Was the hand profitable? +1 win, -1 loss, 0 neutral
    pub outcome: f32,
    /// Model certainty — max of policy softmax (0.0-1.0)
    pub certainty: f32,
    /// Which action was taken (index into NUM_ACTIONS)
    pub action_taken: usize,
    /// Hidden state at decision time
    pub hidden: Vec<f32>,
    /// Policy output at decision time
    pub policy: [f32; NUM_ACTIONS],
}

/// Common trait for all Hebbian strategies
pub trait HebbianStrategy: Send {
    fn name(&self) -> &'static str;
    /// Update internal state based on signal
    fn update(&mut self, signal: &HebbianSignal, policy_weights: &mut [f32], hidden_dim: usize);
    /// Reset (new opponent)
    fn reset(&mut self);
}

// ─── CoinBettingHebbian ─────────────────────────────────────────
/// Parameter-free adaptation via wealth-based betting.
/// If updates help → bet more. If they hurt → throttle.
/// No learning rate to tune. O(sqrt(T)) regret.
pub struct CoinBettingHebbian {
    wealth: f32,
    initial_wealth: f32,
    sum_rewards: f32,
    sum_sq_rewards: f32,
    t: u32,
    /// Direction matrix [NUM_ACTIONS × hidden_dim] — accumulated update direction
    direction: Vec<f32>,
    hidden_dim: usize,
}

impl CoinBettingHebbian {
    pub fn new(hidden_dim: usize) -> Self {
        Self {
            wealth: 1.0,
            initial_wealth: 1.0,
            sum_rewards: 0.0,
            sum_sq_rewards: 0.0,
            t: 0,
            direction: vec![0.0; NUM_ACTIONS * hidden_dim],
            hidden_dim,
        }
    }
}

impl HebbianStrategy for CoinBettingHebbian {
    fn name(&self) -> &'static str { "coin-betting" }

    fn update(&mut self, signal: &HebbianSignal, policy_weights: &mut [f32], hidden_dim: usize) {
        self.t += 1;

        // Reward: certainty-weighted outcome
        let r = signal.outcome * signal.certainty;

        self.sum_rewards += r;
        self.sum_sq_rewards += r * r;

        // KT bet fraction: sum_rewards / (t + 1), clamped
        let bet_fraction = (self.sum_rewards / (self.t as f32 + 1.0)).clamp(-0.5, 0.5);

        // Update wealth
        self.wealth *= 1.0 + r * bet_fraction;
        self.wealth = self.wealth.max(1e-8);

        // Hebbian direction: outer(hidden, policy)
        let mut new_dir = vec![0.0f32; NUM_ACTIONS * hidden_dim];
        let dir_scale = if signal.outcome > 0.0 { 1.0 } else { -0.5 }; // reinforce wins more
        for a in 0..NUM_ACTIONS {
            for h in 0..hidden_dim.min(signal.hidden.len()) {
                new_dir[a * hidden_dim + h] = dir_scale * signal.hidden[h] * signal.policy[a];
            }
        }

        // Normalize direction
        let norm: f32 = new_dir.iter().map(|x| x * x).sum::<f32>().sqrt();
        if norm > 1e-8 {
            for x in new_dir.iter_mut() { *x /= norm; }
        }

        // Adaptive momentum from reward variance
        let adaptive_momentum = if self.t > 1 {
            let variance = self.sum_sq_rewards / self.t as f32
                - (self.sum_rewards / self.t as f32).powi(2);
            (1.0 - 1.0 / (1.0 + 10.0 * variance.max(0.0))).clamp(0.5, 0.99)
        } else {
            0.9
        };

        // Blend direction
        for i in 0..self.direction.len().min(new_dir.len()) {
            self.direction[i] = adaptive_momentum * self.direction[i]
                + (1.0 - adaptive_momentum) * new_dir[i];
        }

        // Apply: delta = direction * bet_size
        let bet_size = self.wealth * bet_fraction.abs();
        for i in 0..policy_weights.len().min(self.direction.len()) {
            policy_weights[i] += self.direction[i] * bet_size * 0.01; // scale down for stability
        }
    }

    fn reset(&mut self) {
        self.wealth = self.initial_wealth;
        self.sum_rewards = 0.0;
        self.sum_sq_rewards = 0.0;
        self.t = 0;
        self.direction.fill(0.0);
    }
}

// ─── BoundGuidedHebbian ─────────────────────────────────────────
/// Uses per-tick analysis to steer updates.
/// Only updates dimensions that are underperforming.
pub struct BoundGuidedHebbian {
    /// Per-action cumulative regret (from search)
    cumulative_regret: [f32; NUM_ACTIONS],
    /// Running mean of hidden activations (for novelty)
    hidden_mean: Vec<f32>,
    /// Which dimensions to update (from periodic diagnosis)
    active_dims: Vec<bool>,
    diagnose_every: u32,
    t: u32,
    hidden_dim: usize,
    lr: f32,
    momentum: f32,
    delta: Vec<f32>,
}

impl BoundGuidedHebbian {
    pub fn new(hidden_dim: usize) -> Self {
        Self {
            cumulative_regret: [0.0; NUM_ACTIONS],
            hidden_mean: vec![0.0; hidden_dim],
            active_dims: vec![true; hidden_dim],
            diagnose_every: 50,
            t: 0,
            hidden_dim,
            lr: 0.01,
            momentum: 0.8,
            delta: vec![0.0; NUM_ACTIONS * hidden_dim],
        }
    }
}

impl HebbianStrategy for BoundGuidedHebbian {
    fn name(&self) -> &'static str { "bound-guided" }

    fn update(&mut self, signal: &HebbianSignal, policy_weights: &mut [f32], hidden_dim: usize) {
        self.t += 1;

        // Update hidden mean (for novelty detection)
        let decay = 0.95;
        for i in 0..hidden_dim.min(signal.hidden.len()) {
            self.hidden_mean[i] = decay * self.hidden_mean[i] + (1.0 - decay) * signal.hidden[i];
        }

        // If we have per-action regrets from search, use those (best signal)
        if let Some(regrets) = &signal.regrets {
            for a in 0..NUM_ACTIONS {
                self.cumulative_regret[a] += regrets[a];
            }

            // Update weights using regret as gradient
            for a in 0..NUM_ACTIONS {
                for h in 0..hidden_dim.min(signal.hidden.len()) {
                    if !self.active_dims[h] { continue; }
                    let idx = a * hidden_dim + h;
                    if idx < self.delta.len() {
                        let grad = regrets[a] * signal.hidden[h];
                        self.delta[idx] = self.momentum * self.delta[idx] + (1.0 - self.momentum) * grad;
                    }
                }
            }
        } else {
            // Fallback: use outcome-based update (weaker)
            let novelty: Vec<f32> = signal.hidden.iter().zip(self.hidden_mean.iter())
                .map(|(h, m)| h - m).collect();

            for a in 0..NUM_ACTIONS {
                for h in 0..hidden_dim.min(novelty.len()) {
                    if !self.active_dims[h] { continue; }
                    let idx = a * hidden_dim + h;
                    if idx < self.delta.len() {
                        let grad = signal.outcome * novelty[h] * signal.policy[a];
                        self.delta[idx] = self.momentum * self.delta[idx] + (1.0 - self.momentum) * grad;
                    }
                }
            }
        }

        // Apply delta
        for i in 0..policy_weights.len().min(self.delta.len()) {
            policy_weights[i] += self.lr * self.delta[i];
        }

        // Periodic diagnosis: deactivate dimensions with low variance
        if self.t % self.diagnose_every == 0 {
            let mean_abs: f32 = self.hidden_mean.iter().map(|x| x.abs()).sum::<f32>()
                / self.hidden_mean.len() as f32;
            for i in 0..self.active_dims.len().min(self.hidden_mean.len()) {
                // Only update dimensions that deviate from mean
                self.active_dims[i] = self.hidden_mean[i].abs() > mean_abs * 0.1;
            }
        }
    }

    fn reset(&mut self) {
        self.cumulative_regret = [0.0; NUM_ACTIONS];
        self.hidden_mean.fill(0.0);
        self.active_dims.fill(true);
        self.delta.fill(0.0);
        self.t = 0;
    }
}

// ─── HomeostaticHebbian ─────────────────────────────────────────
/// Only updates dimensions out of homeostasis.
/// Prevents runaway updates that destroy trained weights.
pub struct HomeostaticHebbian {
    /// Target activation rate per dimension
    target_rate: f32,
    /// Homeostatic band width — update only outside [target ± band]
    band_width: f32,
    /// Running mean of activations per dimension
    running_mean: Vec<f32>,
    /// Running variance
    running_var: Vec<f32>,
    /// Connection strength delta
    delta: Vec<f32>,
    decay_factor: f32,
    growth_step: f32,
    t: u32,
    hidden_dim: usize,
}

impl HomeostaticHebbian {
    pub fn new(hidden_dim: usize) -> Self {
        Self {
            target_rate: 0.0,
            band_width: 1.0,
            running_mean: vec![0.0; hidden_dim],
            running_var: vec![1.0; hidden_dim],
            delta: vec![0.0; NUM_ACTIONS * hidden_dim],
            decay_factor: 0.99,
            growth_step: 0.01,
            t: 0,
            hidden_dim,
        }
    }
}

impl HebbianStrategy for HomeostaticHebbian {
    fn name(&self) -> &'static str { "homeostatic" }

    fn update(&mut self, signal: &HebbianSignal, policy_weights: &mut [f32], hidden_dim: usize) {
        self.t += 1;

        // Update running stats
        let alpha = 0.01;
        for i in 0..hidden_dim.min(signal.hidden.len()) {
            let h = signal.hidden[i];
            let old_mean = self.running_mean[i];
            self.running_mean[i] += alpha * (h - old_mean);
            self.running_var[i] += alpha * ((h - old_mean) * (h - self.running_mean[i]) - self.running_var[i]);
        }

        // Determine which dimensions are out of homeostasis
        for i in 0..hidden_dim.min(signal.hidden.len()) {
            let deviation = (self.running_mean[i] - self.target_rate).abs();
            let std = self.running_var[i].max(1e-8).sqrt();
            let normalized_dev = deviation / std;

            if normalized_dev > self.band_width {
                // Out of homeostasis — grow connections
                let direction = if signal.outcome > 0.0 { 1.0 } else { -1.0 };
                for a in 0..NUM_ACTIONS {
                    let idx = a * hidden_dim + i;
                    if idx < self.delta.len() {
                        self.delta[idx] += self.growth_step * direction * signal.hidden[i] * signal.policy[a];
                        // Saturation clamp
                        self.delta[idx] = self.delta[idx].clamp(-5.0, 5.0);
                    }
                }
            } else {
                // In homeostasis — prune (decay toward zero)
                for a in 0..NUM_ACTIONS {
                    let idx = a * hidden_dim + i;
                    if idx < self.delta.len() {
                        self.delta[idx] *= self.decay_factor;
                    }
                }
            }
        }

        // Apply delta
        for i in 0..policy_weights.len().min(self.delta.len()) {
            policy_weights[i] += self.delta[i];
        }
    }

    fn reset(&mut self) {
        self.running_mean.fill(0.0);
        self.running_var.fill(1.0);
        self.delta.fill(0.0);
        self.t = 0;
    }
}

/// Factory — create strategy by name
pub fn create_strategy(name: &str, hidden_dim: usize) -> Box<dyn HebbianStrategy> {
    match name {
        "coin-betting" | "coin" => Box::new(CoinBettingHebbian::new(hidden_dim)),
        "bound-guided" | "bound" => Box::new(BoundGuidedHebbian::new(hidden_dim)),
        "homeostatic" | "homeo" => Box::new(HomeostaticHebbian::new(hidden_dim)),
        _ => Box::new(CoinBettingHebbian::new(hidden_dim)), // default
    }
}
