//! WASM bindings for Ligerito prover and verifier
//!
//! This module enables running Ligerito in the browser!
//!
//! ## Usage in JavaScript:
//!
//! ```javascript
//! import init, { prove, verify } from './ligerito_wasm.js';
//!
//! async function main() {
//!   // Initialize WASM module
//!   await init();
//!
//!   // Generate a proof
//!   const polynomial = new Uint32Array(4096); // 2^12 elements
//!   for (let i = 0; i < polynomial.length; i++) {
//!     polynomial[i] = Math.floor(Math.random() * 0xFFFFFFFF);
//!   }
//!
//!   const proof = prove(polynomial, 12); // config_size = 12 (2^12)
//!   console.log('Proof size:', proof.length, 'bytes');
//!
//!   // Verify the proof
//!   const isValid = verify(proof, 12);
//!   console.log('Proof is valid:', isValid);
//! }
//! ```

use wasm_bindgen::prelude::*;
use crate::{
    prover::prove_with_transcript,
    verifier::verify_with_transcript,
    hardcoded_config_12, hardcoded_config_16, hardcoded_config_20, hardcoded_config_24,
    hardcoded_config_28, hardcoded_config_30,
    hardcoded_config_12_verifier, hardcoded_config_16_verifier, hardcoded_config_20_verifier,
    hardcoded_config_24_verifier, hardcoded_config_28_verifier, hardcoded_config_30_verifier,
    FinalizedLigeritoProof,
    transcript::FiatShamir,
};
use binary_fields::{BinaryElem32, BinaryElem128};
use std::marker::PhantomData;

// Re-export wasm-bindgen-rayon's init_thread_pool for JavaScript
#[cfg(feature = "wasm-parallel")]
pub use wasm_bindgen_rayon::init_thread_pool;

#[wasm_bindgen]
extern "C" {
    #[wasm_bindgen(js_namespace = console)]
    fn log(s: &str);
}

macro_rules! console_log {
    ($($t:tt)*) => (log(&format!($($t)*)))
}

/// Create transcript based on type string
fn create_transcript(transcript_type: &str) -> Result<FiatShamir, JsValue> {
    match transcript_type {
        "sha256" => Ok(FiatShamir::new_sha256(0)),
        #[cfg(feature = "transcript-merlin")]
        "merlin" => Ok(FiatShamir::new_merlin()),
        #[cfg(feature = "transcript-blake2b")]
        "blake2b" => Ok(FiatShamir::new_blake2b()),
        _ => Err(JsValue::from_str(&format!(
            "Unsupported transcript: {}. Available: sha256{}{}",
            transcript_type,
            if cfg!(feature = "transcript-merlin") { ", merlin" } else { "" },
            if cfg!(feature = "transcript-blake2b") { ", blake2b" } else { "" },
        ))),
    }
}

/// Generate a Ligerito proof from a polynomial
///
/// # Arguments
/// * `polynomial` - Polynomial coefficients as u32 array
/// * `config_size` - Log2 of polynomial size (12, 20, or 24)
/// * `transcript` - Optional transcript type: "sha256" (default), "merlin", or "blake2b"
///
/// # Returns
/// Serialized proof bytes
///
/// # Example (JavaScript)
/// ```javascript
/// const polynomial = new Uint32Array(4096); // 2^12
/// // Fill with data...
/// const proof = prove(polynomial, 12, "sha256");
/// ```
#[wasm_bindgen]
pub fn prove(polynomial: &[u32], config_size: u8, transcript: Option<String>) -> Result<Vec<u8>, JsValue> {
    let transcript_type = transcript.as_deref().unwrap_or("sha256");
    let fs = create_transcript(transcript_type)?;

    // Convert to BinaryElem32
    let poly: Vec<BinaryElem32> = polynomial
        .iter()
        .map(|&x| BinaryElem32::from(x))
        .collect();

    // Helper macro to reduce repetition
    macro_rules! prove_with_config {
        ($config_fn:ident, $expected_size:expr) => {{
            let config = $config_fn(
                PhantomData::<BinaryElem32>,
                PhantomData::<BinaryElem128>,
            );

            if poly.len() != (1 << $expected_size) {
                return Err(JsValue::from_str(&format!(
                    "Expected {} elements for config_size {}, got {}",
                    1 << $expected_size, $expected_size, poly.len()
                )));
            }

            prove_with_transcript(&config, &poly, fs)
                .map_err(|e| JsValue::from_str(&format!("Proving failed: {}", e)))?
        }};
    }

    // Get appropriate config and prove
    let proof = match config_size {
        12 => prove_with_config!(hardcoded_config_12, 12),
        16 => prove_with_config!(hardcoded_config_16, 16),
        20 => prove_with_config!(hardcoded_config_20, 20),
        24 => prove_with_config!(hardcoded_config_24, 24),
        28 => prove_with_config!(hardcoded_config_28, 28),
        30 => prove_with_config!(hardcoded_config_30, 30),
        _ => {
            return Err(JsValue::from_str(&format!(
                "Unsupported config_size: {}. Supported: 12, 16, 20, 24, 28, 30",
                config_size
            )));
        }
    };

    // Serialize proof
    bincode::serialize(&proof)
        .map_err(|e| JsValue::from_str(&format!("Serialization failed: {}", e)))
}

