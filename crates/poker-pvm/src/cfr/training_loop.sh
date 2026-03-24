#!/bin/bash
# CTM-MoE training loop: self-play → train → measure → repeat
#
# usage: ./training_loop.sh [iterations] [hands_per_iter]
#
# each iteration:
#   1. Rust self-play generates training data
#   2. Python trains CTM-MoE experts on GPU (or CPU)
#   3. Measure exploitability
#   4. Export ONNX for next iteration

set -e

ITERS=${1:-5}
HANDS=${2:-500000}
STRATEGY=${3:-/tmp/strategy_100m.bin}
MODEL_DIR=${4:-models}
SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
PVM_DIR="$(cd "$SCRIPT_DIR/../.." && pwd)"

mkdir -p "$MODEL_DIR"

echo "=== CTM-MoE Training Loop ==="
echo "iterations: $ITERS"
echo "hands/iter: $HANDS"
echo "strategy:   $STRATEGY"
echo "models:     $MODEL_DIR"
echo ""

for i in $(seq 1 $ITERS); do
    echo "========================================"
    echo "ITERATION $i / $ITERS"
    echo "========================================"

    # 1. Self-play data generation (Rust)
    echo "[1/4] generating self-play data ($HANDS hands)..."
    cargo run --release --features std --bin cfr-selfplay -- \
        --strategy "$STRATEGY" \
        --hands "$HANDS" \
        --output "$MODEL_DIR/selfplay_v${i}.bin" \
        ${PREV_ONNX:+--moe-dir "$MODEL_DIR/onnx"}

    SAMPLES=$(python3 -c "
import struct
with open('$MODEL_DIR/selfplay_v${i}.bin', 'rb') as f:
    n = struct.unpack('<I', f.read(4))[0]
    print(n)
")
    echo "  generated $SAMPLES samples"

    # 2. Train CTM-MoE experts (Python)
    echo "[2/4] training CTM-MoE v$i..."
    python3 "$SCRIPT_DIR/train_experts_v2.py" \
        --data "$MODEL_DIR/selfplay_v${i}.bin" \
        --version "$i" \
        --output-dir "$MODEL_DIR" \
        --epochs 60

    # 3. Export to ONNX
    echo "[3/4] exporting ONNX..."
    python3 "$SCRIPT_DIR/export_onnx.py" \
        --model-dir "$MODEL_DIR" \
        --version "$i" \
        --output-dir "$MODEL_DIR/onnx"

    PREV_ONNX="$MODEL_DIR/onnx"

    # 4. Measure exploitability
    echo "[4/4] measuring exploitability..."
    cargo run --release --features std --bin cfr-selfplay -- \
        --strategy "$STRATEGY" \
        --hands 50000 \
        --measure-only \
        ${PREV_ONNX:+--moe-dir "$MODEL_DIR/onnx"} \
        | tee -a "$MODEL_DIR/exploitability_log.txt"

    echo ""
done

echo "=== Training complete ==="
echo "Models in $MODEL_DIR/"
ls -lh "$MODEL_DIR"/*.pt "$MODEL_DIR"/onnx/*.onnx 2>/dev/null
echo ""
echo "Exploitability log:"
cat "$MODEL_DIR/exploitability_log.txt"
