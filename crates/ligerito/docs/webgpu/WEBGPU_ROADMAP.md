# WebGPU Acceleration Roadmap for Ligerito

Based on Penumbra Labs' ZPrize-winning implementation, this document outlines a plan to implement WebGPU acceleration for Ligerito proof generation.

## Current State

**WASM Performance (2^20 polynomial):**
- Proving time: ~40 seconds
- Verification time: ~23 seconds
- Binary size: 339 KB
- Bottleneck: CPU-bound computation with 4-5x WASM overhead

## WebGPU Performance Potential

**Expected improvements based on Penumbra's results:**
- 10-100x speedup for large circuits (2^16 to 2^20)
- Better scaling for larger problem sizes
- Parallel execution across GPU cores

## Implementation Plan

### Phase 1: Research & Foundation (2-3 weeks)

**Study Penumbra's approach:**
- [ ] Analyze cuZK paper (Lu et al, 2022) sparse matrix construction
- [ ] Study Pippenger's Bucket Method optimization
- [ ] Understand sparse matrix transposition for GPU
- [ ] Research Montgomery form representation for field operations

**Key concepts to implement:**
1. **Sparse Matrix Construction**: Transform MSM operations into sparse matrix operations
2. **CSR Format**: Compressed Sparse Row for memory-efficient storage
3. **Point Conversion**: Convert to Montgomery form with optimal limb sizes
4. **Scalar Decomposition**: Break scalars into chunks for parallel processing

### Phase 2: WebGPU Infrastructure (2 weeks)

**Setup WebGPU environment:**
- [ ] Add `wgpu` crate dependency
- [ ] Create WGSL shader structure
- [ ] Implement buffer management for GPU memory
- [ ] Create workgroup size calculator

**Browser detection:**
```typescript
// Detect WebGPU support
const adapter = await navigator.gpu?.requestAdapter();
if (adapter) {
  // Use WebGPU accelerated path
  initWebGPU();
} else {
  // Fallback to WASM CPU path (current implementation)
  initWASM();
}
```

### Phase 3: Core Algorithms (4-6 weeks)

#### 3.1 Point Conversion & Scalar Decomposition

**Montgomery Form Conversion:**
- [ ] Implement 13-bit limb representation (optimal for 32-bit GPU)
- [ ] Convert field elements to Montgomery form
- [ ] Implement Barrett Reduction for big-integer operations

**Scalar Decomposition:**
- [ ] Split 251-bit scalars into 16-bit chunks
- [ ] Implement signed bucket index technique
- [ ] Optimize for K = b/c chunks per scalar

#### 3.2 Sparse Matrix Operations

**CSR Matrix Generation:**
- [ ] Direct CSR generation (skip ELL intermediate step)
- [ ] Parallel transpose using atomic operations
- [ ] Implement sparse matrix vector multiplication (SPMV)

**Structure:**
```rust
struct CSRMatrix {
    row_ptr: Vec<u32>,      // Row pointers
    col_idx: Vec<u32>,      // Column indices
    values: Vec<FieldElem>, // Non-zero values
}
```

#### 3.3 Bucket Reduction

**Parallel Bucket Accumulation:**
- [ ] Implement half-pyramid reduction (cuZK Algorithm 4)
- [ ] Split buckets across threads
- [ ] Compute running sums per thread
- [ ] Multiply by bucket indices

#### 3.4 Result Aggregation

**Horner's Method:**
- [ ] Extract reduced bucket sums from GPU
- [ ] Compute final result on CPU (faster for small point sets)
- [ ] Minimize GPU-CPU data transfers

### Phase 4: WGSL Shaders (3-4 weeks)

**Required shaders:**

1. **Point Conversion Shader**: Convert to Montgomery form
2. **Scalar Decomposition Shader**: Break into chunks
3. **Matrix Transpose Shader**: Parallel CSR transpose
4. **Bucket Accumulation Shader**: Sparse matrix multiplication
5. **Bucket Reduction Shader**: Half-pyramid reduction

**Example shader structure:**
```wgsl
@group(0) @binding(0) var<storage, read> points: array<Point>;
@group(0) @binding(1) var<storage, read> scalars: array<Scalar>;
@group(0) @binding(2) var<storage, read_write> buckets: array<Bucket>;

@compute @workgroup_size(256)
fn bucket_accumulation(@builtin(global_invocation_id) id: vec3<u32>) {
    let bucket_id = id.x;
    // Accumulate points into bucket
    // ...
}
```

### Phase 5: Integration & Optimization (2-3 weeks)

**Browser integration:**
- [ ] Create fallback mechanism (WebGPU → WASM)
- [ ] Implement feature detection
- [ ] Add progress reporting for long computations
- [ ] Profile and optimize memory transfers

