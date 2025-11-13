# Makefile for building Ligerito verifier for PolkaVM/CoreVM
#
# Prerequisites:
# 1. Activate polkaports environment FIRST:
#    . ../../polkaports/activate.sh corevm
#    (or polkavm for PolkaVM target)
#
# Build:
#   make              # Build for CoreVM
#   make polkavm      # Build for PolkaVM
#
# Clean:
#   make clean

.PHONY: all clean polkavm corevm info

# Detect which environment is activated
SUFFIX ?= $(POLKAPORTS_SUFFIX)
SYSROOT ?= $(POLKAPORTS_SYSROOT)

# Default target
all: check-env ligerito_verifier.$(SUFFIX)

# Check if polkaports environment is activated
check-env:
ifndef POLKAPORTS_SUFFIX
	$(error Please activate polkaports first: . ../../polkaports/activate.sh corevm)
endif
	@echo "Building for: $(SUFFIX)"
	@echo "Using sysroot: $(SYSROOT)"

# Build for CoreVM specifically
corevm:
	$(MAKE) SUFFIX=corevm build-rust

# Build for PolkaVM specifically
polkavm:
	$(MAKE) SUFFIX=polkavm build-rust

# Build the Rust binary using Cargo with custom target
build-rust:
	@echo "Building Ligerito verifier for $(SUFFIX)..."
	RUSTC_BOOTSTRAP=1 cargo build \
		--target=$(SYSROOT)/riscv64emac-$(SUFFIX)-linux-musl.json \
		-Zbuild-std=core,alloc,std,panic_abort \
		-Zbuild-std-features=panic_immediate_abort \
		--release
	@echo "Rust binary built at: target/riscv64emac-$(SUFFIX)-linux-musl/release/polkavm_verifier"

# Link into final PolkaVM/CoreVM binary
ligerito_verifier.$(SUFFIX): build-rust
	@echo "Linking final $(SUFFIX) binary..."
	$(SYSROOT)/bin/polkatool link \
		--min-stack-size 16384 \
		target/riscv64emac-$(SUFFIX)-linux-musl/release/polkavm_verifier \
		-o ligerito_verifier.$(SUFFIX)
	@echo "✓ Final binary created: ligerito_verifier.$(SUFFIX)"
	@ls -lh ligerito_verifier.$(SUFFIX)

clean:
	rm -f ligerito_verifier.elf ligerito_verifier.polkavm
	cargo clean --manifest-path ../../ligerito/Cargo.toml

# Alternative: Build as library with FFI
ffi:
	cargo build --manifest-path ../../ligerito/Cargo.toml \
		--release \
		--lib \
		--features="std,verifier-only,ffi"
	@echo "FFI library built at: ../../target/release/libligerito.so"

# Test build (quick sanity check)
test:
	cargo build --manifest-path ../../ligerito/Cargo.toml \
		--features="std,verifier-only" \
		--example polkavm_verifier
	@echo "Test build successful!"

# Show build info
info:
	@echo "Ligerito PolkaVM Verifier Build Info"
	@echo "===================================="
	@echo "Features: std, verifier-only"
	@echo "Target: RISC-V (via PolkaVM SDK)"
	@echo ""
	@echo "To build for PolkaVM:"
	@echo "  1. Activate polkaports: . ../../../polkaports/activate.sh corevm"
	@echo "  2. Run: make"
	@echo ""
	@echo "For native testing:"
	@echo "  make test"
