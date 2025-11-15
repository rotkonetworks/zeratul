#!/bin/bash
# Build script for terminator - handles libclang path detection

# Detect libclang location
if [ -f "/usr/lib/libclang.so" ]; then
    # Arch Linux / distrobox
    export LIBCLANG_PATH=/usr/lib
elif [ -d "/nix/store" ]; then
    # NixOS - find libclang in nix store
    CLANG_PATH=$(find /nix/store -name "libclang.so" -type f 2>/dev/null | grep -v "rocm" | head -1)
    if [ -n "$CLANG_PATH" ]; then
        export LIBCLANG_PATH=$(dirname "$CLANG_PATH")
    fi
fi

# Check if LIBCLANG_PATH is set
if [ -z "$LIBCLANG_PATH" ]; then
    echo "Warning: Could not auto-detect libclang location"
    echo "Please set LIBCLANG_PATH manually"
    exit 1
fi

echo "Using LIBCLANG_PATH: $LIBCLANG_PATH"

# Build
cargo build --release -p terminator "$@"
