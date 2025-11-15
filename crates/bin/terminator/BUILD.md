# Building Terminator

## Quick Build

```bash
./build.sh
```

That's it! The script handles all the environment setup.

## Manual Build

### 1. Set Environment Variable

```bash
export ROCKSDB_LIB_DIR=/usr/lib64
```

### 2. Build

```bash
cargo build --release
```

### 3. Run

```bash
./target/release/terminator
```

## Why ROCKSDB_LIB_DIR?

### The Problem

Penumbra's view database uses RocksDB for storage. The Rust crate `librocksdb-sys` has build issues with newer versions of GCC/Clang that cause compilation errors like:

```
config.flag("-include").flag("cstdint");
```

### The Solution (Two Options)

#### Option 1: Use System RocksDB (Recommended)

Setting `ROCKSDB_LIB_DIR=/usr/lib64` tells `librocksdb-sys` to use the system's RocksDB library instead of building from source.

**However**, this still requires `libclang` for bindgen. The build process is:
1. Even with system rocksdb, bindgen runs to generate FFI bindings
2. Bindgen needs libclang to parse C++ headers
3. Install clang/llvm development packages

#### Option 2: Wait for upstream fix

The `librocksdb-sys` crate maintainers are aware of this issue with newer compilers. A fix involving the `-include cstdint` flag is being deployed.

### System Requirements

You need both RocksDB AND clang/llvm development packages:

#### Arch Linux
```bash
sudo pacman -S rocksdb clang
```

#### Ubuntu/Debian
```bash
sudo apt install librocksdb-dev libclang-dev
```

#### Fedora
```bash
sudo dnf install rocksdb-devel clang-devel
```

#### macOS
```bash
brew install rocksdb llvm
```

### Current Build Status

⚠️ **Note:** The build currently requires `libclang` even with `ROCKSDB_LIB_DIR` set, due to bindgen generating FFI bindings. This is a known issue with the current version of `librocksdb-sys`.

The upstream Penumbra repository may have updates to address this. Check:
- https://github.com/penumbra-zone/penumbra/issues
- rocksdb-rs/rust-rocksdb repository for fixes

## Build Modes

### Development Build

```bash
export ROCKSDB_LIB_DIR=/usr/lib64
cargo build
```

Faster compilation, larger binary, includes debug symbols.

### Release Build

```bash
export ROCKSDB_LIB_DIR=/usr/lib64
cargo build --release
```

Slower compilation, smaller binary, optimized for performance.

**Always use release mode for actual trading!**

## Troubleshooting

### "Unable to find libclang"

```
error: failed to run custom build command for `librocksdb-sys v0.17.3+10.4.2`

Caused by:
  process didn't exit successfully

  Unable to find libclang
```

**Solution:** Set `ROCKSDB_LIB_DIR`:
```bash
export ROCKSDB_LIB_DIR=/usr/lib64
cargo build --release
```

### "cannot find -lrocksdb"

```
error: linking with `cc` failed

  /usr/bin/ld: cannot find -lrocksdb
```

**Solution:** Install RocksDB:
```bash
# Arch
sudo pacman -S rocksdb

# Ubuntu
sudo apt install librocksdb-dev
```

### Wrong RocksDB Path

If `/usr/lib64` doesn't exist on your system, find the correct path:

```bash
# Find rocksdb library
find /usr -name "librocksdb.so*" 2>/dev/null

# Common locations:
# - /usr/lib64/librocksdb.so (Fedora, Arch)
# - /usr/lib/x86_64-linux-gnu/librocksdb.so (Ubuntu)
# - /usr/local/lib/librocksdb.so (macOS with brew)
```

Then use that directory:
```bash
export ROCKSDB_LIB_DIR=/usr/lib/x86_64-linux-gnu  # Ubuntu
# or
export ROCKSDB_LIB_DIR=/usr/local/lib  # macOS
```

### Build Takes Too Long

Release builds are slow (5-10 minutes) because of:
- RocksDB
- Cryptography libraries (ark-groth16, decaf377)
- Penumbra SDK

**Tips:**
1. Use `cargo build` for development (faster)
2. Use `cargo build --release` only for running
3. Use `cargo check` for quick validation (no code generation)

### Out of Memory

If build fails with OOM:

```bash
# Reduce parallel compilation
CARGO_BUILD_JOBS=2 cargo build --release
```

## Permanent Setup

Add to your shell profile (`~/.bashrc`, `~/.zshrc`, etc.):

```bash
# Terminator / Penumbra build config
export ROCKSDB_LIB_DIR=/usr/lib64
```

Then:
```bash
source ~/.bashrc  # or restart terminal
cargo build --release
```

## Build Script Details

The `build.sh` script does:

```bash
#!/bin/bash
set -e  # Exit on error

echo "Building Terminator..."
echo "Note: Using system rocksdb library"

export ROCKSDB_LIB_DIR=/usr/lib64
cargo build --release "$@"

echo "✓ Build complete!"
echo "  Binary: target/release/terminator"
```

You can pass extra args:
```bash
./build.sh --features some-feature
./build.sh --verbose
```

## Cross-Compilation

For other architectures:

```bash
# Add target
rustup target add aarch64-unknown-linux-gnu

# Set rocksdb for target
export ROCKSDB_LIB_DIR_aarch64_unknown_linux_gnu=/path/to/aarch64/rocksdb

# Build
cargo build --release --target aarch64-unknown-linux-gnu
```

## Clean Build

If you encounter weird issues:

```bash
# Clean build artifacts
cargo clean

# Rebuild
./build.sh
```

## Verification

After building, verify the binary:

```bash
# Check it exists
ls -lh target/release/terminator

# Check dependencies
ldd target/release/terminator | grep rocksdb

# Run it
./target/release/terminator --version  # (once we add --version flag)
```

## Performance

**Release vs Debug:**

| Mode | Build Time | Binary Size | Runtime Speed |
|------|-----------|-------------|---------------|
| Debug | ~2 min | ~500 MB | Slow |
| Release | ~8 min | ~50 MB | Fast |

**Always use release mode for production trading!**

---

**TL;DR:**
```bash
./build.sh
./target/release/terminator
```
