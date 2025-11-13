//! Ligerito Verifier Guest Program for PolkaVM/CoreVM
//!
//! This is the GUEST program that runs inside PolkaVM.
//! It reads proof data from stdin, verifies it, and returns the result via exit code.
//!
//! # Build
//!
//! ```bash
//! cd examples/polkavm_verifier
//! . ../../polkaports/activate.sh polkavm
//! make
//! ```
//!
//! # Protocol
//!
//! Input (stdin): [config_size: u32][proof_bytes: bincode]
//! Output (exit code): 0 = valid, 1 = invalid, 2 = error

use ligerito::{verify, FinalizedLigeritoProof};
use binary_fields::{BinaryElem32, BinaryElem128};
use std::io::{self, Read};

fn main() {
    // Read all input from stdin
    let mut input = Vec::new();
    if let Err(e) = io::stdin().read_to_end(&mut input) {
        eprintln!("Error reading input: {}", e);
        std::process::exit(2);
    }

    // Parse config size (first 4 bytes)
    if input.len() < 4 {
        eprintln!("Input too short: need at least 4 bytes for config_size");
        std::process::exit(2);
    }

    let config_size = u32::from_le_bytes([input[0], input[1], input[2], input[3]]);
    let proof_bytes = &input[4..];

    // Deserialize proof
    let proof: FinalizedLigeritoProof<BinaryElem32, BinaryElem128> =
        match bincode::deserialize(proof_bytes) {
            Ok(p) => p,
            Err(e) => {
                eprintln!("Failed to deserialize proof: {}", e);
                std::process::exit(2);
            }
        };

    // Get appropriate config and verify
    let result = match config_size {
        12 => verify(&ligerito::hardcoded_config_12_verifier(), &proof),
        16 => verify(&ligerito::hardcoded_config_16_verifier(), &proof),
        20 => verify(&ligerito::hardcoded_config_20_verifier(), &proof),
        24 => verify(&ligerito::hardcoded_config_24_verifier(), &proof),
        28 => verify(&ligerito::hardcoded_config_28_verifier(), &proof),
        30 => verify(&ligerito::hardcoded_config_30_verifier(), &proof),
        _ => {
            eprintln!("Unsupported config size: {}", config_size);
            std::process::exit(2);
        }
    };

    // Return result via exit code
    match result {
        Ok(true) => {
            println!("VALID");
            std::process::exit(0);
        }
        Ok(false) => {
            println!("INVALID");
            std::process::exit(1);
        }
        Err(e) => {
            eprintln!("Verification error: {:?}", e);
            std::process::exit(2);
        }
    }
}

#[cfg(feature = "ffi")]
mod ffi {
    use super::*;
    use core::slice;

    /// FFI function for verifying a proof from C/PolkaVM
    ///
    /// # Safety
    /// Caller must ensure proof_ptr and config_ptr point to valid memory
    ///
    /// # Arguments
    /// * `proof_ptr` - Pointer to serialized proof bytes
    /// * `proof_len` - Length of proof in bytes
    /// * `config_size` - Log2 size (12, 16, 20, 24, 28, or 30)
    ///
    /// # Returns
    /// * 1 if proof is valid
    /// * 0 if proof is invalid or error occurred
    #[no_mangle]
    pub unsafe extern "C" fn ligerito_verify(
        proof_ptr: *const u8,
        proof_len: usize,
        config_size: u32,
    ) -> i32 {
        // Safety check
        if proof_ptr.is_null() {
            return 0;
        }

        // Convert raw pointer to slice
        let proof_bytes = slice::from_raw_parts(proof_ptr, proof_len);

        // Deserialize proof (using bincode)
        let proof: FinalizedLigeritoProof<BinaryElem32, BinaryElem128> =
            match bincode::deserialize(proof_bytes) {
                Ok(p) => p,
                Err(_) => return 0,  // Deserialization failed
            };

        // Get config based on size
        let result = match config_size {
            12 => {
                let config = ligerito::hardcoded_config_12_verifier();
                verify(&config, &proof)
            }
            16 => {
                let config = ligerito::hardcoded_config_16_verifier();
                verify(&config, &proof)
            }
            20 => {
                let config = ligerito::hardcoded_config_20_verifier();
                verify(&config, &proof)
            }
            24 => {
                let config = ligerito::hardcoded_config_24_verifier();
                verify(&config, &proof)
            }
            28 => {
                let config = ligerito::hardcoded_config_28_verifier();
                verify(&config, &proof)
            }
            30 => {
                let config = ligerito::hardcoded_config_30_verifier();
                verify(&config, &proof)
            }
            _ => return 0,  // Unsupported size
        };

        // Handle result
        match result {
            Ok(true) => 1,   // Valid
            Ok(false) => 0,  // Invalid
            Err(_) => 0,     // Error during verification
        }
    }
}
