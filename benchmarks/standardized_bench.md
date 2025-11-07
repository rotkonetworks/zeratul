# standardized ligerito benchmarks

all benchmarks use identical parameters:
- polynomial size: 2^20 = 1,048,576 elements
- field: BinaryElem32 for coefficients, BinaryElem128 for commitments
- seed: 1234 (deterministic)
- measurement: single run with @elapsed (julia) or Instant::now (rust)
- julia benchmarks: include warmup run to exclude JIT compilation time

## test specifications

### polynomial generation
```
seed = 1234
poly[i] = BinaryElem32(i % 0xFFFFFFFF) for i in 0..(2^20-1)
```

### prover configuration
- use hardcoded_config_20 for all implementations
- measure only proving time (exclude setup/poly generation)

### verifier configuration  
- use hardcoded_config_20_verifier for all implementations
- measure only verification time (exclude proof parsing if separate)

### output format
```
proving: XXXms
verification: XXXms
```
