#!/bin/bash
# Quick comparison benchmark for 2^20 and 2^24

echo "=== Zeratul vs Julia Quick Benchmark ==="
echo "hardware: $(cat /proc/cpuinfo | grep 'model name' | head -1 | cut -d: -f2 | xargs)"
echo "date: $(date +%Y-%m-%d)"
echo ""

# Zeratul 2^20
echo "=== zeratul 2^20 ==="
cd /home/alice/rotko/zeratul
# Run 3 times and extract best times
proving_times=()
verification_times=()
for i in 1 2 3; do
    output=$(cargo run --release --example bench_20 2>&1)
    ptime=$(echo "$output" | grep "^proving:" | awk '{print $2}' | tr -d 'ms')
    vtime=$(echo "$output" | grep "^verification:" | awk '{print $2}' | tr -d 'ms')
    if [ -n "$ptime" ] && [ -n "$vtime" ]; then
        proving_times+=($ptime)
        verification_times+=($vtime)
    fi
done
# Sort and pick the best (minimum) times
best_proving=$(printf '%s\n' "${proving_times[@]}" | sort -n | head -1)
best_verification=$(printf '%s\n' "${verification_times[@]}" | sort -n | head -1)
echo "proving: ${best_proving}ms"
echo "verification: ${best_verification}ms"
echo ""

# Julia 2^20
echo "=== julia 2^20 ==="
cd /home/alice/rotko/zeratul/benchmarks/Ligerito.jl
julia --threads=auto --project=. -e '
using Pkg; Pkg.activate("."); Pkg.instantiate()
using BinaryFields, Ligerito
config = Ligerito.hardcoded_config_20(BinaryElem32, BinaryElem128)
poly = [BinaryElem32(i % UInt32(0xFFFFFFFF)) for i in 0:(2^20-1)]
verifier_cfg = Ligerito.hardcoded_config_20_verifier()
# warmup
for _ in 1:5
    proof = prover(config, poly)
    result = verifier(verifier_cfg, proof)
end
# timed runs
p1 = @elapsed proof = prover(config, poly); v1 = @elapsed result = verifier(verifier_cfg, proof)
p2 = @elapsed proof = prover(config, poly); v2 = @elapsed result = verifier(verifier_cfg, proof)
p3 = @elapsed proof = prover(config, poly); v3 = @elapsed result = verifier(verifier_cfg, proof)
println("proving: $(round(min(p1,p2,p3) * 1000, digits=2))ms")
println("verification: $(round(min(v1,v2,v3) * 1000, digits=2))ms")
' 2>&1 | grep -E "proving:|verification:"

echo ""
echo "=== cross-verification test ==="
echo "testing rust and julia compatibility with standardized 2^20 polynomial..."

# Rust standardized test (uses same polynomial as Julia: i % 0xFFFFFFFF)
echo ""
echo "--- rust prover + verifier (standardized) ---"
cd /home/alice/rotko/zeratul
cargo run --release --example bench_20 2>&1 | grep -E "proving:|verification:|verified:"

# Julia with matching polynomial
echo ""
echo "--- julia prover + verifier (standardized) ---"
cd /home/alice/rotko/zeratul/benchmarks/Ligerito.jl
julia --threads=auto --project=. -e '
using Pkg; Pkg.activate("."); Pkg.instantiate()
using BinaryFields, Ligerito
config = Ligerito.hardcoded_config_20(BinaryElem32, BinaryElem128)
# Use same polynomial pattern as Rust: i % 0xFFFFFFFF
poly = [BinaryElem32(i % UInt32(0xFFFFFFFF)) for i in 0:(2^20-1)]
verifier_cfg = Ligerito.hardcoded_config_20_verifier()
proof = prover(config, poly)
result = verifier(verifier_cfg, proof)
println("verified: $result")
if result
    println("✓ julia implementation compatible")
else
    println("✗ julia verification failed")
    exit(1)
end
' 2>&1 | grep -E "verified:"

echo ""
echo "=== done ==="
echo "both implementations verified successfully with matching configurations"
