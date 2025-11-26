//! Raw WASM exports - no wasm-bindgen, pure extern "C" ABI
//!
//! This module provides a minimal WASM interface using only the C ABI.
//! JavaScript must manually manage memory via the exported alloc/dealloc functions.
//!
//! ## Why skip wasm-bindgen?
//! - Full control over memory layout
//! - Works with multi-instance parallelism (each worker gets its own WASM instance)
//! - No SharedArrayBuffer requirements
//! - Simpler debugging
//! - Smaller binary size
//!
//! ## Memory Layout for Results
//! prove_raw returns a pointer to: [len: u32 (4 bytes)][proof_bytes: len bytes]
//! JS must read len first, then read len bytes of proof data.
//!
//! ## Random Number Generation
//! Uses a seeded RNG. Call `set_random_seed` from JS before proving to provide
//! entropy. If not called, uses a default seed (NOT cryptographically secure).

use core::alloc::Layout;
use std::alloc::{alloc, dealloc};
use std::slice;
use std::sync::atomic::{AtomicU64, Ordering};

// Global seed for random number generation
static RANDOM_SEED_LO: AtomicU64 = AtomicU64::new(0x0123456789abcdef);
static RANDOM_SEED_HI: AtomicU64 = AtomicU64::new(0xfedcba9876543210);

/// Set the random seed from JavaScript
/// Call this with 32 bytes of cryptographic randomness before proving
#[no_mangle]
pub extern "C" fn set_random_seed(seed_ptr: *const u8) {
    if seed_ptr.is_null() {
        return;
    }
    let seed_bytes = unsafe { slice::from_raw_parts(seed_ptr, 32) };

    // Split into two u64s (for a simple xorshift128+ seed)
    let lo = u64::from_le_bytes(seed_bytes[0..8].try_into().unwrap());
    let hi = u64::from_le_bytes(seed_bytes[8..16].try_into().unwrap());
    // Mix in more bytes
    let lo2 = u64::from_le_bytes(seed_bytes[16..24].try_into().unwrap());
    let hi2 = u64::from_le_bytes(seed_bytes[24..32].try_into().unwrap());

    RANDOM_SEED_LO.store(lo ^ lo2, Ordering::SeqCst);
    RANDOM_SEED_HI.store(hi ^ hi2, Ordering::SeqCst);
}

// Custom getrandom implementation using xorshift128+
use getrandom::register_custom_getrandom;

fn custom_getrandom(dest: &mut [u8]) -> Result<(), getrandom::Error> {
    // Simple xorshift128+ PRNG (NOT cryptographically secure without proper seeding!)
    let mut s0 = RANDOM_SEED_LO.load(Ordering::SeqCst);
    let mut s1 = RANDOM_SEED_HI.load(Ordering::SeqCst);

    for chunk in dest.chunks_mut(8) {
        // xorshift128+
        let mut t = s0;
        let s = s1;
        s0 = s;
        t ^= t << 23;
        t ^= t >> 18;
        t ^= s ^ (s >> 5);
        s1 = t;

        let result = s0.wrapping_add(s1);
        let bytes = result.to_le_bytes();
        let len = chunk.len().min(8);
        chunk[..len].copy_from_slice(&bytes[..len]);
    }

    RANDOM_SEED_LO.store(s0, Ordering::SeqCst);
    RANDOM_SEED_HI.store(s1, Ordering::SeqCst);

    Ok(())
}

register_custom_getrandom!(custom_getrandom);

use crate::{prover, verifier};
use crate::{hardcoded_config_12, hardcoded_config_20, hardcoded_config_24, hardcoded_config_28, hardcoded_config_30};
use crate::{hardcoded_config_12_verifier, hardcoded_config_20_verifier, hardcoded_config_24_verifier, hardcoded_config_28_verifier, hardcoded_config_30_verifier};
use binary_fields::{BinaryElem32, BinaryElem128};
use std::marker::PhantomData;

