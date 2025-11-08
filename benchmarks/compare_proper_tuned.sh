#!/bin/bash
set -e

# proper julia vs rust comparison: SMT off, turbo off, 8 physical cores

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
cd "$SCRIPT_DIR/.."

echo "=== julia vs rust proper comparison ==="
echo "hardware: $(cat /proc/cpuinfo | grep 'model name' | head -1 | cut -d: -f2 | xargs)"
echo "config: 8 physical cores (0-7), SMT disabled, turbo disabled, performance governor"
echo "date: $(date +%Y-%m-%d)"
echo ""

# check sudo
if ! sudo -n true 2>/dev/null; then
    echo "error: need passwordless sudo"
    exit 1
fi

# save original state
ORIGINAL_GOVERNOR=$(cat /sys/devices/system/cpu/cpu0/cpufreq/scaling_governor)
ORIGINAL_BOOST=$(cat /sys/devices/system/cpu/cpufreq/boost 2>/dev/null || echo "1")
ORIGINAL_SMT=$(cat /sys/devices/system/cpu/smt/control)

cleanup() {
    echo ""
    echo "=== restoring original state ==="
    echo "$ORIGINAL_SMT" | sudo tee /sys/devices/system/cpu/smt/control >/dev/null || true
    [ -f /sys/devices/system/cpu/cpufreq/boost ] && echo "$ORIGINAL_BOOST" | sudo tee /sys/devices/system/cpu/cpufreq/boost >/dev/null
    for cpu in /sys/devices/system/cpu/cpu{0..15}/cpufreq/scaling_governor; do
        [ -f "$cpu" ] && echo "$ORIGINAL_GOVERNOR" | sudo tee "$cpu" >/dev/null || true
    done
    echo "done!"
}
trap cleanup EXIT INT TERM

echo "=== tuning cpu ==="
echo "off" | sudo tee /sys/devices/system/cpu/smt/control >/dev/null
[ -f /sys/devices/system/cpu/cpufreq/boost ] && echo "0" | sudo tee /sys/devices/system/cpu/cpufreq/boost >/dev/null
for cpu in /sys/devices/system/cpu/cpu{0..15}/cpufreq/scaling_governor; do
    [ -f "$cpu" ] && echo "performance" | sudo tee "$cpu" >/dev/null || true
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
    output=$(taskset -c 0-7 cargo run --release --example bench_20 2>&1 | tail -3)
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
    output=$(taskset -c 0-7 cargo run --release --example bench_standardized_24 2>&1 | tail -5)
    ptime=$(echo "$output" | grep "^proving:" | awk '{print $2}' | tr -d 'ms')
    vtime=$(echo "$output" | grep "^verification:" | awk '{print $2}' | tr -d 'ms')
    echo "${ptime}ms / ${vtime}ms"
    rust_24_prove+=($ptime)
    rust_24_verify+=($vtime)
done
rust_24_p_min=$(printf '%s\n' "${rust_24_prove[@]}" | sort -n | head -1)
rust_24_v_min=$(printf '%s\n' "${rust_24_verify[@]}" | sort -n | head -1)
echo ""

echo "=== rust (zeratul) - 2^28 (3 runs) ==="
rust_28_prove=()
rust_28_verify=()
for i in {1..3}; do
    echo -n "  run $i/3... "
    output=$(taskset -c 0-7 cargo run --release --example bench_standardized_28 2>&1 | tail -5)
    ptime=$(echo "$output" | grep "^proving:" | awk '{print $2}' | tr -d 'ms')
    vtime=$(echo "$output" | grep "^verification:" | awk '{print $2}' | tr -d 'ms')
    ptime_s=$(echo "scale=2; $ptime / 1000" | bc)
    vtime_s=$(echo "scale=2; $vtime / 1000" | bc)
    echo "${ptime_s}s / ${vtime_s}s"
    rust_28_prove+=($ptime_s)
    rust_28_verify+=($vtime_s)
done
rust_28_p_min=$(printf '%s\n' "${rust_28_prove[@]}" | sort -n | head -1)
rust_28_v_min=$(printf '%s\n' "${rust_28_verify[@]}" | sort -n | head -1)
echo ""

echo "=== rust (zeratul) - 2^30 (3 runs) ==="
rust_30_prove=()
rust_30_verify=()
for i in {1..3}; do
    echo -n "  run $i/3... "
    output=$(taskset -c 0-7 cargo run --release --example bench_standardized_30 2>&1 | tail -5)
    ptime=$(echo "$output" | grep "^proving:" | awk '{print $2}' | tr -d 'ms')
    vtime=$(echo "$output" | grep "^verification:" | awk '{print $2}' | tr -d 'ms')
    ptime_s=$(echo "scale=2; $ptime / 1000" | bc)
    vtime_s=$(echo "scale=2; $vtime / 1000" | bc)
    echo "${ptime_s}s / ${vtime_s}s"
    rust_30_prove+=($ptime_s)
    rust_30_verify+=($vtime_s)
done
rust_30_p_min=$(printf '%s\n' "${rust_30_prove[@]}" | sort -n | head -1)
rust_30_v_min=$(printf '%s\n' "${rust_30_verify[@]}" | sort -n | head -1)
echo ""

# julia benchmarks
echo "=== julia (ligerito.jl) - 2^20 (10 runs) ==="
cd benchmarks/Ligerito.jl
julia_20_output=$(taskset -c 0-7 julia --threads=8 --project=. -e '
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
julia_24_output=$(taskset -c 0-7 julia --threads=8 --project=. -e '
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

# calculate ratios
ratio_20_p=$(echo "scale=2; $rust_20_p_min / $julia_20_p_min" | bc)
ratio_20_v=$(echo "scale=2; $rust_20_v_min / $julia_20_v_min" | bc)
ratio_24_p=$(echo "scale=2; $rust_24_p_min / $julia_24_p_min" | bc)
ratio_24_v=$(echo "scale=2; $rust_24_v_min / $julia_24_v_min" | bc)

echo "=== final comparison (8 physical cores, SMT off, turbo off) ==="
echo ""
echo "| size | elements | julia proving | julia verify | rust proving | rust verify | proving ratio | verify ratio |"
echo "|------|----------|---------------|--------------|--------------|-------------|---------------|--------------|"
echo "| 2^20 | 1.05M    | ${julia_20_p_min}ms | ${julia_20_v_min}ms | ${rust_20_p_min}ms | ${rust_20_v_min}ms | ${ratio_20_p}x | ${ratio_20_v}x |"
echo "| 2^24 | 16.8M    | ${julia_24_p_min}ms | ${julia_24_v_min}ms | ${rust_24_p_min}ms | ${rust_24_v_min}ms | ${ratio_24_p}x | ${ratio_24_v}x |"
echo "| 2^28 | 268.4M   | TBD | TBD | ${rust_28_p_min}s | ${rust_28_v_min}s | TBD | TBD |"
echo "| 2^30 | 1.07B    | TBD | TBD | ${rust_30_p_min}s | ${rust_30_v_min}s | TBD | TBD |"
echo ""
