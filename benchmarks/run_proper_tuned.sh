#!/bin/bash
set -e

# proper cpu-tuned benchmarking: disable SMT, turbo, use physical cores only

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
cd "$SCRIPT_DIR/.."

echo "=== zeratul proper tuned benchmarks ==="
echo "hardware: $(cat /proc/cpuinfo | grep 'model name' | head -1 | cut -d: -f2 | xargs)"
echo "cores: 8 physical cores (0-7), SMT disabled, turbo disabled"
echo "date: $(date +%Y-%m-%d)"
echo ""

# check sudo
if ! sudo -n true 2>/dev/null; then
    echo "error: need passwordless sudo"
    echo "run: ./benchmarks/setup_sudo.sh"
    exit 1
fi

# save original state
ORIGINAL_GOVERNOR=$(cat /sys/devices/system/cpu/cpu0/cpufreq/scaling_governor)
ORIGINAL_BOOST=$(cat /sys/devices/system/cpu/cpufreq/boost 2>/dev/null || echo "1")
ORIGINAL_SMT=$(cat /sys/devices/system/cpu/smt/control)

echo "original state:"
echo "  governor: $ORIGINAL_GOVERNOR"
echo "  turbo boost: $ORIGINAL_BOOST"
echo "  smt: $ORIGINAL_SMT"
echo ""

cleanup() {
    echo ""
    echo "=== restoring original state ==="

    # restore SMT
    echo "restoring SMT to '$ORIGINAL_SMT'..."
    echo "$ORIGINAL_SMT" | sudo tee /sys/devices/system/cpu/smt/control >/dev/null || true

    # restore turbo boost
    if [ -f /sys/devices/system/cpu/cpufreq/boost ]; then
        echo "restoring turbo boost to '$ORIGINAL_BOOST'..."
        echo "$ORIGINAL_BOOST" | sudo tee /sys/devices/system/cpu/cpufreq/boost >/dev/null
    fi

    # restore scaling governor
    echo "restoring scaling governor to '$ORIGINAL_GOVERNOR'..."
    for cpu in /sys/devices/system/cpu/cpu{0..15}/cpufreq/scaling_governor; do
        [ -f "$cpu" ] && echo "$ORIGINAL_GOVERNOR" | sudo tee "$cpu" >/dev/null || true
    done

    echo "done!"
}

trap cleanup EXIT INT TERM

echo "=== tuning cpu for benchmarking ==="

# disable SMT (hyperthreading)
echo "disabling SMT..."
echo "off" | sudo tee /sys/devices/system/cpu/smt/control >/dev/null

# disable turbo boost
if [ -f /sys/devices/system/cpu/cpufreq/boost ]; then
    echo "disabling turbo boost..."
    echo "0" | sudo tee /sys/devices/system/cpu/cpufreq/boost >/dev/null
fi

# set performance governor on physical cores 0-15
echo "setting performance governor on physical cores 0-15..."
for cpu in /sys/devices/system/cpu/cpu{0..15}/cpufreq/scaling_governor; do
    [ -f "$cpu" ] && echo "performance" | sudo tee "$cpu" >/dev/null || true
done

# verify SMT is actually off
smt_state=$(cat /sys/devices/system/cpu/smt/control)
if [ "$smt_state" != "off" ] && [ "$smt_state" != "forceoff" ]; then
    echo "WARNING: SMT is still '$smt_state', not 'off'!"
fi

sync
sleep 1

echo ""
echo "=== building optimized binaries ==="
cargo build --release --quiet
echo ""

# use 8 threads on physical cores 0-7 (reserve higher cores for other work)
export RAYON_NUM_THREADS=8

min_time() {
    sort -n | head -1
}

avg_time() {
    awk '{sum+=$1; count++} END {printf "%.2f", sum/count}'
}

echo "=== 2^20 (1,048,576 elements) - 10 runs ==="
prove_times_20=()
verify_times_20=()
for i in {1..10}; do
    echo -n "  run $i/10... "
    # pin to physical cores 0-7 (SMT disabled so these are the only active cores)
    output=$(taskset -c 0-7 cargo run --release --example bench_20 2>&1 | tail -3)
    ptime=$(echo "$output" | grep "^proving:" | awk '{print $2}' | tr -d 'ms')
    vtime=$(echo "$output" | grep "^verification:" | awk '{print $2}' | tr -d 'ms')
    echo "prove: ${ptime}ms, verify: ${vtime}ms"
    prove_times_20+=($ptime)
    verify_times_20+=($vtime)
done

min_prove_20=$(printf '%s\n' "${prove_times_20[@]}" | min_time)
avg_prove_20=$(printf '%s\n' "${prove_times_20[@]}" | avg_time)
min_verify_20=$(printf '%s\n' "${verify_times_20[@]}" | min_time)
avg_verify_20=$(printf '%s\n' "${verify_times_20[@]}" | avg_time)

echo ""
echo "2^20 results:"
echo "  proving:      min=${min_prove_20}ms, avg=${avg_prove_20}ms"
echo "  verification: min=${min_verify_20}ms, avg=${avg_verify_20}ms"
echo ""

