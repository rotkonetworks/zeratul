#!/bin/bash
# Build multi-threaded WASM manually (bypass wasm-pack limitations)
#
# This script builds WASM with SharedArrayBuffer support using cargo directly,
# then runs wasm-bindgen to generate JavaScript bindings.

set -e

echo "Building Multi-threaded WASM (manual cargo build)"
echo "=================================================="
echo "⚠️  This build requires:"
echo "  - Nightly Rust"
echo "  - SharedArrayBuffer + Atomics browser support"
echo "  - Server headers:"
echo "    Cross-Origin-Opener-Policy: same-origin"
echo "    Cross-Origin-Embedder-Policy: require-corp"
echo

# Step 1: Build WASM with atomics + SIMD using cargo
echo "Step 1: Building WASM with atomics + bulk-memory + SIMD..."
echo "Note: Enabling WASM SIMD128 for 2-4x speedup in binary field operations"
echo "Note: Setting max memory to 4GB for large polynomial support (2^28)"

# Build from ligerito manifest with explicit target dir
RUSTFLAGS='-C target-feature=+atomics,+bulk-memory,+mutable-globals,+simd128 -C link-arg=--max-memory=4294967296' \
cargo +nightly build \
    --manifest-path ../Cargo.toml \
    --target-dir ../target \
    --lib \
    --release \
    --target wasm32-unknown-unknown \
    --features wasm-parallel,hardware-accel \
    --no-default-features \
    -Z build-std=panic_abort,std

echo
echo "✓ WASM binary built successfully"

# Step 2: Run wasm-bindgen
echo
echo "Step 2: Generating JavaScript bindings..."
mkdir -p ../pkg/parallel-web

# Find the WASM file - check multiple possible locations
WASM_FILE="../target/wasm32-unknown-unknown/release/ligerito.wasm"
if [ ! -f "$WASM_FILE" ]; then
    WASM_FILE="../../../target/wasm32-unknown-unknown/release/ligerito.wasm"
fi
if [ ! -f "$WASM_FILE" ]; then
    WASM_FILE="../../target/wasm32-unknown-unknown/release/ligerito.wasm"
fi
if [ ! -f "$WASM_FILE" ]; then
    echo "Error: Could not find ligerito.wasm"
    echo "Searched: ../target, ../../../target, ../../target"
    exit 1
fi
echo "Found WASM at: $WASM_FILE"

wasm-bindgen \
    "$WASM_FILE" \
    --out-dir ../pkg/parallel-web \
    --target web

echo
echo "✓ JavaScript bindings generated"

# Step 3: Run wasm-opt (optional, for size optimization)
if command -v wasm-opt &> /dev/null; then
    echo
    echo "Step 3: Optimizing WASM with wasm-opt..."
    wasm-opt -Oz \
        ../pkg/parallel-web/ligerito_bg.wasm \
        -o ../pkg/parallel-web/ligerito_bg.wasm
    echo "✓ WASM optimized"
else
    echo
    echo "⚠️  wasm-opt not found (skipping optimization)"
    echo "   Install with: cargo install wasm-opt"
fi

# Step 4: Copy to www directory
echo
echo "Step 4: Copying to ../examples/www/..."
cp ../pkg/parallel-web/ligerito_bg.wasm ../examples/www/
cp ../pkg/parallel-web/ligerito.js ../examples/www/

echo
echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
echo "✓ Multi-threaded WASM build complete!"
echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
echo
echo "Output:"
echo "  - ../pkg/parallel-web/ligerito_bg.wasm"
echo "  - ../pkg/parallel-web/ligerito.js"
echo "  - ../examples/www/ (copied)"
echo
wc -c ../pkg/parallel-web/ligerito_bg.wasm | awk '{printf "WASM size: %s bytes (%.2f MB)\n", $1, $1/1024/1024}'
echo
echo "To test:"
echo "  cd examples/www cd www &&cd www && python3 serve-multithreaded.py 8080"
echo "  Then open: http://localhost:8080"
echo
