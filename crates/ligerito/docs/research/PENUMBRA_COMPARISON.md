# Penumbra vs Ligerito WebGPU Implementation Comparison

## Overview

After analyzing Penumbra's WebGPU MSM implementation, here's how we can adapt their patterns for Ligerito's binary field operations.

## Architecture Comparison

### Penumbra (Elliptic Curve MSM)

**Repository**: `https://github.com/penumbra-zone/webgpu`

**Structure**:
```
src/reference/webgpu/
├── wgsl/
│   ├── U256.ts          # 256-bit integer ops (modular arithmetic)
│   ├── FieldModulus.ts  # Field modular reduction
│   └── Curve.ts         # Elliptic curve point operations
├── entries/
│   ├── pippengerMSMEntry.ts  # Pippenger's bucket method
│   └── naiveMSMEntry.ts      # Naive MSM implementation
└── utils.ts             # Buffer management, conversions
```

**Key Operations**:
1. **u256 arithmetic**: Add, subtract, multiply with carry propagation
2. **Modular reduction**: Montgomery multiplication, Barrett reduction
3. **Elliptic curve ops**: Point addition, doubling (30+ GPU instructions each)
4. **MSM via Pippenger**: Break scalars into 16-bit chunks, bucket method

### Ligerito (Binary Field Operations)

**Our Structure**:
```
src/gpu/
├── mod.rs            # Module exports
├── device.rs         # GPU initialization ✅
├── fft.rs            # Additive FFT acceleration
├── sumcheck.rs       # Parallel sumcheck construction
├── buffers.rs        # Buffer management ✅
└── shaders.rs        # WGSL shader source
```

**Key Operations**:
1. **u128 arithmetic**: XOR (native GPU operation!)
2. **No modular reduction**: Binary fields are closed under XOR
3. **FFT butterfly**: 2 XOR operations (vs 30+ for elliptic curves!)
4. **Sumcheck**: Parallel query processing, reduction

## Critical Insight: Simplicity Advantage

### Penumbra's Complexity

```wgsl
// Penumbra: u256 modular addition (30+ instructions)
fn field_add(a: u256, b: u256, modulus: u256) -> u256 {
    var sum = u256_add(a, b);          // 8 x u32 adds with carries
    if (gte(sum, modulus)) {           // 8 x u32 comparisons
        sum = u256_sub(sum, modulus);  // 8 x u32 subs with borrows
    }
    return sum;
}

// Penumbra: Elliptic curve point addition (100+ instructions!)
fn curve_add(P: Point, Q: Point) -> Point {
    // Jacobian coordinates
    // ~100 field multiplications
    // ~50 field additions
    // ~30 conditional branches
    ...
}
```

### Ligerito's Simplicity

```wgsl
// Ligerito: GF(2^128) addition (1 instruction!)
fn field_add(a: vec4<u32>, b: vec4<u32>) -> vec4<u32> {
    return a ^ b;  // XOR = addition in binary fields!
}

// Ligerito: GF(2^128) multiplication (simpler)
fn field_mul(a: vec4<u32>, b: vec4<u32>) -> vec4<u32> {
    // Carryless multiplication (shift + XOR)
    // ~64 XOR operations (vs 256 for modular mul)
    // No carries, no modular reduction!
    ...
}

// Ligerito: FFT butterfly (2 XORs!)
fn fft_butterfly(lo: vec4<u32>, hi: vec4<u32>) -> vec2<vec4<u32>> {
    return vec2(lo ^ hi, lo);  // That's it!
}
```

## Adaptation Strategy

### What to Copy from Penumbra

1. **Device initialization pattern** ✅ Already implemented
   - Request adapter with high performance preference
   - Create device with compute pipeline
   - Query capabilities (max buffer size, workgroup size)

2. **Buffer management pattern**
   - Storage buffers for input/output
   - Staging buffers for CPU-GPU transfers
   - Uniform buffers for parameters

3. **Pipeline architecture**
   - Compute shaders with workgroup_size optimization
   - Bind group layouts for buffers
   - Command encoding and submission

4. **TypeScript/JavaScript integration** (for web demo)
   - `navigator.gpu.requestAdapter()`
   - Buffer allocation and mapping
   - Shader compilation and caching

### What to Simplify for Binary Fields

1. **No u256 module needed**: We use u128 (vec4<u32>)
2. **No modular reduction**: Binary fields don't need it
3. **No Montgomery form**: Already optimal representation
4. **Simpler shaders**: XOR-based operations are native

## Implementation Plan (8-10 Weeks)

### Phase 1: Field Operations in WGSL (Week 1-2)

Create `src/gpu/shaders/binary_field.wgsl`:

```wgsl
// GF(2^128) represented as vec4<u32> (4 x 32-bit components)
struct BinaryField128 {
    data: vec4<u32>
}

// Addition (XOR)
fn gf128_add(a: vec4<u32>, b: vec4<u32>) -> vec4<u32> {
    return a ^ b;
}

// Carryless multiplication (adapted from our WASM SIMD)
fn gf128_mul(a: vec4<u32>, b: vec4<u32>) -> vec4<u32> {
    // Use Karatsuba or bit-slicing
    // ~64 XOR operations total
    ...
}
```

