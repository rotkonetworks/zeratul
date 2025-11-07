# comparison with reference julia implementation

analyzing optimizations in bcc-research/ligerito-impl to identify gaps in our rust implementation.

## key optimizations found in reference

### 1. threading/parallelism

**julia impl**:
- uses `Threads.@spawn` for parallel evaluation scaling (scale_evals_inplace!)
- parallel fft with `fft_twiddles_parallel!`
- adaptive thread depth based on `nthreads()`

**our impl**:
- ✅ parallel row hashing
- ✅ parallel merkle tree construction (large layers)
- ✅ parallel reed-solomon encoding (encode_cols)
- ❌ missing: parallel fft in reed-solomon
- ❌ missing: parallel basis evaluation scaling

**action**: add parallel fft to reed-solomon crate

### 2. simd optimizations

**julia impl** (BinaryFields/src/binaryfield.jl:1):
```julia
# Using SIMD for fast reinterpret. Julia's base `reinterpret` is insanely slow
# in this particular case likely due to (in this case unnecessary) type safety
# checks
```

**our impl**:
- uses standard rust field arithmetic
- no explicit simd

**action**: investigate rust simd for field operations (packed_simd, std::simd)

### 3. generated functions for field arithmetic

**julia impl** (binaryfield.jl:73):
```julia
@generated function compute_tmp(hi::T) where T <: BinaryPoly
    # generates specialized code for each field size at compile time
```

julia's `@generated` functions create specialized code per type at compile time.

**our impl**:
- generic trait implementations
- compile-time monomorphization via generics

**status**: rust's zero-cost abstractions should provide similar benefits through monomorphization.

### 4. in-place operations

**julia impl**:
- extensive use of in-place operations (`scale_evals_inplace!`, `fft_twiddles!`)
- `@views` macro for non-copying slices

**our impl**:
- some in-place operations (encode_in_place)
- could add more

**action**: audit for unnecessary allocations, add more in-place variants

### 5. sumcheck structure

**julia impl**:
- stateful `SumcheckProverInstance` with basis_polys vector
- glue operation with alpha scaling
- eval_01x_product for efficient evaluation

**our impl**:
- functional approach with standalone functions
- similar logic but different structure

**status**: both approaches valid, theirs may have slight cache advantages

## performance gap analysis

### our results vs paper

| size | metric | ours | paper (m1) | gap |
|------|--------|------|------------|-----|
| 2^20 | proving | 342ms | 80ms | 4.3x |
| 2^20 | verification | 131ms | - | goal: <100ms |
| 2^24 | proving | 6.38s | 1.3s | 4.9x |

### likely causes of gap

1. **hardware**: m1 mac has specialized silicon, unified memory architecture
2. **julia jit**: julia's llvm-based jit may generate more optimized simd code
3. **field arithmetic**: julia's generated functions + simd for gf(2^n) ops
4. **fft**: missing parallel fft in our reed-solomon

## optimization priorities

### high impact (implement next)

1. **parallel fft in reed-solomon**
   - julia has `fft_twiddles_parallel!`
   - should significantly speed up prover (~2-3x for encode_cols)

2. **simd field arithmetic**
   - julia explicitly mentions using simd
   - rust: use `std::simd` or `packed_simd`
   - target: binary field mul/add operations

3. **reduce allocations**
   - add more `_inplace` variants
   - use `Vec::with_capacity` everywhere
   - profile allocation hot paths

### medium impact

4. **optimize sumcheck loop**
   - current bottleneck: 148 iterations × 16k ops
   - consider stateful prover instance like julia

5. **cache more intermediate values**
   - we cache some basis evaluations
   - could cache more lagrange evaluations

### low impact (polish)

6. **benchmark on m1 mac**
   - direct comparison with paper
   - isolate hardware vs software gaps

7. **profile-guided optimization**
   - use `cargo pgo` for hot path optimization

## wasm/polkavm constraints

remember: verifier must run single-threaded for wasm/polkavm deployment.

**safe to parallelize**:
- prover operations (all encoding, hashing, merkle)
- fft in prover path

**must stay single-threaded**:
- verifier path (already done ✅)
- field arithmetic (or use wasm simd)

## next steps

1. implement parallel fft in reed-solomon crate
2. investigate rust simd for field operations
3. profile allocations and add in-place variants
4. re-benchmark after each optimization
5. document performance gains

---

*comparison based on bcc-research/ligerito-impl @ latest commit*
*our impl: ligerito @ commit 721d851*