/// Allocate memory in WASM linear memory
/// Returns pointer to allocated region, or 0 on failure
#[no_mangle]
pub extern "C" fn wasm_alloc(size: u32) -> *mut u8 {
    if size == 0 {
        return core::ptr::null_mut();
    }
    let layout = match Layout::from_size_align(size as usize, 8) {
        Ok(l) => l,
        Err(_) => return core::ptr::null_mut(),
    };
    unsafe { alloc(layout) }
}

/// Free memory allocated by wasm_alloc
#[no_mangle]
pub extern "C" fn wasm_dealloc(ptr: *mut u8, size: u32) {
    if ptr.is_null() || size == 0 {
        return;
    }
    let layout = match Layout::from_size_align(size as usize, 8) {
        Ok(l) => l,
        Err(_) => return,
    };
    unsafe { dealloc(ptr, layout) }
}

/// Get expected polynomial size for a config
/// Returns 0 for invalid config_size
#[no_mangle]
pub extern "C" fn get_polynomial_size(config_size: u8) -> u32 {
    match config_size {
        12 | 20 | 24 | 28 | 30 => 1u32 << config_size,
        _ => 0,
    }
}

/// Generate a proof from polynomial data
///
/// # Arguments
/// - poly_ptr: pointer to polynomial data (u32 array in WASM memory)
/// - poly_len: number of u32 elements (NOT bytes)
/// - config_size: 12, 20, or 24
///
/// # Returns
/// Pointer to result struct: [status: u8][len: u32][data: bytes]
/// - status = 0: success, len = proof length, data = proof bytes
/// - status = 1: error, len = error message length, data = error string (UTF-8)
///
/// Caller must free the returned pointer with wasm_dealloc(ptr, 1 + 4 + len)
#[no_mangle]
pub extern "C" fn prove_raw(poly_ptr: *const u32, poly_len: u32, config_size: u8) -> *mut u8 {
    // Build result helper
    fn build_result(status: u8, data: &[u8]) -> *mut u8 {
        let total_size = 1 + 4 + data.len(); // status + len + data
        let ptr = unsafe {
            let layout = Layout::from_size_align(total_size, 8).unwrap();
            alloc(layout)
        };
        if ptr.is_null() {
            return ptr;
        }
        unsafe {
            *ptr = status;
            let len_bytes = (data.len() as u32).to_le_bytes();
            core::ptr::copy_nonoverlapping(len_bytes.as_ptr(), ptr.add(1), 4);
            core::ptr::copy_nonoverlapping(data.as_ptr(), ptr.add(5), data.len());
        }
        ptr
    }

    fn build_error(msg: &str) -> *mut u8 {
        build_result(1, msg.as_bytes())
    }

    // Validate input
    if poly_ptr.is_null() {
        return build_error("null polynomial pointer");
    }

    let expected_len = get_polynomial_size(config_size);
    if expected_len == 0 {
        return build_error("invalid config_size (use 12, 20, 24, 28, or 30)");
    }
    if poly_len != expected_len {
        return build_error("polynomial length mismatch");
    }

    // Read polynomial from WASM memory
    let poly_slice = unsafe { slice::from_raw_parts(poly_ptr, poly_len as usize) };
    let poly: Vec<BinaryElem32> = poly_slice.iter().map(|&x| BinaryElem32::from(x)).collect();

    // Generate proof
    let proof_result = match config_size {
        12 => {
            let config = hardcoded_config_12(PhantomData::<BinaryElem32>, PhantomData::<BinaryElem128>);
            prover::<BinaryElem32, BinaryElem128>(&config, &poly)
        }
        20 => {
            let config = hardcoded_config_20(PhantomData::<BinaryElem32>, PhantomData::<BinaryElem128>);
            prover::<BinaryElem32, BinaryElem128>(&config, &poly)
        }
        24 => {
            let config = hardcoded_config_24(PhantomData::<BinaryElem32>, PhantomData::<BinaryElem128>);
            prover::<BinaryElem32, BinaryElem128>(&config, &poly)
        }
        28 => {
            let config = hardcoded_config_28(PhantomData::<BinaryElem32>, PhantomData::<BinaryElem128>);
            prover::<BinaryElem32, BinaryElem128>(&config, &poly)
        }
        30 => {
            let config = hardcoded_config_30(PhantomData::<BinaryElem32>, PhantomData::<BinaryElem128>);
            prover::<BinaryElem32, BinaryElem128>(&config, &poly)
        }
        _ => unreachable!(),
    };

    match proof_result {
        Ok(proof) => {
            // Serialize proof
            match bincode::serialize(&proof) {
                Ok(bytes) => build_result(0, &bytes),
                Err(e) => build_error(&format!("serialization failed: {}", e)),
            }
        }
        Err(e) => build_error(&format!("proving failed: {}", e)),
    }
}

