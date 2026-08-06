//! Native Rust MoE — loads PyTorch-trained expert weights, runs with Hebbian.
//!
//! No ONNX, no Python, no dependencies.
//! Each expert is a CTM with thinking steps + halt gate.
//! Router picks top-2 experts per decision, blends outputs.
//! Hebbian modifies policy_head weights during play.

use super::ctm::NUM_ACTIONS;
use super::ctm_native::{Linear, gelu, sigmoid, softmax};

/// One MoE expert — simplified CTM (no sync accumulators)
#[derive(Clone)]
pub struct NativeExpert {
    pub step_net_0: Linear,   // Linear(input+hidden, hidden)
    pub step_net_2: Linear,   // Linear(hidden, hidden)
    pub halt_net: Linear,     // Linear(hidden, 1)
    pub value_head: Linear,   // Linear(hidden, 1)
    pub policy_head: Linear,  // Linear(hidden, NUM_ACTIONS)
    pub init_hidden: Vec<f32>,
    pub max_ticks: usize,
    pub hidden_dim: usize,
    // Hebbian state
    pub hebbian_enabled: bool,
    pub hebbian_lr: f32,
    last_hidden: Vec<f32>,
    last_policy: [f32; NUM_ACTIONS],
    baseline_policy_weights: Vec<f32>,
    n_evals: u32,
}

/// Router — two linear layers with GELU
#[derive(Clone)]
pub struct NativeRouter {
    pub l1: Linear,  // Linear(input, 64)
    pub l2: Linear,  // Linear(64, num_experts)
}

/// Complete MoE — router + experts
pub struct NativeMoE {
    pub router: NativeRouter,
    pub experts: Vec<NativeExpert>,
    pub expert_names: Vec<String>,
    pub input_dim: usize,
    pub hidden_dim: usize,
    pub blend_weight: f32,  // how much MoE influences blueprint (0.0-1.0)
}

pub struct MoEOutput {
    pub value: f32,
    pub policy: [f32; NUM_ACTIONS],
    pub expert_idx: [usize; 2],
    pub expert_weights: [f32; 2],
}

impl NativeExpert {
    fn new(input_dim: usize, hidden_dim: usize) -> Self {
        Self {
            step_net_0: Linear { weight: vec![0.0; (input_dim + hidden_dim) * hidden_dim], bias: vec![0.0; hidden_dim], in_dim: input_dim + hidden_dim, out_dim: hidden_dim },
            step_net_2: Linear { weight: vec![0.0; hidden_dim * hidden_dim], bias: vec![0.0; hidden_dim], in_dim: hidden_dim, out_dim: hidden_dim },
            halt_net: Linear { weight: vec![0.0; hidden_dim], bias: vec![0.0; 1], in_dim: hidden_dim, out_dim: 1 },
            value_head: Linear { weight: vec![0.0; hidden_dim], bias: vec![0.0; 1], in_dim: hidden_dim, out_dim: 1 },
            policy_head: Linear { weight: vec![0.0; hidden_dim * NUM_ACTIONS], bias: vec![0.0; NUM_ACTIONS], in_dim: hidden_dim, out_dim: NUM_ACTIONS },
            init_hidden: vec![0.0; hidden_dim],
            max_ticks: 8,
            hidden_dim,
            hebbian_enabled: false,
            hebbian_lr: 0.001,
            last_hidden: vec![0.0; hidden_dim],
            last_policy: [0.0; NUM_ACTIONS],
            baseline_policy_weights: Vec::new(),
            n_evals: 0,
        }
    }

    pub fn enable_hebbian(&mut self, lr: f32) {
        self.hebbian_enabled = true;
        self.hebbian_lr = lr;
        self.baseline_policy_weights = self.policy_head.weight.clone();
    }

