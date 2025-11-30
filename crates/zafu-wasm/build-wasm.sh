#!/bin/bash
# Build multi-threaded WASM for zafu-wasm (parallel trial decryption)
set -e

echo "Building Zafu WASM (parallel trial decryption)"
echo "================================================"
echo

# Step 1: Build WASM with atomics + SIMD using nightly cargo
echo "Step 1: Building WASM with atomics + bulk-memory + SIMD..."
RUSTFLAGS='-C target-feature=+atomics,+bulk-memory,+mutable-globals,+simd128' \
cargo +nightly build \
    --lib \
    --release \
    --target wasm32-unknown-unknown \
    -Z build-std=panic_abort,std

echo
echo "✓ WASM binary built"

# Step 2: Run wasm-bindgen
echo
echo "Step 2: Generating JavaScript bindings..."
mkdir -p pkg

WASM_FILE="../../target/wasm32-unknown-unknown/release/zafu_wasm.wasm"
if [ ! -f "$WASM_FILE" ]; then
    WASM_FILE="target/wasm32-unknown-unknown/release/zafu_wasm.wasm"
fi

wasm-bindgen \
    "$WASM_FILE" \
    --out-dir pkg \
    --target web

echo "✓ JavaScript bindings generated"

# Step 3: Copy to zidecar www
echo
echo "Step 3: Copying to ../bin/zidecar/www/pkg/..."
cp pkg/zafu_wasm_bg.wasm ../bin/zidecar/www/pkg/
cp pkg/zafu_wasm.js ../bin/zidecar/www/pkg/
cp pkg/zafu_wasm.d.ts ../bin/zidecar/www/pkg/ 2>/dev/null || true

# Step 4: Copy worker helpers for rayon
echo "Step 4: Copying wasm-bindgen-rayon worker helpers..."
# wasm-bindgen-rayon snippets must be relative to zafu_wasm.js (in pkg/)
mkdir -p ../bin/zidecar/www/pkg/snippets/wasm-bindgen-rayon-38edf6e439f6d70d/src
RAYON_SRC=$(find ~/.cargo/registry/src -name "wasm-bindgen-rayon-*" -type d 2>/dev/null | head -1)
if [ -n "$RAYON_SRC" ]; then
    cp "$RAYON_SRC/src/workerHelpers.js" ../bin/zidecar/www/pkg/snippets/wasm-bindgen-rayon-38edf6e439f6d70d/src/
    cp "$RAYON_SRC/src/workerHelpers.no-bundler.js" ../bin/zidecar/www/pkg/snippets/wasm-bindgen-rayon-38edf6e439f6d70d/src/ 2>/dev/null || true

    # Fix the import path for non-bundler usage
    # Change '../../..' to '../../../zafu_wasm.js' for correct module resolution
    sed -i "s|import('../../..')|import('../../../zafu_wasm.js')|g" ../bin/zidecar/www/pkg/snippets/wasm-bindgen-rayon-38edf6e439f6d70d/src/workerHelpers.js

    echo "✓ Worker helpers copied and patched for pkg/snippets/"
else
    echo "⚠️  Could not find wasm-bindgen-rayon source (worker helpers may need manual copy)"
fi

echo
echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
echo "✓ Zafu WASM build complete!"
echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
echo
wc -c ../bin/zidecar/www/pkg/zafu_wasm_bg.wasm | awk '{printf "WASM size: %s bytes (%.2f KB)\n", $1, $1/1024}'
echo
echo "Features:"
echo "  - Parallel trial decryption (rayon + web workers)"
echo "  - SIMD128 acceleration"
echo "  - Binary batch action format (148 bytes/action)"
echo
