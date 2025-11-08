#!/bin/bash

# setup passwordless sudo for cpu governor tuning

echo "=== setup passwordless sudo for benchmarking ==="
echo ""
echo "this will add a sudoers rule to allow your user ($USER) to modify:"
echo "  - /sys/devices/system/cpu/cpu*/cpufreq/scaling_governor"
echo "  - /sys/devices/system/cpu/cpufreq/boost"
echo "  - /sys/devices/system/cpu/smt/control"
echo ""
echo "without requiring a password for each benchmark run."
echo ""
read -p "continue? (y/n) " -n 1 -r
echo ""

if [[ ! $REPLY =~ ^[Yy]$ ]]; then
    echo "aborted"
    exit 1
fi

SUDOERS_FILE="/etc/sudoers.d/zeratul-benchmarks"

cat <<EOF | sudo tee "$SUDOERS_FILE" > /dev/null
# allow $USER to modify cpu governor for benchmarking
$USER ALL=(ALL) NOPASSWD: /usr/bin/tee /sys/devices/system/cpu/cpu*/cpufreq/scaling_governor
$USER ALL=(ALL) NOPASSWD: /usr/bin/tee /sys/devices/system/cpu/cpufreq/boost
$USER ALL=(ALL) NOPASSWD: /usr/bin/tee /sys/devices/system/cpu/smt/control
EOF

sudo chmod 0440 "$SUDOERS_FILE"
sudo visudo -c -f "$SUDOERS_FILE"

if [ $? -eq 0 ]; then
    echo ""
    echo "✓ sudoers rule installed at $SUDOERS_FILE"
    echo ""
    echo "you can now run tuned benchmarks without password prompts:"
    echo "  ./benchmarks/run_tuned_benchmarks.sh"
    echo "  ./benchmarks/compare_julia_rust.sh"
else
    echo ""
    echo "✗ error: sudoers file syntax check failed"
    sudo rm -f "$SUDOERS_FILE"
    exit 1
fi
