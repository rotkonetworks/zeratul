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
using BinaryFields, Ligerito
config = Ligerito.hardcoded_config_20(BinaryElem32, BinaryElem128)
poly = [BinaryElem32(i % UInt32(0xFFFFFFFF)) for i in 0:(2^20-1)]
verifier_cfg = Ligerito.hardcoded_config_20_verifier()
# multiple warmup runs to fully compile all paths
for _ in 1:5
    proof = prover(config, poly)
    result = verifier(verifier_cfg, proof)
end
# actual timed run - take best of 3 to account for variability
p1 = @elapsed proof = prover(config, poly); v1 = @elapsed result = verifier(verifier_cfg, proof)
p2 = @elapsed proof = prover(config, poly); v2 = @elapsed result = verifier(verifier_cfg, proof)
p3 = @elapsed proof = prover(config, poly); v3 = @elapsed result = verifier(verifier_cfg, proof)
println("proving time: $(round(min(p1,p2,p3) * 1000, digits=2))ms")
println("verification time: $(round(min(v1,v2,v3) * 1000, digits=2))ms")
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
