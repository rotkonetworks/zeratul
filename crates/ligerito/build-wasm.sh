#!/bin/bash
# Build Ligerito for WASM
#
# Prerequisites:
#   cargo install wasm-pack
#
# Usage:
#   ./build-wasm.sh           # Single-threaded WASM (works everywhere)
#   ./build-wasm.sh parallel  # Multi-threaded WASM (requires SharedArrayBuffer)
#
# This script builds the WASM module and generates JavaScript bindings

set -e

# Check if building with multi-threading support
PARALLEL_MODE=""
if [ "$1" = "parallel" ]; then
    PARALLEL_MODE="yes"
    FEATURES="wasm-parallel"
    echo "Building Ligerito for WASM (MULTI-THREADED)..."
    echo "================================================"
    echo "⚠️  This build requires:"
    echo "  - SharedArrayBuffer + Atomics browser support"
    echo "  - Server headers:"
    echo "    Cross-Origin-Opener-Policy: same-origin"
    echo "    Cross-Origin-Embedder-Policy: require-corp"
else
    FEATURES="wasm"
    echo "Building Ligerito for WASM (single-threaded)..."
    echo "================================================"
fi
echo

# Build for web (generates ES modules)
echo "Building for web (ES modules)..."
if [ "$PARALLEL_MODE" = "yes" ]; then
    # Multi-threaded build requires atomics + bulk-memory
    RUSTFLAGS='-C target-feature=+atomics,+bulk-memory,+mutable-globals' \
    wasm-pack build \
        --target web \
        --out-dir pkg/web \
        --features $FEATURES \
        --no-default-features \
        -- -Z build-std=panic_abort,std
else
    # Single-threaded build (normal)
    wasm-pack build \
        --target web \
        --out-dir pkg/web \
        --features $FEATURES \
        --no-default-features
fi

echo
echo "Building for bundler (webpack/vite/etc)..."
if [ "$PARALLEL_MODE" = "yes" ]; then
    RUSTFLAGS='-C target-feature=+atomics,+bulk-memory,+mutable-globals' \
    wasm-pack build \
        --target bundler \
        --out-dir pkg/bundler \
        --features $FEATURES \
        --no-default-features \
        -- -Z build-std=panic_abort,std
else
    wasm-pack build \
        --target bundler \
        --out-dir pkg/bundler \
        --features $FEATURES \
        --no-default-features
fi

echo
echo "Building for Node.js..."
if [ "$PARALLEL_MODE" = "yes" ]; then
    RUSTFLAGS='-C target-feature=+atomics,+bulk-memory,+mutable-globals' \
    wasm-pack build \
        --target nodejs \
        --out-dir pkg/nodejs \
        --features $FEATURES \
        --no-default-features \
        -- -Z build-std=panic_abort,std
else
    wasm-pack build \
        --target nodejs \
        --out-dir pkg/nodejs \
        --features $FEATURES \
        --no-default-features
fi

echo
echo "✓ Build complete!"

# Copy web build to www directory
echo
echo "Copying web build to www/..."
cp pkg/web/ligerito_bg.wasm www/
cp pkg/web/ligerito.js www/

echo
echo "Output directories:"
echo "  - pkg/web/      (for <script type=\"module\">)"
echo "  - pkg/bundler/  (for webpack/vite)"
echo "  - pkg/nodejs/   (for Node.js)"
echo "  - www/          (demo website)"
echo
echo "WASM file sizes:"
wc -c pkg/web/ligerito_bg.wasm | awk '{printf "  Web:     %s bytes (%.2f MB)\n", $1, $1/1024/1024}'
wc -c pkg/bundler/ligerito_bg.wasm | awk '{printf "  Bundler: %s bytes (%.2f MB)\n", $1, $1/1024/1024}'
wc -c pkg/nodejs/ligerito_bg.wasm | awk '{printf "  Node.js: %s bytes (%.2f MB)\n", $1, $1/1024/1024}'
echo
echo "To run the demo website:"
echo "  cd www && python3 -m http.server 8080"
echo "  Then open: http://localhost:8080"
