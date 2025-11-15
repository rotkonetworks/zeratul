#!/bin/bash
# Build script for Terminator
# Workaround for rocksdb build issues

set -e

echo "Building Terminator..."
echo "Note: Using system rocksdb library"

export ROCKSDB_LIB_DIR=/usr/lib64

cargo build --release "$@"

echo ""
echo "✓ Build complete!"
echo "  Binary: target/release/terminator"
echo ""
echo "To run:"
echo "  ./target/release/terminator"
