#!/bin/bash
set -e

# julia 2^28 and 2^30 benchmarks with proper cpu tuning

echo "=== julia large benchmarks ==="
echo "config: 8 physical cores, SMT off, turbo off"
echo ""

# check sudo
if ! sudo -n true 2>/dev/null; then
    echo "error: need passwordless sudo"
    exit 1
fi

# save state
ORIGINAL_GOVERNOR=$(cat /sys/devices/system/cpu/cpu0/cpufreq/scaling_governor)
ORIGINAL_BOOST=$(cat /sys/devices/system/cpu/cpufreq/boost 2>/dev/null || echo "1")
ORIGINAL_SMT=$(cat /sys/devices/system/cpu/smt/control)

cleanup() {
    echo ""
    echo "restoring state..."
    echo "$ORIGINAL_SMT" | sudo tee /sys/devices/system/cpu/smt/control >/dev/null || true
    [ -f /sys/devices/system/cpu/cpufreq/boost ] && echo "$ORIGINAL_BOOST" | sudo tee /sys/devices/system/cpu/cpufreq/boost >/dev/null
    for cpu in /sys/devices/system/cpu/cpu{0..15}/cpufreq/scaling_governor; do
        [ -f "$cpu" ] && echo "$ORIGINAL_GOVERNOR" | sudo tee "$cpu" >/dev/null || true
    done
}
trap cleanup EXIT INT TERM

echo "tuning cpu..."
echo "off" | sudo tee /sys/devices/system/cpu/smt/control >/dev/null
[ -f /sys/devices/system/cpu/cpufreq/boost ] && echo "0" | sudo tee /sys/devices/system/cpu/cpufreq/boost >/dev/null
for cpu in /sys/devices/system/cpu/cpu{0..15}/cpufreq/scaling_governor; do
    [ -f "$cpu" ] && echo "performance" | sudo tee "$cpu" >/dev/null || true
done
sync
sleep 1
echo ""

cd benchmarks/Ligerito.jl

echo "=== julia 2^28 (3 runs) ==="
julia_28_output=$(taskset -c 0-7 julia --threads=8 --project=. -e '
using BinaryFields, Ligerito
config = Ligerito.hardcoded_config_28(BinaryElem32, BinaryElem128)
poly = [BinaryElem32(i % UInt32(0xFFFFFFFF)) for i in 0:(2^28-1)]
verifier_cfg = Ligerito.hardcoded_config_28_verifier()

# warmup
println("warming up...")
for _ in 1:2
    proof = prover(config, poly)
    result = verifier(verifier_cfg, proof)
end

# timed runs
prove_times = Float64[]
verify_times = Float64[]
for i in 1:3
    t_prove = @elapsed proof = prover(config, poly)
    t_verify = @elapsed result = verifier(verifier_cfg, proof)
    push!(prove_times, t_prove)
    push!(verify_times, t_verify)
    println("  run $i/3... $(round(t_prove, digits=2))s / $(round(t_verify, digits=2))s")
end
println("MIN: $(round(minimum(prove_times), digits=2))s / $(round(minimum(verify_times), digits=2))s")
' 2>&1)

echo "$julia_28_output" | grep -E "(warming|run|MIN)"
julia_28_p_min=$(echo "$julia_28_output" | grep "^MIN:" | awk '{print $2}' | tr -d 's/')
julia_28_v_min=$(echo "$julia_28_output" | grep "^MIN:" | awk '{print $4}' | tr -d 's')
echo ""

echo "=== julia 2^30 (3 runs) ==="
julia_30_output=$(taskset -c 0-7 julia --threads=8 --project=. -e '
using BinaryFields, Ligerito
config = Ligerito.hardcoded_config_30(BinaryElem32, BinaryElem128)
poly = [BinaryElem32(i % UInt32(0xFFFFFFFF)) for i in 0:(2^30-1)]
verifier_cfg = Ligerito.hardcoded_config_30_verifier()

# warmup
println("warming up...")
for _ in 1:2
    proof = prover(config, poly)
    result = verifier(verifier_cfg, proof)
end

# timed runs
prove_times = Float64[]
verify_times = Float64[]
for i in 1:3
    t_prove = @elapsed proof = prover(config, poly)
    t_verify = @elapsed result = verifier(verifier_cfg, proof)
    push!(prove_times, t_prove)
    push!(verify_times, t_verify)
    println("  run $i/3... $(round(t_prove, digits=2))s / $(round(t_verify, digits=2))s")
end
println("MIN: $(round(minimum(prove_times), digits=2))s / $(round(minimum(verify_times), digits=2))s")
' 2>&1)

echo "$julia_30_output" | grep -E "(warming|run|MIN)"
julia_30_p_min=$(echo "$julia_30_output" | grep "^MIN:" | awk '{print $2}' | tr -d 's/')
julia_30_v_min=$(echo "$julia_30_output" | grep "^MIN:" | awk '{print $4}' | tr -d 's')
echo ""

echo "=== results ==="
echo "2^28: proving ${julia_28_p_min}s, verification ${julia_28_v_min}s"
echo "2^30: proving ${julia_30_p_min}s, verification ${julia_30_v_min}s"
