//! Native Rust CTM expert inference with sync accumulators.
//!
//! Reimplements the Python CTMExpert in pure Rust — no tch-rs, no ONNX.
//! Loads weights from serialized format, runs inference with full
//! access to sync state for Hebbian plasticity.
//!
//! Architecture (matches train_experts.py CTMExpert):
//!   step_net: Linear(input+hidden, hidden) → GELU → Linear(hidden, hidden) → GELU
//!   halt_net: Linear(hidden, 1) → sigmoid
//!   value_head: Linear(hidden+sync, 1) → tanh
//!   policy_head: Linear(hidden+sync, 9) → softmax
//!   sync: alpha = r*alpha + h[left]*h[right], sync = alpha/sqrt(beta)

use super::ctm::NUM_ACTIONS;

/// CTM configuration — all features toggleable (Eriksen filter pattern)
/// Autoresearch loop can sweep these parameters to find optimal config.
#[derive(Clone, Debug)]
pub struct CTMConfig {
    pub k: usize,                    // thinking steps (default 8)
    pub hidden_dim: usize,           // hidden state size (default 128)
    pub n_sync: usize,               // sync accumulator pairs (default 64)
    pub persist_sync: bool,          // cross-hand memory (default true)
    pub hebbian_enabled: bool,       // weight modification at inference (default false)
    pub hebbian_lr: f32,             // learning rate for weight updates (default 0.001)
    pub hebbian_gating: bool,        // median-gated novelty (default true)
    pub hebbian_projection: bool,    // project through existing weights (default true)
    pub hebbian_momentum: f32,       // EMA momentum (default 0.95)
    pub baseline_decay: f32,         // pull toward original weights (default 0.01)
    pub halt_enabled: bool,          // adaptive halting (default true)
    pub blend_weight: f32,           // CTM output blend with blueprint (default 0.3)
}

impl Default for CTMConfig {
    fn default() -> Self {
        Self {
            k: DEFAULT_K,
            hidden_dim: 128,
            n_sync: 64,
            persist_sync: true,
            hebbian_enabled: false,
            hebbian_lr: 0.001,
            hebbian_gating: true,
            hebbian_projection: true,
            hebbian_momentum: 0.95,
            baseline_decay: 0.01,
            halt_enabled: true,
            blend_weight: 0.3,
        }
    }
}

/// Default thinking steps — configurable per instance
pub const DEFAULT_K: usize = 8;

/// A dense linear layer: y = Wx + b
#[derive(Clone)]
pub struct Linear {
    pub weight: Vec<f32>,  // [out_dim × in_dim] row-major
    pub bias: Vec<f32>,    // [out_dim]
    pub in_dim: usize,
    pub out_dim: usize,
}

impl Linear {
    pub fn forward(&self, input: &[f32]) -> Vec<f32> {
        let mut output = self.bias.clone();
        for i in 0..self.out_dim {
            let row_start = i * self.in_dim;
            for j in 0..self.in_dim {
                output[i] += self.weight[row_start + j] * input[j];
            }
        }
        output
    }
}

pub fn gelu(x: f32) -> f32 {
    0.5 * x * (1.0 + ((2.0 / std::f32::consts::PI).sqrt() * (x + 0.044715 * x * x * x)).tanh())
}

pub fn sigmoid(x: f32) -> f32 {
    1.0 / (1.0 + (-x).exp())
}

pub fn softmax(logits: &mut [f32]) {
    let max = logits.iter().cloned().fold(f32::NEG_INFINITY, f32::max);
    let mut sum = 0.0f32;
    for l in logits.iter_mut() {
        *l = (*l - max).exp();
        sum += *l;
    }
    if sum > 1e-10 { for l in logits.iter_mut() { *l /= sum; } }
}

/// CTM Expert with sync accumulators — pure Rust, no dependencies
pub struct NativeCTMExpert {
    pub step_net_0: Linear,  // Linear(input+hidden, hidden)
    pub step_net_2: Linear,  // Linear(hidden, hidden)
    pub halt_net: Linear,     // Linear(hidden, 1)
    pub value_head: Linear,   // Linear(hidden+sync, 1)
    pub policy_head: Linear,  // Linear(hidden+sync, NUM_ACTIONS)
    pub init_hidden: Vec<f32>, // [hidden_dim]
    pub sync_left: Vec<usize>, // indices for sync pairs
    pub sync_right: Vec<usize>,
    pub decay: Vec<f32>,       // [n_sync]
    pub hidden_dim: usize,
    pub n_sync: usize,
    pub input_dim: usize,

