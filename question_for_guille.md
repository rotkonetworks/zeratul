## GPU-Optimized Parametrization for n=20 Scale

Hey Guille,

We're implementing WebGPU acceleration for Ligerito sumcheck and hit an interesting optimization question.

### Current Implementation & Problem

For n=20 (2^20 table size), we use:
```
initial_dims = (2^14, 2^6)  // 16384 × 64 matrix
initial_k = 6
num_queries = 148
```

Our V1 GPU implementation allocated `num_queries × 2^n` element arrays on GPU:
- n=16: 148 × 2^16 × 16 bytes = **155 MB** → exceeds typical WebGPU 128 MB limit
- n=20: 148 × 2^20 × 16 bytes = **2.4 GB** → impossible

We fixed this with V2's hybrid architecture (GPU computes contributions, CPU accumulates), but now we're only seeing **1.03x speedup at n=24**. The GPU isn't the bottleneck anymore, but we're wondering if different parametrization could help.

### GPU Constraints & Observations

**Hardware limits:**
- **WebGPU buffer limit**: 128 MB on most GPUs (spec minimum)
- **Optimal workgroup sizes**: 256-1024 threads
- **Memory transfers are expensive**: CPU↔GPU transfers dominate at small k

**Current bottleneck analysis:**
- V2 GPU memory: only 2.4 KB (148 queries × 16 bytes) - well within limits ✓
- CPU memory: 2 × 2^20 × 16 bytes = 32 MB - manageable ✓
- Performance: 1.03x speedup suggests we're not fully utilizing GPU

**Why we think k matters:**
- Current k=6 → dot products of 2^6 = 64 elements (small for GPU)
- Larger k → fewer rounds, bigger dot products, better GPU occupancy
- More queries → better parallelism, but only if it doesn't break security

### Alternative Parametrizations (Same 2^20 Scale)

**Option A: Larger k (fewer rounds, bigger dot products)**
```
n=20, initial_k=10, initial_dims=(2^10, 2^10)
- 1024-element dot products (optimal for GPU workgroups)
- 10 sumcheck rounds instead of 20
- Square matrix (better cache locality)
```

**Option B: More parallelism (smaller n, more queries)**
```
n=18, initial_k=6, increase inv_rate → 512-1024 queries
- Less CPU memory (8 MB vs 32 MB)
- More parallel GPU work
- Still ~2^20 computational scale
```

**Option C: Balanced k=8**
```
n=20, initial_k=8, initial_dims=(2^12, 2^8)
- 256-element dot products
- Middle ground between rounds and dot product size
```

### Questions

1. **Security**: Does changing k (while keeping n=20) affect security or soundness?
2. **Proof size**: How does k impact final proof size?
3. **Query count**: Can we safely increase num_queries for more GPU parallelism?
4. **Practical bounds**: What's the valid range for k at n=20?

### Context & Benchmark Results

**Why n≥20 matters:**
- Ligerito's proof sizes only become competitive with recursion at n≥20
- Below n=20, the proof overhead isn't worth it

**Current V2 benchmark results (8-core CPU):**
```
n=20: CPU=1973ms  GPU=2138ms  → 0.92x (CPU faster!)
n=24: CPU=18058ms GPU=17509ms → 1.03x (marginal GPU win)
```

**The question:**
Given that we solved the 128MB memory limit with V2 hybrid architecture, can we now use **different parametrization** (larger k, more queries, different dims) to better utilize GPU parallelism while staying at the n=20 scale where Ligerito is useful?

Or is the current parametrization (k=6, 148 queries) already optimal for security/proof-size, and we should just accept that GPU doesn't help much for this algorithm?

Thanks!