    fn forward(&mut self, input: &[f32]) -> (f32, [f32; NUM_ACTIONS]) {
        let mut h = self.init_hidden.clone();
        let mut total_value = 0.0f32;
        let mut total_policy = [0.0f32; NUM_ACTIONS];
        let mut remaining = 1.0f32;

        for _step in 0..self.max_ticks {
            // Concatenate input + hidden
            let mut cat = Vec::with_capacity(input.len() + self.hidden_dim);
            cat.extend_from_slice(input);
            cat.extend_from_slice(&h);

            // step_net: Linear → GELU → Linear → GELU
            let mut h1 = self.step_net_0.forward(&cat);
            for x in h1.iter_mut() { *x = gelu(*x); }
            let mut h2 = self.step_net_2.forward(&h1);
            for x in h2.iter_mut() { *x = gelu(*x); }
            h = h2;

            // Halt
            let halt_out = self.halt_net.forward(&h);
            let halt_p = sigmoid(halt_out[0]);

            // Value + Policy (from hidden only, no sync)
            let value_out = self.value_head.forward(&h);
            let value = value_out[0].tanh();

            let mut policy_out = self.policy_head.forward(&h);
            softmax(&mut policy_out);

            let w = remaining * halt_p;
            total_value += w * value;
            for i in 0..NUM_ACTIONS.min(policy_out.len()) {
                total_policy[i] += w * policy_out[i];
            }
            remaining *= 1.0 - halt_p;
            if remaining < 0.01 { break; }
        }

        // Remainder
        if remaining > 0.01 {
            let value_out = self.value_head.forward(&h);
            let value = value_out[0].tanh();
            let mut policy_out = self.policy_head.forward(&h);
            softmax(&mut policy_out);
            total_value += remaining * value;
            for i in 0..NUM_ACTIONS.min(policy_out.len()) {
                total_policy[i] += remaining * policy_out[i];
            }
        }

        // Normalize
        let psum: f32 = total_policy.iter().sum();
        if psum > 1e-8 { for p in &mut total_policy { *p /= psum; } }

        self.last_hidden = h;
        self.last_policy = total_policy;
        self.n_evals += 1;

        (total_value, total_policy)
    }

    /// Hebbian update based on hand outcome
    pub fn hebbian_update(&mut self, outcome: f32) {
        if !self.hebbian_enabled || self.n_evals < 2 { return; }

        let lr = self.hebbian_lr;
        // Simple Hebbian: reinforce actions that led to wins
        // delta_W[action][hidden] = lr * outcome * last_hidden[hidden] * last_policy[action]
        for action in 0..NUM_ACTIONS {
            for j in 0..self.hidden_dim {
                let w_idx = action * self.hidden_dim + j;
                if w_idx < self.policy_head.weight.len() {
                    let delta = lr * outcome * self.last_hidden[j] * self.last_policy[action];
                    self.policy_head.weight[w_idx] += delta;
                }
            }
        }

        // Decay toward baseline (1% per hand)
        if !self.baseline_policy_weights.is_empty() {
            let decay = 0.01;
            for i in 0..self.policy_head.weight.len().min(self.baseline_policy_weights.len()) {
                self.policy_head.weight[i] = (1.0 - decay) * self.policy_head.weight[i]
                    + decay * self.baseline_policy_weights[i];
            }
        }
    }
}

impl NativeRouter {
    fn forward(&self, input: &[f32]) -> Vec<f32> {
        let mut h = self.l1.forward(input);
        for x in h.iter_mut() { *x = gelu(*x); }
        self.l2.forward(&h)
    }
}

