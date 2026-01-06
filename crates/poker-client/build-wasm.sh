#!/bin/bash
# build poker-client for wasm

set -e

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
WORKSPACE_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"
WEB_DIR="$SCRIPT_DIR/web"

# ensure wasm-bindgen-cli is installed
if ! command -v wasm-bindgen &> /dev/null; then
    echo "installing wasm-bindgen-cli..."
    cargo install wasm-bindgen-cli
fi

# build for wasm with unstable web_sys apis for clipboard
echo "building for wasm32-unknown-unknown..."
cd "$WORKSPACE_ROOT"
RUSTFLAGS="--cfg=web_sys_unstable_apis" cargo build -p poker-client --release --target wasm32-unknown-unknown

# generate js bindings
echo "generating js bindings..."
wasm-bindgen --out-dir "$WEB_DIR" --target web \
    "$WORKSPACE_ROOT/target/wasm32-unknown-unknown/release/poker-client.wasm"

echo "build complete! files in $WEB_DIR"
echo "to serve: python3 -m http.server -d $WEB_DIR 8080"