**Benefit over Penumbra**: 10x simpler than their field ops!

### Phase 2: FFT Parallelization (Week 3-4)

Create `src/gpu/shaders/fft.wgsl`:

```wgsl
@group(0) @binding(0) var<storage, read_write> data: array<vec4<u32>>;
@group(0) @binding(1) var<uniform> params: FFTParams;

@compute @workgroup_size(256)
fn fft_butterfly(@builtin(global_invocation_id) id: vec3<u32>) {
    let idx = id.x;
    let stride = params.stride;

    if (idx >= params.size / 2u) { return; }

    let i = idx * stride * 2u;
    let j = i + stride;

    let lo = data[i];
    let hi = data[j];

    // Additive FFT butterfly (no twiddle factors!)
    data[i] = lo ^ hi;  // XOR = add in GF(2^n)
    data[j] = lo;
}
```

**Expected speedup**: 5-10x for FFT operations

### Phase 3: Sumcheck Parallelization (Week 5-6)

Create `src/gpu/shaders/sumcheck.wgsl`:

```wgsl
// Phase 1: Compute contributions in parallel (148 queries → 148 threads)
@compute @workgroup_size(148)
fn compute_contributions(@builtin(global_invocation_id) id: vec3<u32>) {
    let query_idx = id.x;

    // Compute tensorized dot product
    var dot = tensorized_dot_product(query_idx);

    // Multiply by alpha^i
    let contribution = gf128_mul(dot, alpha_pows[query_idx]);

    // Compute local basis polynomial
    for (var i = 0u; i < basis_size; i++) {
        local_basis[query_idx * basis_size + i] =
            gf128_mul(sks_vks[i], contribution);
    }
}

// Phase 2: Reduce local basis polynomials (parallel reduction)
@compute @workgroup_size(256)
fn reduce_basis(@builtin(global_invocation_id) id: vec3<u32>) {
    let elem_idx = id.x;
    var sum = vec4<u32>(0u);

    for (var q = 0u; q < 148u; q++) {
        sum ^= local_basis[q * basis_size + elem_idx];
    }

    basis_poly[elem_idx] = sum;
}
```

**Expected speedup**: 3-5x for sumcheck construction

### Phase 4: Rust Integration (Week 7-8)

Implement in `src/gpu/fft.rs`:

```rust
impl GpuFft {
    pub async fn fft_inplace<F: BinaryFieldElement>(&self, data: &mut [F])
        -> Result<(), String>
    {
        // 1. Create storage buffer
        let buffer = self.create_buffer(data);

        // 2. Run log2(n) butterfly passes
        for pass in 0..data.len().trailing_zeros() {
            self.run_butterfly_shader(pass, &buffer)?;
        }

        // 3. Download result
        self.read_buffer(&buffer, data).await?;

        Ok(())
    }
}
```

### Phase 5: WASM Fallback Integration (Week 9)

```rust
// In prover.rs
pub async fn ligero_commit_gpu<F>(poly: &[F], rows: usize, cols: usize)
    -> Result<LigeritoCommitment>
{
    // Try WebGPU first
    if let Some(gpu) = GpuDevice::try_new().await {
        match gpu_fft(&gpu, poly).await {
            Ok(result) => return Ok(result),
            Err(e) => {
                #[cfg(target_arch = "wasm32")]
                web_sys::console::warn_1(&format!("GPU failed, falling back to CPU: {}", e).into());
            }
        }
    }

    // Fallback to CPU (current WASM implementation)
    ligero_commit_cpu(poly, rows, cols)
}
```

### Phase 6: Testing & Benchmarking (Week 10)

- Correctness tests: GPU results == CPU results
- Performance benchmarks on different GPUs
- Browser compatibility testing
- Graceful degradation testing

## Performance Projections

### Current (WASM + SIMD)
- **2^20 proving**: 12.7 seconds
- **2^20 verification**: 5.9 seconds

### With WebGPU (Conservative)
- **2^20 proving**: 4-6 seconds (2-3x faster)
- **2^20 verification**: 2-3 seconds (2-3x faster)

### With WebGPU (Optimistic)
- **2^20 proving**: 2-4 seconds (3-6x faster)
- **2^20 verification**: 1-2 seconds (3-6x faster)

## Key Takeaways

1. **Binary fields are MUCH simpler than elliptic curves for GPU**
   - XOR vs modular arithmetic
   - No Montgomery form needed
   - Simpler shaders (2 instructions vs 100+)

2. **Penumbra's architecture is solid, but we can simplify**
   - Copy: Device init, buffer management, pipeline patterns
   - Simplify: Field ops, no u256 module, native XOR operations

3. **Timeline is shorter than Penumbra's**
   - Their implementation: ~6 months (ZPrize 2023)
   - Our implementation: 8-10 weeks (simpler operations)

4. **Already ahead with SIMD**
   - Current 12.7s performance is excellent
   - WebGPU would be incremental improvement (not mandatory)

## Next Steps

1. ✅ Study Penumbra's implementation (DONE)
2. Implement binary field operations in WGSL
3. Port FFT butterfly to GPU
4. Implement parallel sumcheck
5. Integrate with WASM fallback
6. Test and benchmark

Ready to start implementing!