    /// Persistent sync state across hands (the Hebbian memory)
    pub persist_alpha: Vec<f32>,
    pub persist_beta: Vec<f32>,
    /// Baseline sync for novelty detection
    pub baseline_sync: Vec<f32>,
    pub n_evals: u32,
    /// All toggleable features
    pub config: CTMConfig,
    /// Last sync state (for computing novelty delta)
    last_sync: Vec<f32>,
    /// Last policy output (for outer product)
    last_policy: [f32; NUM_ACTIONS],
    /// Original weights to decay toward (prevents over-adaptation)
    baseline_policy_weights: Vec<f32>,
    baseline_value_weights: Vec<f32>,
}

/// Output from CTM inference including sync state
pub struct CTMOutput {
    pub value: f32,
    pub policy: [f32; NUM_ACTIONS],
    pub sync_state: Vec<f32>,  // current sync accumulator — used for Hebbian
    pub steps_used: usize,
}

impl NativeCTMExpert {
    /// Create with given dimensions (weights loaded separately)
    pub fn new(input_dim: usize, hidden_dim: usize, n_sync: usize) -> Self {
        Self {
            step_net_0: Linear { weight: vec![0.0; (input_dim + hidden_dim) * hidden_dim], bias: vec![0.0; hidden_dim], in_dim: input_dim + hidden_dim, out_dim: hidden_dim },
            step_net_2: Linear { weight: vec![0.0; hidden_dim * hidden_dim], bias: vec![0.0; hidden_dim], in_dim: hidden_dim, out_dim: hidden_dim },
            halt_net: Linear { weight: vec![0.0; hidden_dim], bias: vec![0.0; 1], in_dim: hidden_dim, out_dim: 1 },
            value_head: Linear { weight: vec![0.0; (hidden_dim + n_sync)], bias: vec![0.0; 1], in_dim: hidden_dim + n_sync, out_dim: 1 },
            policy_head: Linear { weight: vec![0.0; (hidden_dim + n_sync) * NUM_ACTIONS], bias: vec![0.0; NUM_ACTIONS], in_dim: hidden_dim + n_sync, out_dim: NUM_ACTIONS },
            init_hidden: vec![0.0; hidden_dim],
            sync_left: (0..n_sync).collect(),
            sync_right: (0..n_sync).map(|i| (i + 1) % hidden_dim).collect(),
            decay: vec![0.0; n_sync],
            hidden_dim,
            n_sync,
            input_dim,
            persist_alpha: vec![0.0; n_sync],
            persist_beta: vec![1.0; n_sync],
            baseline_sync: vec![0.0; n_sync],
            n_evals: 0,
            config: CTMConfig { hidden_dim, n_sync, ..CTMConfig::default() },
            last_sync: vec![0.0; n_sync],
            last_policy: [0.0; NUM_ACTIONS],
            baseline_policy_weights: Vec::new(),
            baseline_value_weights: Vec::new(),
        }
    }

    /// Enable Hebbian weight modification at inference time
    pub fn enable_hebbian(&mut self, lr: f32) {
        self.config.hebbian_enabled = true;
        self.config.hebbian_lr = lr;
        // Save original weights as baseline to decay toward
        self.baseline_policy_weights = self.policy_head.weight.clone();
        self.baseline_value_weights = self.value_head.weight.clone();
    }

    /// Initialize with small random weights (Xavier-like) for testing
    pub fn random_init(&mut self, seed: u64) {
        let mut rng = seed;
        let mut next = || -> f32 {
            rng ^= rng << 13; rng ^= rng >> 7; rng ^= rng << 17;
            ((rng as u32) as f32 / u32::MAX as f32 - 0.5) * 0.1
        };

        for w in self.step_net_0.weight.iter_mut() { *w = next(); }
        for w in self.step_net_2.weight.iter_mut() { *w = next(); }
        for w in self.halt_net.weight.iter_mut() { *w = next(); }
        for w in self.value_head.weight.iter_mut() { *w = next(); }
        for w in self.policy_head.weight.iter_mut() { *w = next(); }
        for w in self.init_hidden.iter_mut() { *w = next() * 0.01; }
        for w in self.decay.iter_mut() { *w = 1.0 + next(); }

        // Random sync pairs
        for i in 0..self.n_sync {
            self.sync_left[i] = (next().abs() * self.hidden_dim as f32) as usize % self.hidden_dim;
            self.sync_right[i] = (next().abs() * self.hidden_dim as f32) as usize % self.hidden_dim;
        }
    }