/// Verify a Ligerito proof
///
/// # Arguments
/// * `proof_bytes` - Serialized proof bytes (from `prove()`)
/// * `config_size` - Log2 of polynomial size (12, 20, or 24)
/// * `transcript` - Optional transcript type: "sha256" (default), "merlin", or "blake2b"
///   Must match the transcript used when generating the proof!
///
/// # Returns
/// true if proof is valid, false otherwise
///
/// # Example (JavaScript)
/// ```javascript
/// const isValid = verify(proofBytes, 12, "sha256");
/// console.log('Valid:', isValid);
/// ```
#[wasm_bindgen]
pub fn verify(proof_bytes: &[u8], config_size: u8, transcript: Option<String>) -> Result<bool, JsValue> {
    let transcript_type = transcript.as_deref().unwrap_or("sha256");
    let fs = create_transcript(transcript_type)?;

    // Deserialize proof with explicit type
    let proof: FinalizedLigeritoProof<BinaryElem32, BinaryElem128> = bincode::deserialize(proof_bytes)
        .map_err(|e| JsValue::from_str(&format!("Deserialization failed: {}", e)))?;

    // Helper macro to reduce repetition
    macro_rules! verify_with_config {
        ($config_fn:ident) => {{
            let config = $config_fn();
            verify_with_transcript(&config, &proof, fs)
                .map_err(|e| JsValue::from_str(&format!("Verification failed: {}", e)))?
        }};
    }

    // Get appropriate config and verify
    let result = match config_size {
        12 => verify_with_config!(hardcoded_config_12_verifier),
        16 => verify_with_config!(hardcoded_config_16_verifier),
        20 => verify_with_config!(hardcoded_config_20_verifier),
        24 => verify_with_config!(hardcoded_config_24_verifier),
        28 => verify_with_config!(hardcoded_config_28_verifier),
        30 => verify_with_config!(hardcoded_config_30_verifier),
        _ => {
            return Err(JsValue::from_str(&format!(
                "Unsupported config_size: {}. Supported: 12, 16, 20, 24, 28, 30",
                config_size
            )));
        }
    };

    Ok(result)
}

/// Get the expected polynomial size for a given config
///
/// # Example (JavaScript)
/// ```javascript
/// const size = get_polynomial_size(12); // Returns 4096 (2^12)
/// ```
#[wasm_bindgen]
pub fn get_polynomial_size(config_size: u8) -> Result<usize, JsValue> {
    match config_size {
        12 | 16 | 20 | 24 | 28 | 30 => Ok(1 << config_size),
        _ => Err(JsValue::from_str(&format!(
            "Unsupported config_size: {}",
            config_size
        ))),
    }
}

