# Ligerito CLI Usage Guide

The Ligerito CLI provides a complete command-line interface for polynomial commitments, supporting proving, verification, and data generation.

## Installation

```bash
cargo install --path crates/ligerito --features cli
```

Or build from source:
```bash
cargo build --release --features cli
./target/release/ligerito --help
```

## Commands

### 1. Generate Test Data

Create polynomial data for testing:

```bash
# Generate random 2^20 polynomial
ligerito generate --size 20 --output poly.bin

# Generate sequential pattern
ligerito generate --size 16 --pattern sequential --output poly.bin

# Generate zeros
ligerito generate --size 12 --pattern zeros > zeros.bin
```

**Patterns:**
- `random` (default): Random values
- `zeros`: All zeros
- `ones`: All ones
- `sequential`: 0, 1, 2, 3, ...

### 2. Create Proofs

Generate polynomial commitment proofs:

```bash
# From file
cat poly.bin | ligerito prove --size 20 > proof.bin

# From generated data (pipe)
ligerito generate --size 20 | ligerito prove --size 20 > proof.bin

# Hex output (for text-based storage)
cat poly.bin | ligerito prove --size 20 --format hex > proof.hex
```

**Supported sizes:** 12, 16, 20, 24, 28, 30 (log2 of polynomial length)

### 3. Verify Proofs

Verify polynomial commitment proofs:

```bash
# Verify binary proof
cat proof.bin | ligerito verify --size 20
echo $?  # 0 if valid, 1 if invalid

# Verify hex proof
cat proof.hex | ligerito verify --size 20 --format hex

# Verbose output
cat proof.bin | ligerito verify --size 20 --verbose
```

### 4. View Configuration

Show configuration details for a given size:

```bash
ligerito config --size 20
```

Output:
```
Polynomial elements: 2^20 = 1048576
Recursive steps: 3
Initial k: 7
Recursive ks: [6]
Log dimensions: [(13, 7), (7, 6)]

Estimated sizes:
  Polynomial: 4194304 bytes (4.00 MB)
```

## Complete Workflows

### End-to-End Proof Generation and Verification

```bash
# 1. Generate test polynomial
ligerito generate --size 20 --pattern random --output poly.bin

# 2. Create proof
cat poly.bin | ligerito prove --size 20 --format bincode > proof.bin

# 3. Verify proof
cat proof.bin | ligerito verify --size 20 --verbose
```

### One-Liner Roundtrip

```bash
ligerito generate --size 16 | ligerito prove --size 16 | ligerito verify --size 16
```

### Store Proofs in Multiple Formats

```bash
# Binary (compact)
cat poly.bin | ligerito prove --size 20 --format bincode > proof.bin

# Hex (text-friendly)
cat poly.bin | ligerito prove --size 20 --format hex > proof.hex
```

## File Formats

### Polynomial Data Format

Raw binary format: consecutive 4-byte little-endian u32 values representing BinaryElem32 field elements.

Example for 2^12 polynomial:
- Size: 4096 elements × 4 bytes = 16,384 bytes
- Format: `[u32_le, u32_le, u32_le, ...]`

### Proof Format

- **Bincode** (default): Binary serialization using bincode
- **Hex**: Hex-encoded bincode for text storage/transmission

## Performance Characteristics

| Size | Elements | Poly Size | Prove Time | Verify Time | Proof Size |
|------|----------|-----------|------------|-------------|------------|
| n=12 | 4K       | 16 KB     | ~5 ms      | ~2 ms       | ~10 KB     |
| n=16 | 65K      | 256 KB    | ~50 ms     | ~15 ms      | ~40 KB     |
| n=20 | 1M       | 4 MB      | ~400 ms    | ~70 ms      | ~145 KB    |
| n=24 | 16M      | 64 MB     | ~4.7 s     | ~1.2 s      | ~238 KB    |
| n=28 | 268M     | 1 GB      | ~115 s     | ~20 s       | ~357 KB    |

*(Benchmarks: AMD Ryzen 7 8845HS, k=6 configuration)*

## Integration Examples

### Shell Scripts

```bash
#!/bin/bash
# proof-pipeline.sh

SIZE=20

echo "Generating polynomial..."
ligerito generate --size $SIZE --output data.bin

echo "Creating proof..."
cat data.bin | ligerito prove --size $SIZE > proof.bin

echo "Verifying proof..."
if cat proof.bin | ligerito verify --size $SIZE; then
    echo "✓ Proof verified successfully"
else
    echo "✗ Proof verification failed"
    exit 1
fi
```

### Python Integration

