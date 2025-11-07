# standardized benchmark results

**date:** 2025-11-07
**hardware:** amd ryzen 9 7945hx (16 cores / 32 threads)
**threads:** 32

## methodology

all implementations tested with identical parameters:
- polynomial size: 2^20 = 1,048,576 elements
- field: BinaryElem32 for coefficients, BinaryElem128 for commitments  
- polynomial: `poly[i] = i % 0xFFFFFFFF`
- transcript: sha256 (for compatibility)

see `standardized_bench.md` for full specification.

## results

| implementation | proving | verification |
|----------------|---------|--------------|
| **zeratul (ours)** | **184.31ms** | **130.71ms** |
| ligerito.jl (julia) | 3,271.79ms | 384.17ms |
| ashutosh-ligerito (rust) | 3,467.29ms | 269.00ms |

## analysis

- zeratul is **17.8x faster** than ligerito.jl at proving
- zeratul is **18.8x faster** than ashutosh-ligerito at proving
- zeratul is **2.9x faster** than ligerito.jl at verification
- zeratul is **2.1x faster** than ashutosh-ligerito at verification

## reproducing

```bash
git submodule update --init --recursive
./run_standardized_benchmarks.sh
```