echo "=== 2^24 (16,777,216 elements) - 10 runs ==="
prove_times_24=()
verify_times_24=()
for i in {1..10}; do
    echo -n "  run $i/10... "
    output=$(taskset -c 0-7 cargo run --release --example bench_standardized_24 2>&1 | tail -5)
    ptime=$(echo "$output" | grep "^proving:" | awk '{print $2}' | tr -d 'ms')
    vtime=$(echo "$output" | grep "^verification:" | awk '{print $2}' | tr -d 'ms')
    echo "prove: ${ptime}ms, verify: ${vtime}ms"
    prove_times_24+=($ptime)
    verify_times_24+=($vtime)
done

min_prove_24=$(printf '%s\n' "${prove_times_24[@]}" | min_time)
avg_prove_24=$(printf '%s\n' "${prove_times_24[@]}" | avg_time)
min_verify_24=$(printf '%s\n' "${verify_times_24[@]}" | min_time)
avg_verify_24=$(printf '%s\n' "${verify_times_24[@]}" | avg_time)

echo ""
echo "2^24 results:"
echo "  proving:      min=${min_prove_24}ms, avg=${avg_prove_24}ms"
echo "  verification: min=${min_verify_24}ms, avg=${avg_verify_24}ms"
echo ""

echo "=== 2^28 (268,435,456 elements) - 3 runs ==="
prove_times_28=()
verify_times_28=()
for i in {1..3}; do
    echo -n "  run $i/3... "
    output=$(taskset -c 0-7 cargo run --release --example bench_standardized_28 2>&1 | tail -5)
    ptime=$(echo "$output" | grep "^proving:" | awk '{print $2}' | tr -d 'ms')
    vtime=$(echo "$output" | grep "^verification:" | awk '{print $2}' | tr -d 'ms')
    ptime_s=$(echo "scale=2; $ptime / 1000" | bc)
    echo "prove: ${ptime_s}s, verify: ${vtime}ms"
    prove_times_28+=($ptime_s)
    verify_times_28+=($vtime)
done

min_prove_28=$(printf '%s\n' "${prove_times_28[@]}" | min_time)
avg_prove_28=$(printf '%s\n' "${prove_times_28[@]}" | avg_time)
min_verify_28=$(printf '%s\n' "${verify_times_28[@]}" | min_time)
avg_verify_28=$(printf '%s\n' "${verify_times_28[@]}" | avg_time)

echo ""
echo "2^28 results:"
echo "  proving:      min=${min_prove_28}s, avg=${avg_prove_28}s"
echo "  verification: min=${min_verify_28}ms, avg=${avg_verify_28}ms"
echo ""

echo "=== 2^30 (1,073,741,824 elements) - 3 runs ==="
prove_times_30=()
verify_times_30=()
for i in {1..3}; do
    echo -n "  run $i/3... "
    output=$(taskset -c 0-7 cargo run --release --example bench_standardized_30 2>&1 | tail -5)
    ptime=$(echo "$output" | grep "^proving:" | awk '{print $2}' | tr -d 'ms')
    vtime=$(echo "$output" | grep "^verification:" | awk '{print $2}' | tr -d 'ms')
    ptime_s=$(echo "scale=2; $ptime / 1000" | bc)
    echo "prove: ${ptime_s}s, verify: ${vtime}ms"
    prove_times_30+=($ptime_s)
    verify_times_30+=($vtime)
done

min_prove_30=$(printf '%s\n' "${prove_times_30[@]}" | min_time)
avg_prove_30=$(printf '%s\n' "${prove_times_30[@]}" | avg_time)
min_verify_30=$(printf '%s\n' "${verify_times_30[@]}" | min_time)
avg_verify_30=$(printf '%s\n' "${verify_times_30[@]}" | avg_time)

echo ""
echo "2^30 results:"
echo "  proving:      min=${min_prove_30}s, avg=${avg_prove_30}s"
echo "  verification: min=${min_verify_30}ms, avg=${avg_verify_30}ms"
echo ""

echo "=== summary (8 physical cores, SMT off, turbo off, performance governor) ==="
echo ""
echo "| size | elements | proving (min/avg) | verification (min/avg) |"
echo "|------|----------|-------------------|------------------------|"
echo "| 2^20 | 1.05M    | ${min_prove_20}ms / ${avg_prove_20}ms | ${min_verify_20}ms / ${avg_verify_20}ms |"
echo "| 2^24 | 16.8M    | ${min_prove_24}ms / ${avg_prove_24}ms | ${min_verify_24}ms / ${avg_verify_24}ms |"
echo "| 2^28 | 268.4M   | ${min_prove_28}s / ${avg_prove_28}s | ${min_verify_28}ms / ${avg_verify_28}ms |"
echo "| 2^30 | 1.07B    | ${min_prove_30}s / ${avg_prove_30}s | ${min_verify_30}ms / ${avg_verify_30}ms |"
echo ""
echo "all benchmarks completed successfully"