/// Generate random polynomial and prove it entirely within WASM
///
/// This avoids copying large polynomials from JS to WASM, which is crucial
/// for large sizes like 2^28 (1GB of data).
///
/// # Arguments
/// * `config_size` - Log2 of polynomial size (12, 16, 20, 24, 28, 30)
/// * `seed` - Random seed for reproducibility
/// * `transcript` - Optional transcript type: "sha256" (default), "merlin", or "blake2b"
///
/// # Returns
/// Serialized proof bytes
#[wasm_bindgen]
pub fn generate_and_prove(config_size: u8, seed: u64, transcript: Option<String>) -> Result<Vec<u8>, JsValue> {
    use rand::{SeedableRng, Rng};
    use rand_chacha::ChaCha8Rng;

    let transcript_type = transcript.as_deref().unwrap_or("sha256");
    let fs = create_transcript(transcript_type)?;

    let size: usize = 1 << config_size;
    console_log!("Generating {} random elements in WASM...", size);

    // Generate random polynomial directly in WASM memory
    let mut rng = ChaCha8Rng::seed_from_u64(seed);
    let poly: Vec<BinaryElem32> = (0..size)
        .map(|_| BinaryElem32::from(rng.gen::<u32>()))
        .collect();

    console_log!("Polynomial generated, starting proof...");

    // Helper macro to reduce repetition
    macro_rules! prove_with_config {
        ($config_fn:ident) => {{
            let config = $config_fn(
                PhantomData::<BinaryElem32>,
                PhantomData::<BinaryElem128>,
            );
            prove_with_transcript(&config, &poly, fs)
                .map_err(|e| JsValue::from_str(&format!("Proving failed: {}", e)))?
        }};
    }

    // Get appropriate config and prove
    let proof = match config_size {
        12 => prove_with_config!(hardcoded_config_12),
        16 => prove_with_config!(hardcoded_config_16),
        20 => prove_with_config!(hardcoded_config_20),
        24 => prove_with_config!(hardcoded_config_24),
        28 => prove_with_config!(hardcoded_config_28),
        30 => prove_with_config!(hardcoded_config_30),
        _ => {
            return Err(JsValue::from_str(&format!(
                "Unsupported config_size: {}. Supported: 12, 16, 20, 24, 28, 30",
                config_size
            )));
        }
    };

    console_log!("Proof generated, serializing...");

    // Serialize proof
    bincode::serialize(&proof)
        .map_err(|e| JsValue::from_str(&format!("Serialization failed: {}", e)))
}

/// Initialize the WASM module (sets up panic hook for better error messages)
#[wasm_bindgen(start)]
pub fn init() {
    console_error_panic_hook::set_once();
    console_log!("Ligerito WASM module initialized");
}

// NOTE: init_thread_pool is re-exported from wasm-bindgen-rayon (see top of file)
// JavaScript can import it as initThreadPool
//
// Example usage:
// ```javascript
// import init, { initThreadPool } from './ligerito.js';
//
// await init();
// const numThreads = await initThreadPool(navigator.hardwareConcurrency || 4);
// console.log(`Initialized ${numThreads} worker threads`);
// ```

//
// WebGPU Benchmark Functions
//
// These functions expose GPU-accelerated sumcheck for benchmarking in the browser
//

use js_sys::Promise;
use wasm_bindgen_futures::JsFuture;

/// Benchmark configuration for sumcheck tests
#[wasm_bindgen]
#[derive(Clone, Copy)]
pub struct BenchConfig {
    pub n: usize,      // log2 of basis size
    pub k: usize,      // log2 of row size
    pub q: usize,      // number of queries
}

#[wasm_bindgen]
impl BenchConfig {
    #[wasm_bindgen(constructor)]
    pub fn new(n: usize, k: usize, q: usize) -> BenchConfig {
        BenchConfig { n, k, q }
    }
}

/// Result from a sumcheck benchmark
#[wasm_bindgen]
pub struct BenchResult {
    time_ms: f64,
    success: bool,
    error: Option<String>,
}

#[wasm_bindgen]
impl BenchResult {
    #[wasm_bindgen(getter)]
    pub fn time_ms(&self) -> f64 {
        self.time_ms
    }

    #[wasm_bindgen(getter)]
    pub fn success(&self) -> bool {
        self.success
    }

    #[wasm_bindgen(getter)]
    pub fn error(&self) -> Option<String> {
        self.error.clone()
    }
}

/// Run CPU sumcheck benchmark
///
/// # Example (JavaScript)
/// ```javascript
/// const config = new BenchConfig(10, 6, 32);  // n=10, k=6, q=32
/// const result = await benchCpuSumcheck(config);
/// console.log(`CPU time: ${result.time_ms}ms`);
/// ```
#[wasm_bindgen]
pub fn bench_cpu_sumcheck(config: BenchConfig) -> Promise {
    use wasm_bindgen_futures::future_to_promise;

    future_to_promise(async move {
        let start = js_sys::Date::now();

        // Generate test data
        let result = run_cpu_sumcheck_internal(config.n, config.k, config.q);

        let elapsed = js_sys::Date::now() - start;

        match result {
            Ok(_) => Ok(JsValue::from(BenchResult {
                time_ms: elapsed,
                success: true,
                error: None,
            })),
            Err(e) => Ok(JsValue::from(BenchResult {
                time_ms: elapsed,
                success: false,
                error: Some(e),
            })),
        }
    })
}

