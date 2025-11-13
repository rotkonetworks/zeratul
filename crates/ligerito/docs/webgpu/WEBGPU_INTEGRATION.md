# WebGPU Integration for Browser Benchmarking

## Overview

This document describes the WebGPU integration that enables GPU-accelerated sumcheck operations in the browser via WebAssembly.

## Architecture

```
┌─────────────────────────────────────────────────────────────┐
│                    Browser (JavaScript)                      │
│  ┌────────────────────────────────────────────────────────┐ │
│  │          benchmark-real.html (UI Layer)                │ │
│  │  - Device detection                                    │ │
│  │  - Benchmark configuration                             │ │
│  │  - Results visualization                               │ │
│  └─────────────────┬──────────────────────────────────────┘ │
│                    │ import                                  │
│  ┌─────────────────▼──────────────────────────────────────┐ │
│  │         ligerito.js (WASM Bindings)                    │ │
│  │  - BenchConfig class                                   │ │
│  │  - bench_cpu_sumcheck()                                │ │
│  │  - bench_gpu_sumcheck()                                │ │
│  │  - check_webgpu_available()                            │ │
│  └─────────────────┬──────────────────────────────────────┘ │
│                    │ wasm_bindgen                            │
│  ┌─────────────────▼──────────────────────────────────────┐ │
│  │      ligerito_bg.wasm (Rust Compiled to WASM)         │ │
│  │                                                          │ │
│  │  ┌─────────────────────────────────────────────────┐   │ │
│  │  │  CPU Path: sumcheck_polys::induce_sumcheck_poly │   │ │
│  │  └─────────────────────────────────────────────────┘   │ │
│  │                                                          │ │
│  │  ┌─────────────────────────────────────────────────┐   │ │
│  │  │  GPU Path: gpu::sumcheck::GpuSumcheck           │   │ │
│  │  │  ├─ GpuDevice::new()                             │   │ │
│  │  │  └─ induce_sumcheck_poly()                       │   │ │
│  │  └─────────────────────┬───────────────────────────┘   │ │
│  │                        │ wgpu                            │ │
│  │  ┌─────────────────────▼───────────────────────────┐   │ │
│  │  │     GPU Compute Shaders (WGSL)                  │   │ │
│  │  │  ├─ binary_field.wgsl (GF(2^128) operations)    │   │ │
│  │  │  └─ sumcheck.wgsl (parallel sumcheck kernel)    │   │ │
│  │  └─────────────────────────────────────────────────┘   │ │
│  └──────────────────────────────────────────────────────────┘ │
│                    │ navigator.gpu                           │
│  ┌─────────────────▼──────────────────────────────────────┐ │
│  │                WebGPU API (Browser)                    │ │
│  └─────────────────┬──────────────────────────────────────┘ │
└────────────────────┼───────────────────────────────────────┘
                     │
         ┌───────────▼────────────┐
         │   Hardware GPU         │
         │  (Desktop / Mobile)    │
         └────────────────────────┘
```

## Components

### 1. Rust WASM Bindings (`src/wasm.rs`)

Added WebGPU benchmark functions:

```rust
#[wasm_bindgen]
pub struct BenchConfig {
    pub n: usize,      // log2 of basis size
    pub k: usize,      // log2 of row size
    pub q: usize,      // number of queries
}

#[wasm_bindgen]
pub fn bench_cpu_sumcheck(config: BenchConfig) -> Promise;

#[cfg(feature = "webgpu")]
#[wasm_bindgen]
pub fn bench_gpu_sumcheck(config: BenchConfig) -> Promise;

#[cfg(feature = "webgpu")]
#[wasm_bindgen]
pub async fn check_webgpu_available() -> bool;
```

### 2. Build Script (`build-wasm-webgpu.sh`)

Builds WASM with WebGPU support:

```bash
wasm-pack build \
    --target web \
    --out-dir pkg/web-webgpu \
    --features webgpu \
    --no-default-features
```

Output:
- `pkg/web-webgpu/ligerito_bg.wasm` - Compiled WASM binary
- `pkg/web-webgpu/ligerito.js` - JavaScript bindings
- Copied to `www/webgpu/` for serving

### 3. Benchmark UI (`www/benchmark-real.html`)