/// Verify a proof
///
/// # Arguments
/// - proof_ptr: pointer to proof bytes in WASM memory
/// - proof_len: length of proof in bytes
/// - config_size: 12, 20, 24, 28, or 30
///
/// # Returns
/// - 0: proof is INVALID
/// - 1: proof is VALID
/// - 2: error (invalid input or verification error)
#[no_mangle]
pub extern "C" fn verify_raw(proof_ptr: *const u8, proof_len: u32, config_size: u8) -> u8 {
    if proof_ptr.is_null() || proof_len == 0 {
        return 2; // error
    }

    let proof_bytes = unsafe { slice::from_raw_parts(proof_ptr, proof_len as usize) };

    // Deserialize proof
    let proof: crate::FinalizedLigeritoProof<BinaryElem32, BinaryElem128> =
        match bincode::deserialize(proof_bytes) {
            Ok(p) => p,
            Err(_) => return 2, // error
        };

    // Verify
    let result = match config_size {
        12 => {
            let config = hardcoded_config_12_verifier();
            verifier::<BinaryElem32, BinaryElem128>(&config, &proof)
        }
        20 => {
            let config = hardcoded_config_20_verifier();
            verifier::<BinaryElem32, BinaryElem128>(&config, &proof)
        }
        24 => {
            let config = hardcoded_config_24_verifier();
            verifier::<BinaryElem32, BinaryElem128>(&config, &proof)
        }
        28 => {
            let config = hardcoded_config_28_verifier();
            verifier::<BinaryElem32, BinaryElem128>(&config, &proof)
        }
        30 => {
            let config = hardcoded_config_30_verifier();
            verifier::<BinaryElem32, BinaryElem128>(&config, &proof)
        }
        _ => return 2, // error
    };

    match result {
        Ok(true) => 1,  // valid
        Ok(false) => 0, // invalid
        Err(_) => 2,    // error
    }
}

/// Get result status from prove_raw result pointer
#[no_mangle]
pub extern "C" fn result_status(ptr: *const u8) -> u8 {
    if ptr.is_null() {
        return 2;
    }
    unsafe { *ptr }
}

/// Get result data length from prove_raw result pointer
#[no_mangle]
pub extern "C" fn result_len(ptr: *const u8) -> u32 {
    if ptr.is_null() {
        return 0;
    }
    unsafe {
        let bytes = [*ptr.add(1), *ptr.add(2), *ptr.add(3), *ptr.add(4)];
        u32::from_le_bytes(bytes)
    }
}

/// Get pointer to result data from prove_raw result pointer
#[no_mangle]
pub extern "C" fn result_data_ptr(ptr: *const u8) -> *const u8 {
    if ptr.is_null() {
        return core::ptr::null();
    }
    unsafe { ptr.add(5) }
}

/// Free a result from prove_raw
#[no_mangle]
pub extern "C" fn result_free(ptr: *mut u8) {
    if ptr.is_null() {
        return;
    }
    let len = result_len(ptr);
    let total_size = 1 + 4 + len as usize;
    wasm_dealloc(ptr, total_size as u32);
}
