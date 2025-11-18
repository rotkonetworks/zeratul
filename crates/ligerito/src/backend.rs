//! Backend abstraction for CPU/GPU acceleration
//!
//! This module provides a unified interface for compute operations,
//! with automatic backend selection and graceful fallback.
//!
//! # Current Backends
//!
//! - **CPU**: Uses SIMD when `hardware-accel` is enabled, works everywhere
//! - **GPU**: WebGPU-based acceleration (experimental), falls back to CPU on failure
//!
//! # Future: RAA Backend for WASM
//!
//! A future optimization could add a third backend for WASM targets that uses
//! Repeat-Accumulate-Accumulate (RAA) codes instead of Reed-Solomon for the
//! first round (G₁):
//!
//! ```ignore
//! pub trait Backend {
//!     // Current methods (FFT-based RS encoding)
//!     fn encode_cols<F>(...) -> Result<()>;
//!
//!     // Future method for RAA encoding (WASM-optimized)
//!     fn encode_cols_raa<F>(...) -> Result<()>;
//! }
//! ```
//!
//! The RAA backend would avoid field multiplications entirely, providing 5-10x
//! speedup in WASM at the cost of larger proofs (1060 queries vs 148 for round 1).

use binary_fields::BinaryFieldElement;
use crate::reed_solomon::ReedSolomon;
use std::sync::{Arc, Mutex};

#[cfg(feature = "webgpu")]
use crate::gpu::{GpuDevice, fft::GpuFft};

/// Compute backend selection strategy
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BackendHint {
    /// Automatically select best backend (default)
    Auto,
    /// Force CPU backend
    Cpu,
    /// Prefer GPU, fallback to CPU if unavailable
    Gpu,
}

impl BackendHint {
    /// Parse from environment variable LIGERITO_BACKEND
    pub fn from_env() -> Self {
        match std::env::var("LIGERITO_BACKEND").as_deref() {
            Ok("cpu") | Ok("CPU") => BackendHint::Cpu,
            Ok("gpu") | Ok("GPU") => BackendHint::Gpu,
            Ok("auto") | Ok("AUTO") | Ok(_) => BackendHint::Auto,
            Err(_) => BackendHint::Auto,
        }
    }
}

/// Abstraction over CPU and GPU compute backends
pub trait Backend: Send + Sync {
    /// Perform in-place FFT on data
    fn fft_inplace<F>(&self, data: &mut [F], twiddles: &[F], parallel: bool) -> crate::Result<()>
    where
        F: BinaryFieldElement + Send + Sync + bytemuck::Pod + 'static;

    /// Encode columns of a polynomial matrix using Reed-Solomon
    fn encode_cols<F>(&self, poly_mat: &mut Vec<Vec<F>>, rs: &ReedSolomon<F>, parallel: bool) -> crate::Result<()>
    where
        F: BinaryFieldElement + Send + Sync + bytemuck::Pod + 'static;

    /// Get backend name for logging
    fn name(&self) -> &'static str;
}

/// CPU-only backend (always available)
pub struct CpuBackend;

impl Backend for CpuBackend {
    fn fft_inplace<F>(&self, data: &mut [F], twiddles: &[F], parallel: bool) -> crate::Result<()>
    where
        F: BinaryFieldElement + Send + Sync + bytemuck::Pod + 'static,
    {
        crate::reed_solomon::fft(data, twiddles, parallel);
        Ok(())
    }

    fn encode_cols<F>(&self, poly_mat: &mut Vec<Vec<F>>, rs: &ReedSolomon<F>, parallel: bool) -> crate::Result<()>
    where
        F: BinaryFieldElement + Send + Sync + bytemuck::Pod + 'static,
    {
        crate::ligero::encode_cols(poly_mat, rs, parallel);
        Ok(())
    }

    fn name(&self) -> &'static str {
        "CPU"
    }
}

/// GPU-accelerated backend (WebGPU)
#[cfg(feature = "webgpu")]
pub struct GpuBackend {
    fft: Arc<Mutex<GpuFft>>,
    /// Fallback to CPU if GPU operations fail
    cpu_fallback: CpuBackend,
    /// Track if GPU is currently working (disable after failures)
    enabled: Arc<Mutex<bool>>,
}

#[cfg(feature = "webgpu")]
impl GpuBackend {
    /// Try to initialize GPU backend
    pub fn new() -> crate::Result<Self> {
        // Use pollster to block on async GPU initialization
        let device = pollster::block_on(GpuDevice::new())
            .map_err(|e| crate::LigeritoError::GpuInitFailed(e.to_string()))?;

        let fft = GpuFft::new(device);

        Ok(Self {
            fft: Arc::new(Mutex::new(fft)),
            cpu_fallback: CpuBackend,
            enabled: Arc::new(Mutex::new(true)),
        })
    }

    fn is_enabled(&self) -> bool {
        *self.enabled.lock().unwrap()
    }

    fn disable(&self) {
        *self.enabled.lock().unwrap() = false;
    }
}