/// Run GPU sumcheck benchmark (requires WebGPU support)
///
/// # Example (JavaScript)
/// ```javascript
/// const config = new BenchConfig(10, 6, 32);
/// const result = await benchGpuSumcheck(config);
/// console.log(`GPU time: ${result.time_ms}ms`);
/// ```
#[cfg(feature = "webgpu")]
#[wasm_bindgen]
pub fn bench_gpu_sumcheck(config: BenchConfig) -> Promise {
    use wasm_bindgen_futures::future_to_promise;

    future_to_promise(async move {
        let start = js_sys::Date::now();

        // Run GPU sumcheck
        let result = run_gpu_sumcheck_internal(config.n, config.k, config.q).await;

        let elapsed = js_sys::Date::now() - start;

        match result {
            Ok(_) => Ok(JsValue::from(BenchResult {
                time_ms: elapsed,
                success: true,
                error: None,
            })),
            Err(e) => Ok(JsValue::from(BenchResult {
                time_ms: elapsed,
                success: false,
                error: Some(e),
            })),
        }
    })
}

/// Check if WebGPU is available
#[cfg(feature = "webgpu")]
#[wasm_bindgen]
pub async fn check_webgpu_available() -> bool {
    use crate::gpu::GpuDevice;

    match GpuDevice::new().await {
        Ok(_) => true,
        Err(_) => false,
    }
}

//
// Internal helper functions
//

fn run_cpu_sumcheck_internal(n: usize, k: usize, q: usize) -> Result<(), String> {
    use ligerito_binary_fields::BinaryElem128;
    use crate::sumcheck_polys::induce_sumcheck_poly;

    let row_size = 1 << k;

    // Generate test data
    let sks_vks: Vec<BinaryElem128> = (0..=n)
        .map(|i| BinaryElem128::from_value((i as u128).wrapping_mul(0x123456789ABCDEF)))
        .collect();

    let opened_rows: Vec<Vec<BinaryElem128>> = (0..q)
        .map(|query| {
            (0..row_size)
                .map(|i| {
                    BinaryElem128::from_value(
                        ((query * 1000 + i) as u128).wrapping_mul(0xFEDCBA987654321)
                    )
                })
                .collect()
        })
        .collect();

    let v_challenges: Vec<BinaryElem128> = (0..k)
        .map(|i| BinaryElem128::from_value((i as u128).wrapping_mul(0x111111111111111)))
        .collect();

    let sorted_queries: Vec<usize> = (0..q)
        .map(|i| i * 17 % (1 << n))
        .collect();

    let alpha = BinaryElem128::from_value(0xABCDEF0123456789);

    // Run CPU sumcheck
    let (_basis_poly, _enforced_sum) = induce_sumcheck_poly(
        n,
        &sks_vks,
        &opened_rows,
        &v_challenges,
        &sorted_queries,
        alpha,
    );

    Ok(())
}

#[cfg(feature = "webgpu")]
async fn run_gpu_sumcheck_internal(n: usize, k: usize, q: usize) -> Result<(), String> {
    use ligerito_binary_fields::BinaryElem128;
    use crate::gpu::{GpuDevice, sumcheck::GpuSumcheck};

    let row_size = 1 << k;

    // Initialize GPU device
    let device = GpuDevice::new().await
        .map_err(|e| format!("GPU initialization failed: {}", e))?;

    let mut gpu_sumcheck = GpuSumcheck::new(device);

    // Generate test data
    let sks_vks: Vec<BinaryElem128> = (0..=n)
        .map(|i| BinaryElem128::from_value((i as u128).wrapping_mul(0x123456789ABCDEF)))
        .collect();

    let opened_rows: Vec<Vec<BinaryElem128>> = (0..q)
        .map(|query| {
            (0..row_size)
                .map(|i| {
                    BinaryElem128::from_value(
                        ((query * 1000 + i) as u128).wrapping_mul(0xFEDCBA987654321)
                    )
                })
                .collect()
        })
        .collect();

    let v_challenges: Vec<BinaryElem128> = (0..k)
        .map(|i| BinaryElem128::from_value((i as u128).wrapping_mul(0x111111111111111)))
        .collect();

    let sorted_queries: Vec<usize> = (0..q)
        .map(|i| i * 17 % (1 << n))
        .collect();

    let alpha = BinaryElem128::from_value(0xABCDEF0123456789);

    // Run GPU sumcheck
    let (_basis_poly, _enforced_sum) = gpu_sumcheck
        .induce_sumcheck_poly(
            n,
            &sks_vks,
            &opened_rows,
            &v_challenges,
            &sorted_queries,
            alpha,
        )
        .await
        .map_err(|e| format!("GPU sumcheck failed: {}", e))?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_get_polynomial_size() {
        assert_eq!(get_polynomial_size(12).unwrap(), 4096);
        assert_eq!(get_polynomial_size(20).unwrap(), 1048576);
        assert_eq!(get_polynomial_size(24).unwrap(), 16777216);
    }
}
