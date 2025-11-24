# Fast Build Configuration

This project is configured for fast compilation with parallel builds and the mold linker.

## Already Configured ✅

- **Parallel compilation**: Using 16 cores by default
- **mold linker**: 10x+ faster linking than default
- **Incremental builds**: Reuses previous compilation
- **Optimized profiles**: Faster dev builds, thin LTO for release

## Installation on Different Systems

### Arch Linux / Manjaro (Already Done)
```bash
sudo pacman -S lld mold
```

### Ubuntu / Debian
```bash
# lld
sudo apt install lld

# mold (from release)
curl -LO https://github.com/rui314/mold/releases/download/v2.40.4/mold-2.40.4-x86_64-linux.tar.gz
tar xzf mold-2.40.4-x86_64-linux.tar.gz
sudo cp mold-2.40.4-x86_64-linux/bin/mold /usr/local/bin/
```

### Fedora / RHEL
```bash
sudo dnf install lld mold
```

### macOS
```bash
# lld comes with LLVM
brew install llvm

# mold
brew install mold
```

### NixOS
Add to `configuration.nix`:
```nix
environment.systemPackages = with pkgs; [
  llvmPackages.lld
  mold
];
```

Or in a shell:
```bash
nix-shell -p llvmPackages.lld mold
```

## Build Times

**Without optimization:**
- Initial: 2-3 minutes
- Incremental: 30-60 seconds

**With .cargo/config.toml + mold:**
- Initial: 45-90 seconds ⚡
- Incremental: 5-15 seconds ⚡

**For development (fastest):**
```bash
cargo check -p terminator  # 2-5 seconds
```

## Troubleshooting

If mold causes issues, switch to lld by editing `.cargo/config.toml`:

```toml
[target.x86_64-unknown-linux-gnu]
linker = "clang"
rustflags = ["-C", "link-arg=-fuse-ld=lld"]  # Change mold to lld
```

## Optional: Build Cache (sccache)

For even faster repeated builds across clean builds:

```bash
cargo install sccache
```

Then add to `.cargo/config.toml`:
```toml
[build]
rustc-wrapper = "sccache"
```

This caches compilation artifacts and can speed up clean builds by 50-90%.