#[cfg(feature = "webgpu")]
impl Backend for GpuBackend {
    fn fft_inplace<F>(&self, data: &mut [F], twiddles: &[F], parallel: bool) -> crate::Result<()>
    where
        F: BinaryFieldElement + Send + Sync + bytemuck::Pod + 'static,
    {
        if !self.is_enabled() {
            return self.cpu_fallback.fft_inplace(data, twiddles, parallel);
        }

        // Try GPU FFT
        let mut fft = self.fft.lock().unwrap();
        match pollster::block_on(fft.fft_inplace(data)) {
            Ok(_) => Ok(()),
            Err(e) => {
                eprintln!("GPU FFT failed: {}. Falling back to CPU.", e);
                self.disable();
                drop(fft); // Release lock before fallback
                self.cpu_fallback.fft_inplace(data, twiddles, parallel)
            }
        }
    }

    fn encode_cols<F>(&self, poly_mat: &mut Vec<Vec<F>>, rs: &ReedSolomon<F>, parallel: bool) -> crate::Result<()>
    where
        F: BinaryFieldElement + Send + Sync + bytemuck::Pod + 'static,
    {
        // For now, GPU encode_cols uses CPU implementation
        // TODO: Implement GPU version that processes all columns in parallel
        self.cpu_fallback.encode_cols(poly_mat, rs, parallel)
    }

    fn name(&self) -> &'static str {
        if self.is_enabled() {
            "GPU (WebGPU)"
        } else {
            "GPU (disabled, using CPU)"
        }
    }
}

/// Backend implementation enum (avoids dyn trait object issues)
pub enum BackendImpl {
    Cpu(CpuBackend),
    #[cfg(feature = "webgpu")]
    Gpu(GpuBackend),
}

impl Backend for BackendImpl {
    fn fft_inplace<F>(&self, data: &mut [F], twiddles: &[F], parallel: bool) -> crate::Result<()>
    where
        F: BinaryFieldElement + Send + Sync + bytemuck::Pod + 'static,
    {
        match self {
            BackendImpl::Cpu(cpu) => cpu.fft_inplace(data, twiddles, parallel),
            #[cfg(feature = "webgpu")]
            BackendImpl::Gpu(gpu) => gpu.fft_inplace(data, twiddles, parallel),
        }
    }

    fn encode_cols<F>(&self, poly_mat: &mut Vec<Vec<F>>, rs: &ReedSolomon<F>, parallel: bool) -> crate::Result<()>
    where
        F: BinaryFieldElement + Send + Sync + bytemuck::Pod + 'static,
    {
        match self {
            BackendImpl::Cpu(cpu) => cpu.encode_cols(poly_mat, rs, parallel),
            #[cfg(feature = "webgpu")]
            BackendImpl::Gpu(gpu) => gpu.encode_cols(poly_mat, rs, parallel),
        }
    }

    fn name(&self) -> &'static str {
        match self {
            BackendImpl::Cpu(cpu) => cpu.name(),
            #[cfg(feature = "webgpu")]
            BackendImpl::Gpu(gpu) => gpu.name(),
        }
    }
}

/// Global backend selector with lazy initialization
pub struct BackendSelector {
    backend: BackendImpl,
    hint: BackendHint,
}

impl BackendSelector {
    /// Create new backend selector with given hint
    pub fn new(hint: BackendHint) -> Self {
        let backend = Self::select_backend(hint);

        Self {
            backend,
            hint,
        }
    }

    /// Create with auto-detection (respects LIGERITO_BACKEND env var)
    pub fn auto() -> Self {
        Self::new(BackendHint::from_env())
    }

    /// Get the selected backend
    pub fn backend(&self) -> &BackendImpl {
        &self.backend
    }

    fn select_backend(hint: BackendHint) -> BackendImpl {
        match hint {
            BackendHint::Cpu => {
                BackendImpl::Cpu(CpuBackend)
            }
            BackendHint::Gpu => {
                #[cfg(feature = "webgpu")]
                {
                    match GpuBackend::new() {
                        Ok(gpu) => {
                            #[cfg(not(target_arch = "wasm32"))]
                            eprintln!("GPU initialized successfully");
                            return BackendImpl::Gpu(gpu);
                        }
                        Err(e) => {
                            eprintln!("GPU initialization failed: {:?}. Falling back to CPU.", e);
                        }
                    }
                }

                #[cfg(not(feature = "webgpu"))]
                {
                    eprintln!("GPU requested but not compiled. Use --features webgpu. Falling back to CPU.");
                }

                BackendImpl::Cpu(CpuBackend)
            }
            BackendHint::Auto => {
                // Auto selection logic: use GPU for n >= 20 if available
                #[cfg(feature = "webgpu")]
                {
                    match GpuBackend::new() {
                        Ok(gpu) => {
                            #[cfg(not(target_arch = "wasm32"))]
                            eprintln!("GPU detected and initialized");
                            return BackendImpl::Gpu(gpu);
                        }
                        Err(_) => {
                            // Silently fall back to CPU in auto mode
                        }
                    }
                }

                BackendImpl::Cpu(CpuBackend)
            }
        }
    }
}

impl Default for BackendSelector {
    fn default() -> Self {
        Self::auto()
    }
}

impl Clone for BackendSelector {
    fn clone(&self) -> Self {
        // Create new selector with same hint
        Self::new(self.hint)
    }
}