**Optimization targets:**
- Minimize GPU-CPU data transfers
- Optimize workgroup sizes per GPU architecture
- Tune limb sizes for different devices
- Implement shader caching

### Phase 6: Testing & Benchmarking (1-2 weeks)

**Test coverage:**
- [ ] Correctness tests against WASM implementation
- [ ] Performance benchmarks (2^12, 2^16, 2^20, 2^24)
- [ ] Cross-platform testing (Chrome, Firefox, Safari)
- [ ] Various GPU architectures (Apple M-series, NVIDIA, AMD)

**Performance targets:**
- 2^12: <1 second
- 2^16: <5 seconds
- 2^20: <10 seconds (vs 40s in WASM)
- 2^24: <60 seconds

## Technical Challenges

### 1. Field Arithmetic in WebGPU

**Challenge**: WebGPU only supports 32-bit unsigned integers
**Solution**: Use 13-bit limbs (20 limbs for 253-bit field)

```rust
// Montgomery multiplication with 13-bit limbs
struct BigInt {
    limbs: [u32; 20], // 20 x 13-bit limbs = 260 bits
}
```

### 2. Memory Management

**Challenge**: GPU memory is limited and transfers are expensive
**Solution**:
- Keep data on GPU between shader passes
- Only transfer final reduced results to CPU
- Use CSR format for sparse matrices (memory efficient)

### 3. Atomic Operations

**Challenge**: Race conditions in parallel accumulation
**Solution**: Use WebGPU atomic operations for bucket updates

```wgsl
atomicAdd(&bucket.x, point.x);
atomicAdd(&bucket.y, point.y);
```

### 4. Shader Compilation Time

**Challenge**: WGSL compilation adds overhead
**Solution**:
- Cache compiled shaders
- Precompile during initialization
- Use pipeline cache API when available

## Architecture

```
┌─────────────────────────────────────────────────┐
│                  Ligerito Prover                │
└───────────┬─────────────────────────────────────┘
            │
            ├─ Feature Detection
            │
      ┌─────┴─────┐
      │           │
┌─────▼─────┐   ┌─▼────────────┐
│  WebGPU   │   │    WASM      │
│  (Fast)   │   │  (Fallback)  │
└───────────┘   └──────────────┘
      │               │
      │               └─ Sequential computation
      │                  40s for 2^20
      │
      ├─ Point Conversion (GPU)
      ├─ Scalar Decomposition (GPU)
      ├─ Matrix Transpose (GPU)
      ├─ Bucket Accumulation (GPU)
      ├─ Bucket Reduction (GPU)
      └─ Result Aggregation (CPU)
           ~5-10s for 2^20
```

## Dependencies

```toml
[dependencies]
# Existing
wasm-bindgen = "0.2"
binary-fields = { path = "../binary-fields" }

# New for WebGPU
wgpu = { version = "0.20", features = ["webgpu"] }
pollster = "0.3"  # For async handling
bytemuck = "1.14" # For buffer casting
```

## Milestones & Timeline

**Total estimated time: 14-18 weeks (3.5-4.5 months)**

| Phase | Duration | Deliverable |
|-------|----------|-------------|
| 1. Research | 2-3 weeks | Technical design doc |
| 2. Infrastructure | 2 weeks | WebGPU setup & detection |
| 3. Core Algorithms | 4-6 weeks | Rust implementation |
| 4. WGSL Shaders | 3-4 weeks | GPU shaders |
| 5. Integration | 2-3 weeks | Browser integration |
| 6. Testing | 1-2 weeks | Benchmarks & validation |

## References

1. **Penumbra Labs ZPrize**: https://penumbra.zone/blog/gpu-msm
2. **cuZK Paper** (Lu et al, 2022): Sparse matrix MSM design
3. **WebGPU Spec**: https://www.w3.org/TR/webgpu/
4. **WGSL Spec**: https://www.w3.org/TR/WGSL/
5. **Pippenger's Algorithm**: Bucket method for MSM
6. **Montgomery Multiplication**: Efficient field arithmetic

## Future Enhancements

- **Multi-device proving**: P2P workload sharing across devices
- **Floating-point arithmetic**: Using FMA instructions
- **Dynamic workgroup sizes**: When WebGPU supports it
- **TPU support**: For Google Cloud deployment
- **Subgroup operations**: When available in WebGPU

## Current Recommendation

**For production use today**: Use the current WASM implementation
- Battle-tested and production-ready
- 40s proving time is acceptable for many use cases
- Works on all browsers without GPU requirements

**For future optimization**: Implement WebGPU
- Significant speedup potential (10-100x)
- Better scaling for larger circuits
- Future-proof for client-side proving

The WASM implementation serves as an excellent fallback while WebGPU support matures.