    /// Run CTM forward pass with sync accumulators.
    /// Static K=8 thinking steps (K-ramp caused alpha collision in nanochat).
    pub fn forward(&mut self, features: &[f32], persist: bool) -> CTMOutput {
        let mut h = self.init_hidden.clone();

        // Decay rates
        let r: Vec<f32> = self.decay.iter().map(|d| (-d.clamp(0.0, 15.0)).exp()).collect();

        // Initialize or load sync state
        let mut alpha = if persist { self.persist_alpha.clone() } else { vec![0.0; self.n_sync] };
        let mut beta = if persist { self.persist_beta.clone() } else { vec![1.0; self.n_sync] };

        let mut total_value = 0.0f32;
        let mut total_policy = [0.0f32; NUM_ACTIONS];
        let mut remaining = 1.0f32;
        let mut steps_used = 0;

        for step in 0..self.config.k {
            // step_net: concat(features, h) → Linear → GELU → Linear → GELU
            let mut concat = Vec::with_capacity(self.input_dim + self.hidden_dim);
            concat.extend_from_slice(features);
            concat.extend_from_slice(&h);

            h = self.step_net_0.forward(&concat);
            for v in h.iter_mut() { *v = gelu(*v); }
            h = self.step_net_2.forward(&h);
            for v in h.iter_mut() { *v = gelu(*v); }

            // Sync accumulation
            for i in 0..self.n_sync {
                let left = h[self.sync_left[i] % self.hidden_dim];
                let right = h[self.sync_right[i] % self.hidden_dim];
                alpha[i] = r[i] * alpha[i] + left * right;
                beta[i] = r[i] * beta[i] + 1.0;
            }
            let sync: Vec<f32> = alpha.iter().zip(beta.iter())
                .map(|(a, b)| a / b.sqrt().max(1e-8))
                .collect();

            // Halt probability
            let halt_out = self.halt_net.forward(&h);
            let halt_p = sigmoid(halt_out[0]);

            // Value + Policy conditioned on sync
            let mut h_sync = Vec::with_capacity(self.hidden_dim + self.n_sync);
            h_sync.extend_from_slice(&h);
            h_sync.extend_from_slice(&sync);

            let value_out = self.value_head.forward(&h_sync);
            let value = value_out[0].tanh();

            let mut policy_out = self.policy_head.forward(&h_sync);
            softmax(&mut policy_out);

            // Weighted by halt probability
            let w = remaining * halt_p;
            total_value += w * value;
            for i in 0..NUM_ACTIONS.min(policy_out.len()) {
                total_policy[i] += w * policy_out[i];
            }

            remaining *= 1.0 - halt_p;
            steps_used = step + 1;

            if remaining < 0.01 { break; }
        }

        // Final step gets remaining weight
        if remaining > 0.01 {
            total_value += remaining * total_value / (1.0 - remaining + 1e-8);
        }

        // Normalize policy
        let psum: f32 = total_policy.iter().sum();
        if psum > 1e-8 { for p in &mut total_policy { *p /= psum; } }

        // Update persistent sync state
        if persist {
            self.persist_alpha = alpha.clone();
            self.persist_beta = beta.clone();
        }

        // Update baseline (running average for novelty detection)
        let sync_final: Vec<f32> = alpha.iter().zip(beta.iter())
            .map(|(a, b)| a / b.sqrt().max(1e-8))
            .collect();

        self.n_evals += 1;
        let decay = 0.95;
        for i in 0..self.n_sync {
            self.baseline_sync[i] = decay * self.baseline_sync[i] + (1.0 - decay) * sync_final[i];
        }

        // Store for Hebbian weight update
        self.last_sync = sync_final.clone();
        self.last_policy = total_policy;

        CTMOutput {
            value: total_value,
            policy: total_policy,
            sync_state: sync_final,
            steps_used,
        }
    }

    /// Compute sync novelty: how different is current sync from baseline
    pub fn sync_novelty(&self, current_sync: &[f32]) -> f32 {
        if self.n_evals < 5 { return 0.0; }
        let mut total_dev = 0.0f32;
        for i in 0..self.n_sync.min(current_sync.len()) {
            let dev = (current_sync[i] - self.baseline_sync[i]).abs();
            total_dev += dev;
        }
        (total_dev / self.n_sync as f32).min(1.0)
    }

