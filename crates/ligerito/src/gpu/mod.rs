//! WebGPU acceleration for Ligerito polynomial commitment scheme
//!
//! This module provides GPU-accelerated implementations of core Ligerito operations:
//! - Additive FFT over binary extension fields
//! - Parallel sumcheck polynomial construction
//! - Lagrange basis evaluation
//!
//! The implementation automatically falls back to CPU (WASM) when WebGPU is unavailable.

#[cfg(feature = "webgpu")]
pub mod device;

#[cfg(feature = "webgpu")]
pub mod fft;

#[cfg(feature = "webgpu")]
pub mod sumcheck;

#[cfg(feature = "webgpu")]
pub mod buffers;

#[cfg(feature = "webgpu")]
pub mod shaders;

#[cfg(feature = "webgpu")]
pub use device::{GpuDevice, GpuCapabilities};

/// Check if WebGPU is available in the current environment
#[cfg(feature = "webgpu")]
pub async fn is_webgpu_available() -> bool {
    device::is_available().await
}

/// Stub for when WebGPU feature is disabled
#[cfg(not(feature = "webgpu"))]
pub async fn is_webgpu_available() -> bool {
    false
}
