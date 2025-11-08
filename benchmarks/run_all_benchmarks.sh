#!/bin/bash
# benchmark all ligerito implementations on the same hardware

# get the directory where this script is located
SCRIPT_DIR="$( cd "$( dirname "${BASH_SOURCE[0]}" )" && pwd )"
PROJECT_ROOT="$(dirname "$SCRIPT_DIR")"

echo "==================================="
echo "ligerito implementation benchmarks"
echo "==================================="
echo "hardware: $(cat /proc/cpuinfo | grep 'model name' | head -1 | cut -d: -f2 | xargs)"
echo "threads: $(nproc)"
echo "ram: $(free -h | awk '/^Mem:/ {print $2}')"
echo "date: $(date +%Y-%m-%d)"
echo ""

# benchmark 1: our rust implementation
echo "=== 1. zeratul (our rust implementation) ==="
cd "$PROJECT_ROOT"
cargo bench --bench ligerito_bench 2>&1 | grep -E "proving/2\^20.*time:|verification/2\^20.*time:"
echo ""

# benchmark 2: ligerito.jl (julia)
echo "=== 2. ligerito.jl (julia reference) ==="
cd "$SCRIPT_DIR/Ligerito.jl"
julia --threads=auto --project=. -e '
using Pkg; Pkg.activate("."); Pkg.instantiate()
using BinaryFields, Ligerito, Random
Random.seed!(1234)
config = Ligerito.hardcoded_config_20(BinaryElem32, BinaryElem128)
poly = [BinaryElem32(i % UInt32(0xFFFFFFFF)) for i in 0:(2^20-1)]
# warmup run to trigger JIT compilation
proof = prover(config, poly)
verifier_cfg = Ligerito.hardcoded_config_20_verifier()
result = verifier(verifier_cfg, proof)
# actual timed run
prove_time = @elapsed proof = prover(config, poly)
verify_time = @elapsed result = verifier(verifier_cfg, proof)
println("proving time: $(round(prove_time * 1000, digits=2))ms")
println("verification time: $(round(verify_time * 1000, digits=2))ms")
' 2>&1 | grep -E "proving time:|verification time:"
echo ""

# benchmark 3: ashutosh-ligerito (rust reference)
echo "=== 3. ashutosh-ligerito (rust reference port) ==="
cd "$SCRIPT_DIR/ashutosh-ligerito"
cargo build --release 2>&1 | tail -1
cargo run --release --bin bench_standardized 2>&1 | grep -E "proving:|verification:"
echo ""

echo "==================================="
echo "benchmark complete"
echo "==================================="