    /// Hebbian weight update: modify policy_head weights based on outcome.
    ///
    /// delta_W = lr * outcome * outer(sync_novelty, last_policy)
    ///
    /// This is the core mechanism from the poker plasticity bench (0.193 score).
    /// The network literally rewires based on game outcomes — no gradients.
    ///
    /// `outcome`: +1 won, -1 lost, 0 neutral
    /// Hebbian weight update — ported from nanochat CTMBlock.hebbian_update().
    ///
    /// Key innovations from nanochat:
    /// 1. GATED novelty: only update channels with above-median surprise
    /// 2. PROJECTION: use existing weights to project novelty into action space
    /// 3. EMA delta: momentum prevents oscillation (0.95 old + 0.05 new)
    /// 4. DECAY toward baseline: prevents over-adaptation to one opponent
    ///
    /// `outcome`: +1 won, -1 lost, 0 neutral
    pub fn hebbian_update(&mut self, outcome: f32) {
        if !self.config.hebbian_enabled || self.n_evals < 5 { return; }

        // Step 1: Compute sync novelty per channel
        let novelty: Vec<f32> = self.last_sync.iter().zip(self.baseline_sync.iter())
            .map(|(s, b)| s - b)
            .collect();

        // Step 2: Gate — only channels with above-median novelty contribute
        // (from nanochat: prevents low-signal noise from corrupting updates)
        let mut abs_novelty: Vec<f32> = novelty.iter().map(|n| n.abs()).collect();
        abs_novelty.sort_by(|a, b| a.partial_cmp(b).unwrap());
        let median = abs_novelty.get(abs_novelty.len() / 2).copied().unwrap_or(0.0);
        let gated: Vec<f32> = novelty.iter()
            .map(|n| if n.abs() > median { *n } else { 0.0 })
            .collect();

        // Step 3: Project gated novelty through existing policy weights
        // (from nanochat: action_signal = c_proj.weight.T @ gated)
        // This tells us what each sync channel "means" in action space
        let mut action_signal = [0.0f32; NUM_ACTIONS];
        for action in 0..NUM_ACTIONS {
            for s in 0..self.n_sync {
                let feature_idx = self.hidden_dim + s;
                if feature_idx < self.policy_head.in_dim {
                    let w_idx = action * self.policy_head.in_dim + feature_idx;
                    if w_idx < self.policy_head.weight.len() {
                        action_signal[action] += self.policy_head.weight[w_idx] * gated[s];
                    }
                }
            }
        }

        // Step 4: Outer product: gated_sync × action_signal, scaled by outcome
        // (from nanochat: delta = plastic_lr * outer(gated, action_signal))
        let lr = self.config.hebbian_lr;
        for action in 0..NUM_ACTIONS {
            for s in 0..self.n_sync {
                let feature_idx = self.hidden_dim + s;
                if feature_idx < self.policy_head.in_dim {
                    let w_idx = action * self.policy_head.in_dim + feature_idx;
                    if w_idx < self.policy_head.weight.len() {
                        let delta = lr * outcome * gated[s] * action_signal[action];
                        // EMA: 0.95 momentum + 0.05 new (prevents oscillation)
                        self.policy_head.weight[w_idx] = self.config.hebbian_momentum * self.policy_head.weight[w_idx]
                            + (1.0 - self.config.hebbian_momentum) * (self.policy_head.weight[w_idx] + delta);
                    }
                }
            }
        }

        // Step 5: Decay toward baseline — prevents over-adaptation
        // (1% pull per hand keeps network near Nash while allowing drift)
        let decay = self.config.baseline_decay;
        if !self.baseline_policy_weights.is_empty() {
            for i in 0..self.policy_head.weight.len().min(self.baseline_policy_weights.len()) {
                self.policy_head.weight[i] = (1.0 - decay) * self.policy_head.weight[i]
                    + decay * self.baseline_policy_weights[i];
            }
        }
        if !self.baseline_value_weights.is_empty() {
            for i in 0..self.value_head.weight.len().min(self.baseline_value_weights.len()) {
                self.value_head.weight[i] = (1.0 - decay) * self.value_head.weight[i]
                    + decay * self.baseline_value_weights[i];
            }
        }
    }

    /// Reset persistent sync state
    pub fn reset_memory(&mut self) {
        self.persist_alpha = vec![0.0; self.n_sync];
        self.persist_beta = vec![1.0; self.n_sync];
    }
}
