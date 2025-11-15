#!/bin/bash
# Fix for librocksdb-sys build issues with newer GCC/Clang
# Adds -include cstdint flag to fix compilation

set -e
ROCKSDB_SRC=$(find ~/.cargo/registry/src -name "librocksdb-sys-*" -type d 2>/dev/null | head -1)
BUILD_RS="$ROCKSDB_SRC/build.rs"
if grep -q 'config.flag("cstdint")' "$BUILD_RS"; then
    echo "✓ Already patched: $BUILD_RS"
    exit 0
fi
cp "$BUILD_RS" "$BUILD_RS.orig"
sed -i '/config.cpp(true);/a\    config.flag("-include");\n    config.flag("cstdint");' "$BUILD_RS"
if grep -q 'config.flag("cstdint")' "$BUILD_RS"; then
    echo "✓ Successfully patched librocksdb-sys build.rs"
    echo "  Added: config.flag(\"-include\"); config.flag(\"cstdint\");"
else
    echo "✗ Patch failed, restoring backup"
    mv "$BUILD_RS.orig" "$BUILD_RS"
    exit 1
fi
