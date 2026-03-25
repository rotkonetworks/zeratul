//! ONNX inference for CTM-MoE poker bot
//!
//! loads router + expert ONNX models, runs inference on CPU.
//! <1ms per decision for all models combined.

use std::path::Path;
use std::sync::Mutex;
use super::ctm::{NUM_FEATURES, NUM_ACTIONS, TOP_K};

pub struct OnnxMoE {
    router: Mutex<ort::session::Session>,
    experts: Vec<Option<Mutex<ort::session::Session>>>,
}

pub struct MoEOutput {
    pub value: f32,
    pub action_probs: [f32; NUM_ACTIONS],
    pub expert_indices: [usize; TOP_K],
    pub expert_weights: [f32; TOP_K],
}

fn run_model(session: &Mutex<ort::session::Session>, features: &[f32; NUM_FEATURES])
    -> Result<Vec<Vec<f32>>, ort::Error>
{
    let input = ort::value::Tensor::from_array(([1usize, NUM_FEATURES], features.to_vec()))?;
    let mut sess = session.lock().unwrap();
    let outputs = sess.run(ort::inputs![input])?;
    let mut result = Vec::new();
    for (_name, val) in outputs.iter() {
        let (_shape, data) = val.try_extract_tensor::<f32>()?;
        result.push(data.to_vec());
    }
    Ok(result)
}

impl OnnxMoE {
    pub fn load(model_dir: &str) -> Result<Self, ort::Error> {
        Self::load_version(model_dir, None)
    }

    /// Load models, auto-detecting the highest version or using a specific one.
    pub fn load_version(model_dir: &str, version: Option<u32>) -> Result<Self, ort::Error> {
        let dir = Path::new(model_dir);
        let names = ["headsup", "preflop_multi", "postflop_wet", "postflop_dry", "shortstack", "river_polar"];

        // find highest version if not specified
        let ver = version.unwrap_or_else(|| {
            let mut max_v = 1u32;
            for v in 1..=20 {
                if dir.join(format!("router_v{}.onnx", v)).exists() {
                    max_v = v;
                }
            }
            max_v
        });

        let router = ort::session::Session::builder()?
            .with_intra_threads(1)?
            .commit_from_file(dir.join(format!("router_v{}.onnx", ver)))?;

        let mut experts = Vec::new();
        for name in &names {
            let path = dir.join(format!("expert_{}_v{}.onnx", name, ver));
            if path.exists() {
                experts.push(Some(Mutex::new(ort::session::Session::builder()?
                    .with_intra_threads(1)?
                    .commit_from_file(&path)?)));
            } else {
                experts.push(None);
            }
        }
        Ok(Self { router: Mutex::new(router), experts })
    }

    pub fn evaluate(&self, features: &[f32; NUM_FEATURES]) -> Result<MoEOutput, ort::Error> {
        let router_out = run_model(&self.router, features)?;
        let logits = &router_out[0];

        let max_l = logits.iter().cloned().fold(f32::NEG_INFINITY, f32::max);
        let exp: Vec<f32> = logits.iter().map(|&x| (x - max_l).exp()).collect();
        let sum: f32 = exp.iter().sum();
        let probs: Vec<f32> = exp.iter().map(|&x| x / sum).collect();

        let mut indexed: Vec<(usize, f32)> = probs.into_iter().enumerate().collect();
        indexed.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap());
        let top_sum = indexed[0].1 + indexed[1].1;
        let weights = [indexed[0].1 / top_sum, indexed[1].1 / top_sum];

        let mut total_value = 0.0f32;
        let mut total_policy = [0.0f32; NUM_ACTIONS];

        for k in 0..TOP_K {
            let idx = indexed[k].0;
            let w = weights[k];
            let expert = match &self.experts[idx] { Some(s) => s, None => continue };

            let out = run_model(expert, features)?;
            let value = out.get(0).and_then(|v| v.first().copied()).unwrap_or(0.0);
            let policy = out.get(1).map(|v| v.as_slice()).unwrap_or(&[]);

            total_value += w * value;
            for i in 0..NUM_ACTIONS.min(policy.len()) {
                total_policy[i] += w * policy[i];
            }
        }

        let psum: f32 = total_policy.iter().sum();
        if psum > 1e-8 { for p in &mut total_policy { *p /= psum; } }

        Ok(MoEOutput {
            value: total_value,
            action_probs: total_policy,
            expert_indices: [indexed[0].0, indexed[1].0],
            expert_weights: weights,
        })
    }
}