```python
import subprocess
import sys

def create_proof(poly_data: bytes, size: int) -> bytes:
    """Generate Ligerito proof from polynomial data"""
    result = subprocess.run(
        ['ligerito', 'prove', '--size', str(size)],
        input=poly_data,
        capture_output=True,
        check=True
    )
    return result.stdout

def verify_proof(proof: bytes, size: int) -> bool:
    """Verify Ligerito proof"""
    result = subprocess.run(
        ['ligerito', 'verify', '--size', str(size)],
        input=proof,
        capture_output=True
    )
    return result.returncode == 0

# Example usage
poly = subprocess.run(
    ['ligerito', 'generate', '--size', '16'],
    capture_output=True,
    check=True
).stdout

proof = create_proof(poly, 16)
is_valid = verify_proof(proof, 16)
print(f"Proof valid: {is_valid}")
```

### JavaScript/Node.js Integration

```javascript
const { spawn } = require('child_process');
const fs = require('fs');

async function createProof(polyFile, size) {
  return new Promise((resolve, reject) => {
    const prove = spawn('ligerito', ['prove', '--size', size]);
    const poly = fs.createReadStream(polyFile);

    let proof = Buffer.alloc(0);
    prove.stdout.on('data', chunk => {
      proof = Buffer.concat([proof, chunk]);
    });

    prove.on('close', code => {
      if (code === 0) resolve(proof);
      else reject(new Error(`Proving failed with code ${code}`));
    });

    poly.pipe(prove.stdin);
  });
}

async function verifyProof(proof, size) {
  return new Promise((resolve) => {
    const verify = spawn('ligerito', ['verify', '--size', size]);

    verify.on('close', code => resolve(code === 0));
    verify.stdin.write(proof);
    verify.stdin.end();
  });
}

// Usage
(async () => {
  const proof = await createProof('poly.bin', '20');
  const valid = await verifyProof(proof, '20');
  console.log(`Proof valid: ${valid}`);
})();
```

## Error Handling

Common errors and solutions:

### Size Mismatch
```
Error: Expected 4194304 bytes (1048576 elements of 4 bytes), got 1000
```
**Solution:** Ensure polynomial file matches the specified size (2^n elements × 4 bytes)

### Invalid Size
```
Error: Unsupported size: 15. Must be 12, 16, 20, 24, 28, or 30
```
**Solution:** Use supported power-of-2 sizes only

### Verification Failed
```
✗ Proof verification failed
```
**Solution:** Proof may be corrupted or generated with different parameters

## Advanced Usage

### Custom Input Sources

```bash
# From network
curl https://example.com/poly.bin | ligerito prove --size 20 > proof.bin

# From process substitution
ligerito prove --size 16 < <(ligerito generate --size 16)

# From device (1MB of random data as n=18 polynomial)
head -c 1048576 /dev/urandom | ligerito prove --size 18 > proof.bin
```

### Batch Processing

```bash
# Process multiple polynomials
for size in 12 16 20; do
  echo "Processing size $size..."
  ligerito generate --size $size | \
    ligerito prove --size $size > "proof_${size}.bin"
done
```

### Proof Size Comparison

```bash
# Compare proof sizes across configurations
for size in 12 16 20 24; do
  proof_size=$(ligerito generate --size $size | \
               ligerito prove --size $size | wc -c)
  echo "n=$size: $proof_size bytes"
done
```

## Future Features (TODO)

- [ ] ZODA encoding for arbitrary data
- [ ] Custom configuration files (BYOC - Bring Your Own Config)
- [ ] Streaming mode for large polynomials
- [ ] GPU acceleration flag (--gpu)
- [ ] WebAssembly compilation target
- [ ] Batch proof generation

## See Also

- [Ligerito Paper](https://angeris.github.io/papers/ligerito.pdf)
- [API Documentation](https://docs.rs/ligerito)
- [Web Demo](../examples/www/)
- [Benchmark Results](BENCHMARK_RESULTS.md)

## Transcript Backends

Ligerito supports three cryptographic transcript implementations:

### SHA256 (Default)
- **No external dependencies**
- Works in `no_std` environments
- Works in WASM/browser
- Good performance
- **Current default for CLI**

### Merlin
- Zcash/Dalek ecosystem standard
- Requires `merlin` crate dependency
- Available with `--features transcript-merlin`

### BLAKE3
- Fastest hashing performance
- Requires `blake3` crate dependency  
- Available with `--features transcript-blake3`

### Important Notes

1. **Prover and verifier MUST use the same transcript backend**
2. **Proofs have identical SIZE but different CONTENTS** with different transcripts
3. **Runtime selection not yet implemented** - currently always uses SHA256
4. To use a different backend, rebuild with specific features:

```bash
# Build with Merlin transcript
cargo build --release --features "cli,transcript-merlin" --no-default-features

# Build with BLAKE3 transcript  
cargo build --release --features "cli,transcript-blake3" --no-default-features
```

### Proof Characteristics

All transcript backends produce:
- **Same proof size** (~147 KB for n=20 with k=6)
- **Different proof bytes** (transcript affects Fiat-Shamir challenges)
- **Deterministic proofs** (same input + same transcript = identical proof)

The `--transcript` flag exists in the CLI but currently has no effect. This will be implemented in a future version for runtime selection.
