#!/bin/bash
# Fix for librocksdb-sys build issues with newer GCC/Clang
# Adds -include cstdint flag to fix compilation

set -e

ROCKSDB_SRC=$(find ~/.cargo/registry/src -name "librocksdb-sys-*" -type d 2>/dev/null | head -1)

if [ -z "$ROCKSDB_SRC" ]; then
    echo "Error: librocksdb-sys source not found in cargo registry"
    echo "Try running 'cargo fetch' first"
    exit 1
fi

BUILD_RS="$ROCKSDB_SRC/build.rs"

if [ ! -f "$BUILD_RS" ]; then
    echo "Error: build.rs not found at $BUILD_RS"
    exit 1
fi

# Check if already patched
if grep -q 'config.flag("cstdint")' "$BUILD_RS"; then
    echo "✓ Already patched: $BUILD_RS"
    exit 0
fi

echo "Patching: $BUILD_RS"

# Backup
cp "$BUILD_RS" "$BUILD_RS.orig"

# Apply fix using sed
sed -i '/config.cpp(true);/a\    config.flag("-include");\n    config.flag("cstdint");' "$BUILD_RS"

# Verify
if grep -q 'config.flag("cstdint")' "$BUILD_RS"; then
    echo "✓ Successfully patched librocksdb-sys build.rs"
    echo "  Added: config.flag(\"-include\"); config.flag(\"cstdint\");"
    echo ""
    echo "Now run: ./build.sh"
else
    echo "✗ Patch failed, restoring backup"
    mv "$BUILD_RS.orig" "$BUILD_RS"
    exit 1
fi
