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
