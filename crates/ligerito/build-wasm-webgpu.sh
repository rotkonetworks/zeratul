#!/bin/bash
# Build Ligerito for WASM with WebGPU acceleration
#
# Prerequisites:
#   cargo install wasm-pack
#
# Usage:
#   ./build-wasm-webgpu.sh
#
# This script builds the WASM module with WebGPU support for GPU-accelerated
# sumcheck operations in the browser

set -e

echo "Building Ligerito for WASM with WebGPU acceleration..."
echo "======================================================"
echo

# Build for web (generates ES modules)
echo "Building for web (ES modules) with WebGPU..."
wasm-pack build \
    --target web \
    --out-dir pkg/web-webgpu \
    --features webgpu \
    --no-default-features

echo
echo "✓ Build complete!"

# Copy web build to www directory
echo
echo "Copying web build to www/..."
mkdir -p www/webgpu
cp pkg/web-webgpu/ligerito_bg.wasm www/webgpu/
cp pkg/web-webgpu/ligerito.js www/webgpu/
cp pkg/web-webgpu/ligerito_bg.wasm.d.ts www/webgpu/ 2>/dev/null || true
cp pkg/web-webgpu/ligerito.d.ts www/webgpu/ 2>/dev/null || true

echo
echo "Output directory:"
echo "  - pkg/web-webgpu/ (ES modules with WebGPU)"
echo "  - www/webgpu/     (demo website assets)"
echo
echo "WASM file size:"
wc -c pkg/web-webgpu/ligerito_bg.wasm | awk '{printf "  %.2f MB (%s bytes)\n", $1/1024/1024, $1}'
echo
echo "To test the demo:"
echo "  cd www && python3 -m http.server 8080"
echo "  Then open: http://localhost:8080/benchmark.html"
echo
echo "Note: WebGPU requires:"
echo "  - Chrome 113+ or Edge 113+ (desktop or Android)"
echo "  - HTTPS or localhost"
