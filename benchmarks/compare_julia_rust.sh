#!/bin/bash
set -e

# run both julia and rust benchmarks with proper cpu tuning

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
cd "$SCRIPT_DIR/.."

echo "=== julia vs rust comparison (tuned benchmarks) ==="
echo "hardware: $(cat /proc/cpuinfo | grep 'model name' | head -1 | cut -d: -f2 | xargs)"
echo "cores: 8 (pinned to cores 1-8, core 0 reserved for system)"
echo "date: $(date +%Y-%m-%d)"
echo ""

# check sudo
if ! sudo -n true 2>/dev/null; then
    echo "error: need passwordless sudo"
    exit 1
fi

# save state
ORIGINAL_GOVERNOR=$(cat /sys/devices/system/cpu/cpu1/cpufreq/scaling_governor)
ORIGINAL_BOOST=$(cat /sys/devices/system/cpu/cpufreq/boost 2>/dev/null || echo "1")

cleanup() {
    echo ""
    echo "restoring cpu state..."
    [ -f /sys/devices/system/cpu/cpufreq/boost ] && echo "$ORIGINAL_BOOST" | sudo tee /sys/devices/system/cpu/cpufreq/boost >/dev/null
    for cpu in /sys/devices/system/cpu/cpu{1..8}/cpufreq/scaling_governor; do
        [ -f "$cpu" ] && echo "$ORIGINAL_GOVERNOR" | sudo tee "$cpu" >/dev/null
    done
}
trap cleanup EXIT INT TERM

# tune cpu
echo "tuning cpu..."
[ -f /sys/devices/system/cpu/cpufreq/boost ] && echo "0" | sudo tee /sys/devices/system/cpu/cpufreq/boost >/dev/null
for cpu in /sys/devices/system/cpu/cpu{1..8}/cpufreq/scaling_governor; do
    [ -f "$cpu" ] && echo "performance" | sudo tee "$cpu" >/dev/null
done
sync
sleep 1
echo ""

export RAYON_NUM_THREADS=8

# rust benchmarks
echo "=== rust (zeratul) - 2^20 (10 runs) ==="
rust_20_prove=()
rust_20_verify=()
for i in {1..10}; do
    echo -n "  run $i/10... "
    output=$(taskset -c 1-8 cargo run --release --example bench_20 2>&1 | tail -3)
    ptime=$(echo "$output" | grep "^proving:" | awk '{print $2}' | tr -d 'ms')
    vtime=$(echo "$output" | grep "^verification:" | awk '{print $2}' | tr -d 'ms')
    echo "${ptime}ms / ${vtime}ms"
    rust_20_prove+=($ptime)
    rust_20_verify+=($vtime)
done
rust_20_p_min=$(printf '%s\n' "${rust_20_prove[@]}" | sort -n | head -1)
rust_20_v_min=$(printf '%s\n' "${rust_20_verify[@]}" | sort -n | head -1)
echo ""

echo "=== rust (zeratul) - 2^24 (10 runs) ==="
rust_24_prove=()
rust_24_verify=()
for i in {1..10}; do
    echo -n "  run $i/10... "
    output=$(taskset -c 1-8 cargo run --release --example bench_standardized_24 2>&1 | tail -5)
    ptime=$(echo "$output" | grep "^proving:" | awk '{print $2}' | tr -d 'ms')
    vtime=$(echo "$output" | grep "^verification:" | awk '{print $2}' | tr -d 'ms')
    echo "${ptime}ms / ${vtime}ms"
    rust_24_prove+=($ptime)
    rust_24_verify+=($vtime)
done
rust_24_p_min=$(printf '%s\n' "${rust_24_prove[@]}" | sort -n | head -1)
rust_24_v_min=$(printf '%s\n' "${rust_24_verify[@]}" | sort -n | head -1)
echo ""

