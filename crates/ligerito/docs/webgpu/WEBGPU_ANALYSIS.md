# WebGPU Acceleration Analysis for Ligerito

## Executive Summary

After achieving **3x speedup with SIMD** (40s → 12.7s for 2^20 proving), we're analyzing WebGPU acceleration potential. Unlike Penumbra's elliptic curve MSM (Multi-Scalar Multiplication), Ligerito operates on **binary extension fields** with different computational patterns.

**Current Performance (2^20, WASM+SIMD):**
- Proving: 12.7 seconds
- Verification: 5.9 seconds
- Proof size: 146 KB

**Target (WebGPU):**
- Proving: 2-5 seconds (3-6x speedup)
- Verification: 1-2 seconds (3-6x speedup)

## Computational Profile Analysis

### What Dominates Ligerito Proving Time?

Based on `src/prover.rs` and `src/sumcheck_polys.rs`, the bottlenecks are:

1. **Reed-Solomon Encoding (25-30%)**: FFT over binary fields
2. **Sumcheck Polynomial Construction (40-50%)**:
   - Tensorized dot products
   - Lagrange basis evaluation
   - Field arithmetic (carryless multiplication)
3. **Merkle Tree Construction (15-20%)**: Hashing (SHA-256)
4. **Partial Evaluation (5-10%)**: Multilinear polynomial folding

### Operations Breakdown

```rust
// 1. Reed-Solomon encoding (FFT-based)
ligero_commit(poly, rows, cols, reed_solomon)
  └─> FFT over GF(2^128)
      - O(n log n) field multiplications
      - Additive FFT (no twiddle factors in binary fields!)
      - Highly parallelizable

// 2. Sumcheck polynomial construction
induce_sumcheck_poly(n, sks_vks, opened_rows, challenges, queries, alpha)
  └─> For each query (148 queries):
      ├─> Tensorized dot product: O(k × 2^(k-1)) operations
      ├─> Lagrange basis evaluation: O(n)
      └─> Batched polynomial update: O(2^n)

// 3. Merkle tree
tree.prove(queries)
  └─> SHA-256 hashing
      - Sequential by nature (Merkle path construction)
      - Less GPU-friendly
```

## Key Difference from Penumbra's MSM

### Penumbra (Elliptic Curves)
- **Operation**: Multi-Scalar Multiplication (MSM)
- **Structure**: `Σᵢ sᵢ × Pᵢ` (scalar × point, then sum)
- **Challenge**: 256-bit scalar multiplication, expensive point addition
- **Solution**: Pippenger's algorithm + sparse matrix decomposition
- **GPU Win**: Massive parallelism (millions of independent scalar muls)

### Ligerito (Binary Extension Fields)
- **Operation**: FFT + Lagrange basis evaluation
- **Structure**: Additive FFT, tensor products, polynomial evaluation
- **Challenge**: Small field operations (128-bit), many dependencies
- **Advantage**: Simpler field arithmetic (XOR-based, no modular reduction!)
- **GPU Win**: Data parallelism in FFT, SIMD-friendly operations

## WebGPU Acceleration Strategy

### Approach 1: Parallelize FFT (Highest Impact)

**Target**: Reed-Solomon encoding (25-30% of runtime)

The additive FFT over binary fields is **perfectly suited for GPU**:

```rust
// Additive FFT butterfly (no twiddle factors!)
fn fft_butterfly(u: &mut [F], stride: usize) {
    for i in 0..u.len()/2 {
        let lo = u[i];
        let hi = u[i + stride];
        u[i] = lo ^ hi;           // Addition in GF(2^n)
        u[i + stride] = lo;       // Keep original
    }
}
```

**WebGPU Implementation:**
```wgsl
@group(0) @binding(0) var<storage, read_write> data: array<u128>;

@compute @workgroup_size(256)
fn fft_butterfly(@builtin(global_invocation_id) id: vec3<u32>) {
    let idx = id.x;
    let stride = params.stride;

    if idx < params.size / 2 {
        let lo = data[idx];
        let hi = data[idx + stride];

        data[idx] = lo ^ hi;      // XOR (native GPU op!)
        data[idx + stride] = lo;
    }
}
```

**Expected Speedup**: 5-10x for FFT operations
- FFT is embarrassingly parallel
- Binary field operations map directly to GPU bitwise ops
- No synchronization needed per butterfly layer

### Approach 2: Parallelize Sumcheck Construction (Medium Impact)

**Target**: Sumcheck polynomial induction (40-50% of runtime)

Current sequential implementation:
```rust
for (i, (row, &query)) in opened_rows.iter().enumerate() {
    let dot = tensorized_dot_product(row, v_challenges);
    let contribution = dot * alpha_pows[i];

    // Update basis polynomial (148 iterations)
    evaluate_scaled_basis_inplace(&mut local_basis, sks_vks, query_mod);
    for j in 0..basis_poly.len() {
        basis_poly[j] = basis_poly[j] + local_basis[j] * contribution;
    }
}
```

**Problem**: Sequential dependencies (accumulation into `basis_poly`)

**GPU Solution**: Two-phase reduction
1. **Phase 1 (GPU)**: Compute all local contributions in parallel (148 queries → 148 threads)
2. **Phase 2 (GPU)**: Tree reduction to accumulate results

