#!/bin/bash
set -e

# proper benchmarking with cpu governor tuning
# pins to performance cores (1-8), leaves core 0 for system

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
cd "$SCRIPT_DIR/.."

echo "=== zeratul tuned benchmarks ==="
echo "hardware: $(cat /proc/cpuinfo | grep 'model name' | head -1 | cut -d: -f2 | xargs)"
echo "date: $(date +%Y-%m-%d)"
echo ""

# check if we have sudo
if ! sudo -n true 2>/dev/null; then
    echo "error: need passwordless sudo for cpu governor tuning"
    echo "run: sudo visudo"
    echo "add: $USER ALL=(ALL) NOPASSWD: /usr/bin/tee /sys/devices/system/cpu/cpu*/cpufreq/scaling_governor"
    exit 1
fi

# save original state
ORIGINAL_GOVERNOR=$(cat /sys/devices/system/cpu/cpu1/cpufreq/scaling_governor)
ORIGINAL_BOOST=$(cat /sys/devices/system/cpu/cpufreq/boost 2>/dev/null || echo "1")

echo "original governor: $ORIGINAL_GOVERNOR"
echo "original boost: $ORIGINAL_BOOST"
echo ""

cleanup() {
    echo ""
    echo "=== restoring original state ==="

    # restore turbo boost
    if [ -f /sys/devices/system/cpu/cpufreq/boost ]; then
        echo "restoring turbo boost..."
        echo "$ORIGINAL_BOOST" | sudo tee /sys/devices/system/cpu/cpufreq/boost >/dev/null
    fi

    # restore scaling governor
    echo "restoring scaling governor to '$ORIGINAL_GOVERNOR'..."
    for cpu in /sys/devices/system/cpu/cpu{1..8}/cpufreq/scaling_governor; do
        [ -f "$cpu" ] && echo "$ORIGINAL_GOVERNOR" | sudo tee "$cpu" >/dev/null
    done

    echo "done!"
}

trap cleanup EXIT INT TERM

echo "=== tuning cpu for benchmarking ==="

# disable turbo boost for consistent results
if [ -f /sys/devices/system/cpu/cpufreq/boost ]; then
    echo "disabling turbo boost..."
    echo "0" | sudo tee /sys/devices/system/cpu/cpufreq/boost >/dev/null
fi

# set performance governor on cores 1-8 (skip core 0 for system)
echo "setting performance governor on cores 1-8..."
for cpu in /sys/devices/system/cpu/cpu{1..8}/cpufreq/scaling_governor; do
    [ -f "$cpu" ] && echo "performance" | sudo tee "$cpu" >/dev/null
done

# sync filesystem
sync
sleep 1

echo ""
echo "=== building optimized binaries ==="
cargo build --release
echo ""

# run benchmarks with pinned cores and 8 threads
export RAYON_NUM_THREADS=8

# helper function for min/avg
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
    # pin to cores 1-8 using taskset
    output=$(taskset -c 1-8 cargo run --release --example bench_20 2>&1 | tail -3)
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
    output=$(taskset -c 1-8 cargo run --release --example bench_standardized_24 2>&1 | tail -5)
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
    output=$(taskset -c 1-8 cargo run --release --example bench_standardized_28 2>&1 | tail -5)
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
    output=$(taskset -c 1-8 cargo run --release --example bench_standardized_30 2>&1 | tail -5)
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

echo "=== summary (8 cores, performance governor, turbo disabled) ==="
echo ""
echo "| size | elements | proving (min/avg) | verification (min/avg) |"
echo "|------|----------|-------------------|------------------------|"
echo "| 2^20 | 1.05M    | ${min_prove_20}ms / ${avg_prove_20}ms | ${min_verify_20}ms / ${avg_verify_20}ms |"
echo "| 2^24 | 16.8M    | ${min_prove_24}ms / ${avg_prove_24}ms | ${min_verify_24}ms / ${avg_verify_24}ms |"
echo "| 2^28 | 268.4M   | ${min_prove_28}s / ${avg_prove_28}s | ${min_verify_28}ms / ${avg_verify_28}ms |"
echo "| 2^30 | 1.07B    | ${min_prove_30}s / ${avg_prove_30}s | ${min_verify_30}ms / ${avg_verify_30}ms |"
echo ""
echo "all benchmarks completed successfully"
