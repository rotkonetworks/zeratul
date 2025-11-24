#!/bin/bash
# run testnet with 3 validators locally

set -e

echo "building validators..."
cargo build --release --bin validator

echo ""
echo "creating data directories..."
mkdir -p /tmp/zeratul/validator-{0,1,2}

echo ""
echo "starting validators..."
echo "press ctrl+c to stop all"
echo ""

# run validators in background
./target/release/validator --config configs/validator-0.yaml &
PID0=$!
sleep 0.5

./target/release/validator --config configs/validator-1.yaml &
PID1=$!
sleep 0.5

./target/release/validator --config configs/validator-2.yaml &
PID2=$!

echo "validators started: $PID0 $PID1 $PID2"
echo ""

# wait for ctrl+c
trap "kill $PID0 $PID1 $PID2 2>/dev/null; exit" SIGINT SIGTERM

wait
