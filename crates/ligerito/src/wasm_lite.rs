//! Minimal WASM bindings - no serde dependency
//!
//! This module provides a lightweight WASM API that doesn't require serde/bincode.
//! Proofs are kept internal - JS only sees commitment hashes and verification results.

use wasm_bindgen::prelude::*;
use crate::{prover, verifier, hardcoded_config_12, hardcoded_config_20, hardcoded_config_24};
use crate::{hardcoded_config_12_verifier, hardcoded_config_20_verifier, hardcoded_config_24_verifier};
use binary_fields::{BinaryElem32, BinaryElem128};
use std::marker::PhantomData;

#[wasm_bindgen]
extern "C" {
    #[wasm_bindgen(js_namespace = console)]
    fn log(s: &str);
}

/// Prove and verify in one call - returns true if valid
/// This is the simplest API: just pass polynomial data and get back validity
#[wasm_bindgen]
pub fn prove_and_verify(polynomial: &[u32], config_size: u8) -> Result<bool, JsValue> {
    let poly: Vec<BinaryElem32> = polynomial
        .iter()
        .map(|&x| BinaryElem32::from(x))
        .collect();

    match config_size {
        12 => {
            let config = hardcoded_config_12(
                PhantomData::<BinaryElem32>,
                PhantomData::<BinaryElem128>,
            );
            if poly.len() != (1 << 12) {
                return Err(JsValue::from_str(&format!(
                    "Expected {} elements, got {}", 1 << 12, poly.len()
                )));
            }
            let proof = prover::<BinaryElem32, BinaryElem128>(&config, &poly)
                .map_err(|e| JsValue::from_str(&format!("Proving failed: {}", e)))?;
            let verifier_config = hardcoded_config_12_verifier();
            verifier::<BinaryElem32, BinaryElem128>(&verifier_config, &proof)
                .map_err(|e| JsValue::from_str(&format!("Verification failed: {}", e)))
        }
        20 => {
            let config = hardcoded_config_20(
                PhantomData::<BinaryElem32>,
                PhantomData::<BinaryElem128>,
            );
            if poly.len() != (1 << 20) {
                return Err(JsValue::from_str(&format!(
                    "Expected {} elements, got {}", 1 << 20, poly.len()
                )));
            }
            let proof = prover::<BinaryElem32, BinaryElem128>(&config, &poly)
                .map_err(|e| JsValue::from_str(&format!("Proving failed: {}", e)))?;
            let verifier_config = hardcoded_config_20_verifier();
            verifier::<BinaryElem32, BinaryElem128>(&verifier_config, &proof)
                .map_err(|e| JsValue::from_str(&format!("Verification failed: {}", e)))
        }
        24 => {
            let config = hardcoded_config_24(
                PhantomData::<BinaryElem32>,
                PhantomData::<BinaryElem128>,
            );
            if poly.len() != (1 << 24) {
                return Err(JsValue::from_str(&format!(
                    "Expected {} elements, got {}", 1 << 24, poly.len()
                )));
            }
            let proof = prover::<BinaryElem32, BinaryElem128>(&config, &poly)
                .map_err(|e| JsValue::from_str(&format!("Proving failed: {}", e)))?;
            let verifier_config = hardcoded_config_24_verifier();
            verifier::<BinaryElem32, BinaryElem128>(&verifier_config, &proof)
                .map_err(|e| JsValue::from_str(&format!("Verification failed: {}", e)))
        }
        _ => Err(JsValue::from_str(&format!(
            "Unsupported config_size: {}. Supported: 12, 20, 24", config_size
        ))),
    }
}

/// Get commitment hash from polynomial (32 bytes)
#[wasm_bindgen]
pub fn get_commitment(polynomial: &[u32], config_size: u8) -> Result<Vec<u8>, JsValue> {
    let poly: Vec<BinaryElem32> = polynomial
        .iter()
        .map(|&x| BinaryElem32::from(x))
        .collect();

    match config_size {
        12 => {
            let config = hardcoded_config_12(
                PhantomData::<BinaryElem32>,
                PhantomData::<BinaryElem128>,
            );
            if poly.len() != (1 << 12) {
                return Err(JsValue::from_str("Wrong polynomial size"));
            }
            let proof = prover::<BinaryElem32, BinaryElem128>(&config, &poly)
                .map_err(|e| JsValue::from_str(&format!("Failed: {}", e)))?;
            let root = proof.initial_ligero_cm.root.root.ok_or_else(|| JsValue::from_str("No root"))?;
            Ok(root.to_vec())
        }
        20 => {
            let config = hardcoded_config_20(
                PhantomData::<BinaryElem32>,
                PhantomData::<BinaryElem128>,
            );
            if poly.len() != (1 << 20) {
                return Err(JsValue::from_str("Wrong polynomial size"));
            }
            let proof = prover::<BinaryElem32, BinaryElem128>(&config, &poly)
                .map_err(|e| JsValue::from_str(&format!("Failed: {}", e)))?;
            let root = proof.initial_ligero_cm.root.root.ok_or_else(|| JsValue::from_str("No root"))?;
            Ok(root.to_vec())
        }
        24 => {
            let config = hardcoded_config_24(
                PhantomData::<BinaryElem32>,
                PhantomData::<BinaryElem128>,
            );
            if poly.len() != (1 << 24) {
                return Err(JsValue::from_str("Wrong polynomial size"));
            }
            let proof = prover::<BinaryElem32, BinaryElem128>(&config, &poly)
                .map_err(|e| JsValue::from_str(&format!("Failed: {}", e)))?;
            let root = proof.initial_ligero_cm.root.root.ok_or_else(|| JsValue::from_str("No root"))?;
            Ok(root.to_vec())
        }
        _ => Err(JsValue::from_str("Unsupported config_size")),
    }
}

#[wasm_bindgen(start)]
pub fn init() {
    console_error_panic_hook::set_once();
}