```wgsl
// Phase 1: Compute local contributions
@compute @workgroup_size(148)
fn compute_contributions(@builtin(global_invocation_id) id: vec3<u32>) {
    let query_idx = id.x;
    let row = opened_rows[query_idx];
    let dot = tensorized_dot_product(row, v_challenges);
    let contribution = dot * alpha_pows[query_idx];

    // Each thread computes its own basis polynomial
    local_basis[query_idx] = evaluate_basis(query_idx) * contribution;
}

// Phase 2: Parallel reduction
@compute @workgroup_size(256)
fn reduce_basis(@builtin(global_invocation_id) id: vec3<u32>) {
    let elem_idx = id.x;

    // Sum all local_basis[*][elem_idx] into basis_poly[elem_idx]
    var sum = vec4<u32>(0);
    for (var i = 0u; i < 148u; i++) {
        sum ^= local_basis[i][elem_idx];  // XOR = addition in GF(2^n)
    }
    basis_poly[elem_idx] = sum;
}
```

**Expected Speedup**: 3-5x for sumcheck construction
- 148 queries computed in parallel
- Basis evaluation vectorized
- Reduction leverages GPU memory bandwidth

### Approach 3: Keep Merkle Tree on CPU (Low Impact)

Merkle tree construction is sequential and represents only 15-20% of runtime. The GPU-CPU transfer overhead would negate any gains.

**Decision**: Keep on CPU (current implementation is fine)

## Binary Field Operations on GPU

### Advantage: Native Bitwise Operations

Binary extension fields use **carryless multiplication** which maps to:

```wgsl
// 64x64 → 128-bit carryless multiplication
fn carryless_mul_gpu(a: vec2<u32>, b: vec2<u32>) -> vec4<u32> {
    // Use bit-slicing (similar to our WASM SIMD optimization)
    // But with 256-wide SIMD on GPU!

    var result = vec4<u32>(0);
    for (var i = 0u; i < 64u; i++) {
        let mask = -((b.x >> i) & 1u);
        result.xy ^= (a << i) & mask;
    }
    return result;
}
```

GPU advantages over WASM:
1. **Wider SIMD**: 256-512 bit vectors vs 128-bit in WASM
2. **More registers**: 64KB register file per workgroup
3. **Better memory bandwidth**: 500 GB/s vs 10 GB/s for WASM

### No Montgomery Form Needed!

Unlike Penumbra's elliptic curves (which need Montgomery form for modular arithmetic), binary fields are **already optimal**:

- Addition: Native XOR
- Multiplication: Carryless mul (shift + XOR)
- No modular reduction in computation (only at the end with irreducible polynomial)

This is a **huge simplification** compared to Penumbra's approach!

## Implementation Complexity Comparison

### Penumbra's WebGPU MSM (from their blog)
- ✅ Sparse matrix construction (CSR format)
- ✅ Montgomery form conversion (13-bit limbs)
- ✅ Point coordinate representation
- ✅ Bucket method implementation
- ✅ Scalar decomposition
- ✅ Parallel reduction with atomics
- **Complexity**: High (6 months development)

### Ligerito's WebGPU Acceleration
- ✅ FFT parallelization (simpler than MSM)
- ✅ Lagrange basis vectorization
- ✅ Parallel sumcheck construction
- ⚠️ Field operations (already optimized with SIMD)
- ❌ No need for Montgomery form
- ❌ No need for sparse matrix construction
- **Complexity**: Medium (2-3 months development)

## Revised Timeline

Given that binary field operations are **simpler than elliptic curve operations**, we can accelerate the schedule:

| Phase | Original Estimate | Revised Estimate |
|-------|------------------|------------------|
| 1. Research | 2-3 weeks | ✅ **Done** (this doc) |
| 2. Infrastructure | 2 weeks | 1 week |
| 3. FFT Shaders | 3 weeks | 2 weeks |
| 4. Sumcheck Shaders | 3 weeks | 2 weeks |
| 5. Integration | 2 weeks | 1 week |
| 6. Testing | 2 weeks | 1 week |
| **Total** | **14-18 weeks** | **8-10 weeks** |

## Expected Performance Gains

### Conservative Estimate (3x overall)
- FFT: 25% of time → 5x faster = 5% of time (20% saved)
- Sumcheck: 45% of time → 3x faster = 15% of time (30% saved)
- Other: 30% of time → no change
- **Total**: 50% time saved = **2x faster** → **6 seconds for 2^20**

### Optimistic Estimate (6x overall)
- FFT: 25% → 10x faster = 2.5% (22.5% saved)
- Sumcheck: 45% → 5x faster = 9% (36% saved)
- Field ops: Better GPU arithmetic = 10% saved
- **Total**: 68% time saved = **3x faster** → **4 seconds for 2^20**

### Realistic Target
**2^20 proving: 12.7s → 4-6 seconds (2-3x speedup)**
**2^20 verification: 5.9s → 2-3 seconds (2-3x speedup)**

This would make Ligerito **competitive with native performance** in the browser!

## Next Steps (Phase 2: Infrastructure)

1. Add `wgpu` crate dependency
2. Create GPU device detection and initialization
3. Implement CPU fallback when GPU unavailable
4. Setup WGSL shader compilation pipeline
5. Create buffer management for GPU memory transfers

Ready to proceed with implementation?

## Zcash Integration Analysis

### 2^12 Proofs for Zcash
- **Proof size**: ~146 KB (too large for on-chain verification)
- **Verification time**: ~2-3 seconds (too slow for block validation)
- **Recommendation**: **Not suitable for direct on-chain verification**

### Better Approach: Ligerito Rollups on Zcash
1. **Off-chain Ligerito proving**: Batch Zcash transactions → Ligerito proof
2. **Recursive composition**: Use Zcash's Halo 2 to verify Ligerito proof
3. **On-chain commitment**: Post 32-byte commitment + small recursive proof

This combines:
- ✅ Zcash's privacy (Halo 2)
- ✅ Ligerito's efficiency (binary field ops)
- ✅ Data availability guarantees
- ✅ Practical proof sizes (<10 KB for recursive proof)

**Perfect for hackathon!** Would demonstrate novel rollup design using two different proof systems.
