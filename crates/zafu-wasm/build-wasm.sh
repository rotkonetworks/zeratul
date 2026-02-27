#!/bin/bash
# build zafu-wasm for browser extension
#
# atomics + bulk-memory: required for wasm-bindgen-rayon thread pool
# simd128: accelerates field arithmetic
#
# NOTE: --shared-memory linker flag is NOT used because wasm32-unknown-unknown
# does not generate __wasm_init_tls, which wasm-bindgen requires for thread
# transforms. the rayon thread pool will fail at runtime in chrome extensions
# (SharedArrayBuffer/Memory cloning issue). the worker falls back to
# single-threaded scan_actions(). parallel scanning requires fixing TLS
# generation in rustc for wasm32-unknown-unknown or switching to wasm32-wasip1-threads.
set -e

RUSTFLAGS='-C target-feature=+atomics,+bulk-memory,+mutable-globals,+simd128' \
cargo +nightly build \
    --lib \
    --release \
    --target wasm32-unknown-unknown \
    -Z build-std=panic_abort,std

mkdir -p pkg

WASM_FILE="../../target/wasm32-unknown-unknown/release/zafu_wasm.wasm"
[ ! -f "$WASM_FILE" ] && WASM_FILE="target/wasm32-unknown-unknown/release/zafu_wasm.wasm"

wasm-bindgen "$WASM_FILE" --out-dir pkg --target web

# copy to zidecar www
cp pkg/zafu_wasm_bg.wasm ../bin/zidecar/www/pkg/
cp pkg/zafu_wasm.js ../bin/zidecar/www/pkg/
cp pkg/zafu_wasm.d.ts ../bin/zidecar/www/pkg/ 2>/dev/null || true

# copy rayon worker helpers
mkdir -p ../bin/zidecar/www/pkg/snippets/wasm-bindgen-rayon-38edf6e439f6d70d/src
RAYON_SRC=$(find ~/.cargo/registry/src -name "wasm-bindgen-rayon-*" -type d 2>/dev/null | head -1)
if [ -n "$RAYON_SRC" ]; then
    cp "$RAYON_SRC/src/workerHelpers.js" ../bin/zidecar/www/pkg/snippets/wasm-bindgen-rayon-38edf6e439f6d70d/src/
    cp "$RAYON_SRC/src/workerHelpers.no-bundler.js" ../bin/zidecar/www/pkg/snippets/wasm-bindgen-rayon-38edf6e439f6d70d/src/ 2>/dev/null || true
    sed -i "s|import('../../..')|import('../../../zafu_wasm.js')|g" ../bin/zidecar/www/pkg/snippets/wasm-bindgen-rayon-38edf6e439f6d70d/src/workerHelpers.js
fi

wc -c pkg/zafu_wasm_bg.wasm | awk '{printf "zafu-wasm: %s bytes (%.1f KB)\n", $1, $1/1024}'
