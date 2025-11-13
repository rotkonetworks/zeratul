# Ligerito Verifier for PolkaVM/CoreVM

This example demonstrates how to build the Ligerito verifier for PolkaVM/CoreVM environments.

## What gets built?

The build process creates a **`.polkavm`** or **`.corevm`** binary file that can run on PolkaVM/CoreVM:

1. **Cargo builds** Rust code → RISC-V64 ELF binary using custom target JSON
2. **polkatool link** converts ELF → final `.polkavm`/`.corevm` format

## Build Process

### Step 1: Activate polkaports environment

```bash
# From the polkaports directory
cd ../../polkaports
. ./activate.sh polkavm    # For PolkaVM
# OR
. ./activate.sh corevm     # For CoreVM
```

This sets up:
- `POLKAPORTS_SUFFIX` (polkavm or corevm)
- `POLKAPORTS_SYSROOT` (path to sysroot)
- Adds `polkatool` to PATH

### Step 2: Build

```bash
cd examples/polkavm_verifier
make
```

This will:
1. Build Rust code with `-Zbuild-std` for the custom RISC-V target
2. Link with `polkatool` to create final binary
3. Output: `ligerito_verifier.polkavm` or `ligerito_verifier.corevm`

## Build Commands Explained

### Cargo build with custom target:
```bash
RUSTC_BOOTSTRAP=1 cargo build \
  --target=$SYSROOT/riscv64emac-polkavm-linux-musl.json \
  -Zbuild-std=core,alloc,std,panic_abort \
  -Zbuild-std-features=panic_immediate_abort \
  --release
```

- `RUSTC_BOOTSTRAP=1`: Enables unstable features
- `--target=...json`: Uses custom RISC-V target spec from polkaports
- `-Zbuild-std`: Rebuilds std library for the target
- Result: RISC-V ELF binary

### polkatool link:
```bash
polkatool link \
  --min-stack-size 16384 \
  target/riscv64emac-polkavm-linux-musl/release/polkavm_verifier \
  -o ligerito_verifier.polkavm
```

- Converts RISC-V ELF → PolkaVM format
- Sets minimum stack size
- Result: Final `.polkavm` binary ready to run

## Features Used

- `std`: Uses Rust std library (available via musl libc)
- `verifier-only`: Excludes prover code (smaller binary)
- **NO** `parallel`: Threading not supported in PolkaVM

## Target Architecture

- **ISA**: RISC-V64 (`rv64emac_zbb_xtheadcondmov`)
- **ABI**: `lp64e` (reduced register set)
- **OS**: `linux` (via musl libc)
- **Threading**: Single-threaded only (`"singlethread": true`)
- **Linking**: Position-independent (PIE)

## File Structure

```
examples/polkavm_verifier/
├── Cargo.toml           # Package config (standalone binary)
├── main.rs              # Verifier implementation
├── Makefile             # Build automation
└── README.md            # This file
```

## Binary Size

Expected sizes:
- RISC-V ELF: ~3-4 MB
- Final `.polkavm`: Depends on polkatool compression

## Deployment

The final `.polkavm` or `.corevm` binary can be:
1. Loaded into PolkaVM/CoreVM runtime
2. Executed with proof data passed via memory/syscalls
3. Result returned via exit code or host function calls

## Key Differences: PolkaVM vs CoreVM

### Both support:
- Rust std library (via musl)
- File I/O, console output
- Single-threaded execution

### CoreVM adds:
- Gas metering (`corevm_gas()`)
- Custom allocator (`corevm_alloc()`, `corevm_free()`)
- Video/audio host functions
- Console stream differentiation

## Limitations

### What works:
- ✅ std library (Vec, String, HashMap, etc.)
- ✅ println!/eprintln!
- ✅ File I/O (basic operations)
- ✅ SHA256, serialization
- ✅ Sequential verification

### What doesn't work:
- ❌ Threading (`std::thread::spawn`)
- ❌ Rayon/parallel features
- ❌ Tokio/async runtime
- ❌ `pthread_create` (returns ENOSYS)

**Why?** The `clone` syscall is unimplemented (`unimp` instruction) in polkaports.

## Example: Using the verifier

```rust
use ligerito::{verify, hardcoded_config_20_verifier};

fn main() {
    // Load proof from memory/syscall
    let proof_bytes = load_proof_from_host();

    // Deserialize
    let proof: FinalizedLigeritoProof<BinaryElem32, BinaryElem128> =
        bincode::deserialize(&proof_bytes).unwrap();

    // Verify
    let config = hardcoded_config_20_verifier();
    let is_valid = verify(&config, &proof).unwrap();

    // Return result
    std::process::exit(if is_valid { 0 } else { 1 });
}
```

## Troubleshooting

### Error: "Please activate polkaports first"
```bash
. ../../polkaports/activate.sh polkavm
```

### Error: "polkatool: command not found"
Check that `$POLKAPORTS_SYSROOT/bin/polkatool` exists and PATH is set correctly.

### Error: "failed to resolve: use of undeclared crate or module 'std'"
Make sure `-Zbuild-std` is used when building.

## Development

### Quick test (native build):
```bash
cargo build --release
cargo run --release
```

### Build for PolkaVM:
```bash
. ../../polkaports/activate.sh polkavm
make
```

### Clean:
```bash
make clean
```