impl NativeMoE {
    /// Load from binary file produced by export_native.py
    pub fn load(path: &str) -> Result<Self, String> {
        let data = std::fs::read(path).map_err(|e| format!("read {}: {}", path, e))?;
        let mut pos = 0;

        let read_u32 = |p: &mut usize| -> u32 {
            let v = u32::from_le_bytes([data[*p], data[*p+1], data[*p+2], data[*p+3]]);
            *p += 4;
            v
        };
        let read_f32_slice = |p: &mut usize, n: usize| -> Vec<f32> {
            let mut v = Vec::with_capacity(n);
            for _ in 0..n {
                v.push(f32::from_le_bytes([data[*p], data[*p+1], data[*p+2], data[*p+3]]));
                *p += 4;
            }
            v
        };

        let num_experts = read_u32(&mut pos) as usize;
        let input_dim = read_u32(&mut pos) as usize;
        let hidden_dim = read_u32(&mut pos) as usize;
        let n_actions = read_u32(&mut pos) as usize;

        let expert_names = vec![
            "preflop_multi".into(), "postflop_wet".into(), "postflop_dry".into(),
            "shortstack".into(), "river_polar".into(),
        ];

        let mut experts = Vec::with_capacity(num_experts);
        for _ in 0..num_experts {
            let max_ticks = data[pos] as usize; pos += 1;

            let mut expert = NativeExpert::new(input_dim, hidden_dim);
            expert.max_ticks = max_ticks;

            // step_net_0: weight [hidden, input+hidden], bias [hidden]
            expert.step_net_0.weight = read_f32_slice(&mut pos, hidden_dim * (input_dim + hidden_dim));
            expert.step_net_0.bias = read_f32_slice(&mut pos, hidden_dim);
            // step_net_2: weight [hidden, hidden], bias [hidden]
            expert.step_net_2.weight = read_f32_slice(&mut pos, hidden_dim * hidden_dim);
            expert.step_net_2.bias = read_f32_slice(&mut pos, hidden_dim);
            // halt: weight [1, hidden], bias [1]
            expert.halt_net.weight = read_f32_slice(&mut pos, hidden_dim);
            expert.halt_net.bias = read_f32_slice(&mut pos, 1);
            // vhead: weight [1, hidden], bias [1]
            expert.value_head.weight = read_f32_slice(&mut pos, hidden_dim);
            expert.value_head.bias = read_f32_slice(&mut pos, 1);
            // phead: weight [n_actions, hidden], bias [n_actions]
            expert.policy_head.weight = read_f32_slice(&mut pos, n_actions * hidden_dim);
            expert.policy_head.bias = read_f32_slice(&mut pos, n_actions);
            // h0: [hidden]
            expert.init_hidden = read_f32_slice(&mut pos, hidden_dim);

            experts.push(expert);
        }

        // Router: l1 [64, input], l2 [num_experts, 64]
        let router_hidden = 64;
        let router = NativeRouter {
            l1: Linear {
                weight: read_f32_slice(&mut pos, router_hidden * input_dim),
                bias: read_f32_slice(&mut pos, router_hidden),
                in_dim: input_dim,
                out_dim: router_hidden,
            },
            l2: Linear {
                weight: read_f32_slice(&mut pos, num_experts * router_hidden),
                bias: read_f32_slice(&mut pos, num_experts),
                in_dim: router_hidden,
                out_dim: num_experts,
            },
        };

        println!("[moe-native] loaded {} experts, {}→{} dims, {} actions",
            num_experts, input_dim, hidden_dim, n_actions);

        Ok(Self {
            router,
            experts,
            expert_names,
            input_dim,
            hidden_dim,
            blend_weight: 0.3,
        })
    }

    /// Run MoE inference — route to top-2 experts, blend outputs
    pub fn evaluate(&mut self, features: &[f32]) -> MoEOutput {
        // Route
        let mut logits = self.router.forward(features);
        let n = logits.len();

        // Top-2 selection
        let mut best = (0, f32::NEG_INFINITY);
        let mut second = (0, f32::NEG_INFINITY);
        for (i, &l) in logits.iter().enumerate() {
            if l > best.1 { second = best; best = (i, l); }
            else if l > second.1 { second = (i, l); }
        }

        // Softmax over top-2
        let max = best.1.max(second.1);
        let e1 = (best.1 - max).exp();
        let e2 = (second.1 - max).exp();
        let sum = e1 + e2;
        let w1 = e1 / sum;
        let w2 = e2 / sum;

        // Run top-2 experts
        let (v1, p1) = self.experts[best.0].forward(features);
        let (v2, p2) = self.experts[second.0].forward(features);

        // Blend
        let value = w1 * v1 + w2 * v2;
        let mut policy = [0.0f32; NUM_ACTIONS];
        for i in 0..NUM_ACTIONS {
            policy[i] = w1 * p1[i] + w2 * p2[i];
        }

        MoEOutput {
            value,
            policy,
            expert_idx: [best.0, second.0],
            expert_weights: [w1, w2],
        }
    }

    /// Feed outcome to Hebbian on the experts that were used
    pub fn hebbian_update(&mut self, outcome: f32, expert_idx: &[usize; 2]) {
        self.experts[expert_idx[0]].hebbian_update(outcome);
        self.experts[expert_idx[1]].hebbian_update(outcome);
    }

    /// Enable Hebbian on all experts
    pub fn enable_hebbian(&mut self, lr: f32) {
        for expert in &mut self.experts {
            expert.enable_hebbian(lr);
        }
    }
}
