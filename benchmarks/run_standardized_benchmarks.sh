#!/bin/bash
# run standardized benchmarks on all implementations

set -e

echo "========================================"
echo "standardized ligerito benchmarks"
echo "========================================"
echo "hardware: $(cat /proc/cpuinfo | grep 'model name' | head -1 | cut -d: -f2 | xargs)"
echo "cores: $(nproc)"
echo "date: $(date +%Y-%m-%d)"
echo ""

# benchmark 1: zeratul (our rust implementation)
echo "1. benchmarking zeratul..."
cd "$(dirname "$0")/.."
cargo build --release --example bench_standardized 2>&1 | tail -1
cargo run --release --example bench_standardized 2>&1
echo ""

# benchmark 2: ligerito.jl (julia)
echo "2. benchmarking ligerito.jl..."
cd benchmarks/Ligerito.jl
julia --threads=auto --project=. ../bench_julia.jl 2>&1
echo ""

# benchmark 3: ashutosh-ligerito (rust)
echo "3. benchmarking ashutosh-ligerito..."
cd ../ashutosh-ligerito
# copy our standardized benchmark into their src/
cp ../bench_ashutosh.rs src/bench_standardized.rs
# add it to their Cargo.toml if not already there
if ! grep -q "bench_standardized" Cargo.toml; then
    cat >> Cargo.toml << 'CARGO_EOF'

[[bin]]
name = "bench_standardized"
path = "src/bench_standardized.rs"
CARGO_EOF
fi
cargo build --release --bin bench_standardized 2>&1 | tail -1
cargo run --release --bin bench_standardized 2>&1
echo ""

echo "========================================"
echo "all benchmarks complete"
echo "========================================"
