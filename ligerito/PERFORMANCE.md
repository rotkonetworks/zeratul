# ligerito performance benchmarks

comprehensive criterion-based benchmarks comparing our rust implementation with paper results.

## benchmark environment

- **machine**: linux 6.17.4 x64v3
- **compiler**: rustc with release optimizations
- **parallelism**: rayon with default thread count
- **benchmark tool**: criterion.rs with 10-100 samples

## results summary

### polynomial size 2^20 (4 mib)

| metric | our implementation | paper (m1 mac, 8 threads) | ratio |
|--------|-------------------|---------------------------|-------|
| proving time | 348.75 ms | 80 ms | 4.4x slower |
| verification time | **145.65 ms** | not reported | - |
| proof size | 146.69 kib | 145 kib | ~1.01x |

### polynomial size 2^24 (64 mib)

| metric | our implementation | paper (m1 mac, 8 threads) | ratio |
|--------|-------------------|---------------------------|-------|
| proving time | 6.54 s | 1.3 s | 5.0x slower |
| verification time | **1.40 s** | not reported | - |
| proof size | 236.34 kib | 255 kib | 0.93x (smaller!) |

## detailed measurements

### proving benchmarks

```
proving/2^20            time:   [345.71 ms 348.75 ms 352.35 ms]
                        (95% confidence interval)
                        outliers: 2 of 10 measurements (20.00%)

proving/2^24            time:   [6.4769 s 6.5374 s 6.5990 s]
                        (95% confidence interval)
                        outliers: 0 of 10 measurements (0.00%)
```

### verification benchmarks

```
verification/2^20       time:   [140.34 ms 145.65 ms 151.60 ms]
                        (95% confidence interval)
                        outliers: 13 of 100 measurements (13.00%)

verification/2^24       time:   [1.3876 s 1.4004 s 1.4145 s]
                        (95% confidence interval)
                        outliers: 9 of 100 measurements (9.00%)
```

### proof sizes

```
2^20 proof size: 150208 bytes (146.69 KiB)
2^24 proof size: 242016 bytes (236.34 KiB)
```

## analysis

### current status vs goals

**target**: sub-100ms verification for 2^20
**current**: 145.65ms median verification
**gap**: ~46ms to target (31% reduction needed)

### performance gap with paper

our proving times are 4-5x slower than the paper's m1 mac results. potential factors:
1. **hardware differences**: m1 mac has specialized silicon and unified memory
2. **compiler optimizations**: paper may use different compiler flags
3. **field arithmetic**: our binary field implementation may have optimization opportunities
4. **reed-solomon encoding**: fft operations could be optimized further

### optimization opportunities

#### 1. tensorized dot product (✅ implemented)
- **status**: completed and committed
- **impact**: modest for k=6 (64 elements), critical for larger k
- **complexity**: reduced from o(2^k) to o(k × 2^(k-1))

#### 2. remaining bottlenecks
based on prior profiling, main verification bottleneck is:
- **basis polynomial accumulation**: 16k element vector operations
- **sumcheck main loop**: 148 iterations over 16k vectors

#### 3. proposed next steps
1. **parallelize basis accumulation**: use rayon for 16k vector adds
2. **simd optimizations**: vectorize binary field arithmetic
3. **memory layout**: improve cache locality for field operations
4. **reduce allocations**: pre-allocate buffers, use in-place operations

#### 4. verification-specific optimizations
- batch merkle path verifications
- optimize lagrange basis evaluation caching
- reduce transcript hashing overhead
- parallelize independent query verifications

## scaling characteristics

verification time appears to scale roughly linearly with polynomial size:
- 2^20 → 146ms
- 2^24 (16x larger) → 1.40s (9.6x slower)

this suggests o(n) or o(n log n) complexity, which is expected for:
- merkle path verifications: o(queries × log n)
- sumcheck rounds: o(log n)
- polynomial evaluations: o(queries × k)

## memory characteristics

based on paper table 2, memory usage for our sizes:
- 2^20: ~49 mib total allocated (actual usage likely much lower)
- 2^24: ~630 mib total allocated

our implementation should be similarly efficient given we follow the same protocol structure.

## running benchmarks

```bash
# run all benchmarks
cargo bench --bench ligerito_bench

# run only proving benchmarks
cargo bench --bench ligerito_bench proving

# run only verification benchmarks
cargo bench --bench ligerito_bench verification

# generate detailed report
cargo bench --bench ligerito_bench -- --verbose
```

## benchmark configuration

located in `benches/ligerito_bench.rs`:
- proving: 10 samples per benchmark
- verification: 100 samples per benchmark
- warmup: 3 seconds per benchmark
- criterion default measurement time

## future work

### larger polynomial sizes
enable 2^28 and 2^30 benchmarks when sufficient memory is available:
- 2^28: requires ~9.8 gib ram
- 2^30: requires ~31 gib ram

### additional metrics
- measure memory high-water mark during proving/verification
- profile individual protocol steps (commit, sumcheck, merkle)
- measure proof compression ratios
- benchmark different field sizes

### comparison with other implementations
- compare with reference go implementation
- benchmark against other ligerito variants (raa codes, etc)
- cross-platform comparisons (x86, arm, wasm)

---

*last updated: 2025-11-07*
*baseline: post-tensorization optimization*