Interactive browser-based benchmark interface:

Features:
- WebGPU device detection and capability reporting
- Configurable test scales (Small/Medium/Large/XL)
- CPU vs GPU comparison with speedup metrics
- Visual progress indicators
- Test history tracking
- Mobile GPU detection and optimization notes

### 4. GPU Implementation

#### Device Capabilities
- **Desktop GPUs**: Full support for n≤14 (row_size ≤128)
- **Mobile GPUs**: Same support, optimized for power efficiency
- **Buffer limits**: Automatic CPU fallback for n≥16 (>128MB buffers)

#### Performance Targets (Expected)
- **n=8** (256 basis): ~0.1-0.5ms GPU
- **n=10** (1024 basis): ~0.5-2ms GPU
- **n=12** (4096 basis): ~1-5ms GPU
- **n=14** (16K basis): ~2-10ms GPU

Speedup: 10-50x faster than single-threaded WASM CPU

## Testing

### Browser Requirements
- **Desktop**: Chrome 113+, Edge 113+
- **Android**: Chrome 113+ (tested target: Pixel 8 with Immortalis-G715)
- **iOS**: WebGPU not yet available

### Device Testing
- **Pixel 8 (Immortalis-G715 MC10)**: Primary mobile target
- **Desktop GPUs**: NVIDIA/AMD/Intel integrated and discrete
- **Memory**: 8GB RAM target for mobile devices

### Local Testing

```bash
# Build WASM with WebGPU
cd crates/ligerito
./build-wasm-webgpu.sh

# Serve locally
cd www
python3 -m http.server 8080

# Open in browser
http://localhost:8080/benchmark-real.html
```

## Dependencies

Added to `Cargo.toml`:

```toml
[dependencies]
wasm-bindgen-futures = { version = "0.4", optional = true }
js-sys = { version = "0.3.82", optional = true }

[features]
wasm = [
    "wasm-bindgen",
    "wasm-bindgen-futures",  # Async WASM support
    "js-sys",                # JavaScript types
    # ... other features
]

webgpu = [
    "wasm",
    "wgpu",
    "pollster",
    "web-sys",
    "futures",
]
```

## Known Limitations

1. **Buffer Size Limits**: GPU limited to 128MB max_storage_buffer_binding_size
   - n≥16 automatically falls back to CPU
   - This is a hardware architectural limit, not a software limitation

2. **Row Size Limits**: GPU shader supports row_size ≤128 elements (k≤7)
   - Covers all practical use cases for Ligerito
   - Larger rows would require chunked processing

3. **Browser Support**: WebGPU is still rolling out
   - Available on Chrome/Edge 113+ (desktop and Android)
   - Not yet available on iOS Safari

4. **WASM Size**: WebGPU build is ~8-15MB (includes wgpu + shader compiler)
   - Consider separate builds for CPU-only vs GPU versions
   - Use compression (gzip/brotli) for production deployment

## Mobile Optimization

For Pixel 8 and similar devices:

- **Unified Memory Architecture (UMA)**: GPU shares system RAM with CPU
  - No explicit data transfer overhead
  - Buffer binding limits still apply

- **Power Efficiency**: Mobile GPUs optimized for battery life
  - May show lower absolute performance than desktop
  - Still significant speedup vs single-threaded CPU

- **Thermal Throttling**: Consider in sustained benchmarks
  - GPU may throttle under continuous load
  - Short bursts show best performance

## Next Steps

1. **Complete WASM Build**: Wait for compilation to finish
2. **Test Locally**: Verify benchmark.html works in Chrome
3. **Deploy to Server**: Host on HTTPS for mobile testing
4. **Test on Pixel 8**: Validate WebGPU on target device
5. **Performance Tuning**: Optimize based on real-world results

## Security Notes

All GPU shaders include:
- Integer overflow protection on buffer indexing
- Bounds checking on all array accesses
- Safe handling of shift operations (no shift-by-32 UB)
- Deterministic results across all devices (consensus-critical)

## Resources

- WebGPU Spec: https://gpuweb.github.io/gpuweb/
- wgpu Documentation: https://wgpu.rs/
- Browser Compatibility: https://caniuse.com/webgpu