# julia benchmarks
echo "=== julia (ligerito.jl) - 2^20 (10 runs) ==="
cd benchmarks/Ligerito.jl
julia_20_output=$(taskset -c 1-8 julia --threads=8 --project=. -e '
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
prove_times = Float64[]
verify_times = Float64[]
for i in 1:10
    t_prove = @elapsed proof = prover(config, poly)
    t_verify = @elapsed result = verifier(verifier_cfg, proof)
    push!(prove_times, t_prove * 1000)
    push!(verify_times, t_verify * 1000)
    println("  run $i/10... $(round(t_prove * 1000, digits=2))ms / $(round(t_verify * 1000, digits=2))ms")
end
println("MIN: $(round(minimum(prove_times), digits=2))ms / $(round(minimum(verify_times), digits=2))ms")
' 2>&1)

echo "$julia_20_output" | grep "  run"
julia_20_p_min=$(echo "$julia_20_output" | grep "^MIN:" | awk '{print $2}' | tr -d 'ms/')
julia_20_v_min=$(echo "$julia_20_output" | grep "^MIN:" | awk '{print $4}' | tr -d 'ms')
echo ""

echo "=== julia (ligerito.jl) - 2^24 (10 runs) ==="
julia_24_output=$(taskset -c 1-8 julia --threads=8 --project=. -e '
using BinaryFields, Ligerito
config = Ligerito.hardcoded_config_24(BinaryElem32, BinaryElem128)
poly = [BinaryElem32(i % UInt32(0xFFFFFFFF)) for i in 0:(2^24-1)]
verifier_cfg = Ligerito.hardcoded_config_24_verifier()

# warmup
for _ in 1:3
    proof = prover(config, poly)
    result = verifier(verifier_cfg, proof)
end

# timed runs
prove_times = Float64[]
verify_times = Float64[]
for i in 1:10
    t_prove = @elapsed proof = prover(config, poly)
    t_verify = @elapsed result = verifier(verifier_cfg, proof)
    push!(prove_times, t_prove * 1000)
    push!(verify_times, t_verify * 1000)
    println("  run $i/10... $(round(t_prove * 1000, digits=2))ms / $(round(t_verify * 1000, digits=2))ms")
end
println("MIN: $(round(minimum(prove_times), digits=2))ms / $(round(minimum(verify_times), digits=2))ms")
' 2>&1)

echo "$julia_24_output" | grep "  run"
julia_24_p_min=$(echo "$julia_24_output" | grep "^MIN:" | awk '{print $2}' | tr -d 'ms/')
julia_24_v_min=$(echo "$julia_24_output" | grep "^MIN:" | awk '{print $4}' | tr -d 'ms')
echo ""

cd ../..

# calculate slowdown ratios
ratio_20_p=$(echo "scale=2; $rust_20_p_min / $julia_20_p_min" | bc)
ratio_20_v=$(echo "scale=2; $rust_20_v_min / $julia_20_v_min" | bc)
ratio_24_p=$(echo "scale=2; $rust_24_p_min / $julia_24_p_min" | bc)
ratio_24_v=$(echo "scale=2; $rust_24_v_min / $julia_24_v_min" | bc)

echo "=== final comparison (8 cores, tuned) ==="
echo ""
echo "2^20 (1,048,576 elements):"
echo "  julia:   proving ${julia_20_p_min}ms, verification ${julia_20_v_min}ms"
echo "  rust:    proving ${rust_20_p_min}ms, verification ${rust_20_v_min}ms"
echo "  ratio:   proving ${ratio_20_p}x, verification ${ratio_20_v}x"
echo ""
echo "2^24 (16,777,216 elements):"
echo "  julia:   proving ${julia_24_p_min}ms, verification ${julia_24_v_min}ms"
echo "  rust:    proving ${rust_24_p_min}ms, verification ${rust_24_v_min}ms"
echo "  ratio:   proving ${ratio_24_p}x, verification ${ratio_24_v}x"
echo ""
