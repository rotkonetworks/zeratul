## Ligerito GPU Parametrization Question (Tweet Thread)

**Tweet 1:**
Working on WebGPU acceleration for Ligerito sumcheck. Hit an interesting parametrization question for n=20 scale.

Current config: initial_dims=(2^14, 2^6), k=6, 148 queries
Works but seeing marginal GPU benefit (1.03x at n=24)

**Tweet 2:**
V1 GPU tried allocating 148 × 2^n arrays on GPU:
• n=16: 155 MB → exceeds WebGPU 128MB limit ❌
• n=20: 2.4 GB → impossible ❌

V2 hybrid: GPU computes contributions (2.4KB), CPU accumulates. Solves memory but not fast enough.

**Tweet 3:**
Current bottleneck: k=6 → only 64-element dot products (too small for GPU workgroups which prefer 256-1024)

Could we use larger k at same n=20 scale? e.g., k=10 → 1024-element dots, fewer rounds, better GPU occupancy?

**Tweet 4:**
Questions:
1. Does changing k (while keeping n=20) affect security/soundness?
2. How does k impact proof size?
3. Can we increase num_queries for more parallelism?
4. What's the valid range for k at n=20?

Is current config already optimal or room for GPU tuning?
