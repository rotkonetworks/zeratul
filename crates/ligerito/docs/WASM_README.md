# Ligerito WASM

Run Ligerito polynomial commitments in the browser!

## Features

- ✅ **Zero-knowledge proofs in your browser**
- ✅ **No trusted setup required**
- ✅ **Small WASM bundle** (~500KB compressed)
- ✅ **Fast verification** (~20-50ms for 2^12)
- ✅ **Works with all major bundlers** (webpack, vite, rollup)

## Quick Start

### Installation

```bash
npm install @zeratul/ligerito-wasm
```

Or build from source:

```bash
cd crates/ligerito
./build-wasm.sh
```

### Usage (ES Modules)

```javascript
import init, { prove, verify } from '@zeratul/ligerito-wasm';

async function main() {
  // Initialize WASM module
  await init();

  // Generate a random polynomial (2^12 = 4096 elements)
  const polynomial = new Uint32Array(4096);
  for (let i = 0; i < polynomial.length; i++) {
    polynomial[i] = Math.floor(Math.random() * 0xFFFFFFFF);
  }

  // Generate proof
  console.time('prove');
  const proof = prove(polynomial, 12); // config_size = 12 means 2^12
  console.timeEnd('prove');
  console.log(`Proof size: ${proof.length} bytes`);

  // Verify proof
  console.time('verify');
  const isValid = verify(proof, 12);
  console.timeEnd('verify');
  console.log(`Proof is valid: ${isValid}`);
}

main();
```

### Usage (Webpack/Vite)

```javascript
import init, { prove, verify } from '@zeratul/ligerito-wasm/bundler';

// Rest is the same as above
```

### Usage (Node.js)

```javascript
const { prove, verify } = require('@zeratul/ligerito-wasm/nodejs');

// Rest is the same (but sync, no need for await init())
```

## API Reference

### `init(): Promise<void>`

Initialize the WASM module. Must be called before using other functions (ES modules only).

### `prove(polynomial: Uint32Array, config_size: number): Uint8Array`

Generate a Ligerito proof.

**Parameters:**
- `polynomial`: Polynomial coefficients as `Uint32Array`
  - Size must be exactly `2^config_size` elements
  - Each element is a GF(2^32) field element (u32)
- `config_size`: Log2 of polynomial size
  - Supported: 12 (4KB), 20 (4MB), 24 (64MB)

**Returns:**
- Serialized proof as `Uint8Array` (~6-35 KB depending on size)

**Example:**
```javascript
const polynomial = new Uint32Array(4096); // 2^12
// Fill polynomial with your data...
const proof = prove(polynomial, 12);
```

### `verify(proof: Uint8Array, config_size: number): boolean`

Verify a Ligerito proof.

**Parameters:**
- `proof`: Proof bytes from `prove()`
- `config_size`: Same size used for proving (12, 20, or 24)

**Returns:**
- `true` if proof is valid, `false` otherwise

**Example:**
```javascript
const isValid = verify(proofBytes, 12);
if (isValid) {
  console.log('Proof verified! ✓');
}
```

### `get_polynomial_size(config_size: number): number`

Get the required polynomial size for a given config.

**Parameters:**
- `config_size`: 12, 20, or 24

**Returns:**
- Number of elements (2^config_size)

**Example:**
```javascript
const size = get_polynomial_size(12); // Returns 4096
const polynomial = new Uint32Array(size);
```

## Supported Configurations

| Config Size | Elements | Polynomial Data | Proof Size | Prove Time* | Verify Time* |
|-------------|----------|-----------------|------------|-------------|--------------|
| 12 | 4,096 | 16 KB | ~6.5 KB | ~50ms | ~20ms |
| 20 | 1,048,576 | 4 MB | ~6.5 KB | ~1-2s | ~470ms |
| 24 | 16,777,216 | 64 MB | ~35 KB | ~10-20s | ~470ms |

*Times are approximate for Chrome on M1 Mac. May vary by browser and hardware.

## Browser Demo

See `examples/wasm_demo.html` for a complete interactive demo.

To run:

```bash
cd crates/ligerito
./build-wasm.sh
python3 -m http.server 8000
# Open http://localhost:8000/examples/wasm_demo.html
```

## Use Cases

### Privacy-Preserving Transactions

```javascript
// Client generates proof of valid state transition
const witness = serializeTransfer({
  sender_old: { balance: 100, nonce: 5 },
  receiver_old: { balance: 50, nonce: 3 },
  amount: 10,
  // ... commitments, salts
});

const polynomial = witnessToPolynomial(witness);
const proof = prove(polynomial, 20);

// Submit proof to blockchain
await submitTransaction({
  proof,
  commitments: extractCommitments(witness),
});
```

### Batch Proofs

```javascript
// Aggregate multiple transactions into one proof
const transactions = [tx1, tx2, tx3, ...];
const batchPolynomial = aggregateTransactions(transactions);
const batchProof = prove(batchPolynomial, 24);

// One proof for many transactions!
console.log(`Proved ${transactions.length} txs in ${batchProof.length} bytes`);
```

## Performance Tips

1. **Use smaller configs for demos** (12 is fast)
2. **Use Web Workers** for proving (don't block UI)
3. **Cache the WASM module** (it's ~500KB)
4. **Batch transactions** when possible
5. **Use native verification** on servers (faster than WASM)

## Web Worker Example

```javascript
// worker.js
import init, { prove } from '@zeratul/ligerito-wasm';

await init();

self.onmessage = async (e) => {
  const { polynomial, configSize } = e.data;
  try {
    const proof = prove(polynomial, configSize);
    self.postMessage({ proof });
  } catch (error) {
    self.postMessage({ error: error.message });
  }
};

// main.js
const worker = new Worker('worker.js', { type: 'module' });

worker.postMessage({ polynomial, configSize: 12 });
worker.onmessage = (e) => {
  if (e.data.error) {
    console.error('Proving failed:', e.data.error);
  } else {
    console.log('Proof generated!', e.data.proof);
  }
};
```

## Building from Source

Prerequisites:
```bash
cargo install wasm-pack
```

Build:
```bash
cd crates/ligerito
./build-wasm.sh
```

This generates:
- `pkg/web/` - ES modules for `<script type="module">`
- `pkg/bundler/` - For webpack/vite/rollup
- `pkg/nodejs/` - For Node.js

## Bundle Size

Compressed sizes (gzip):
- WASM binary: ~450 KB
- JavaScript glue: ~15 KB
- **Total: ~465 KB**

Comparable to many image files, totally acceptable for modern web apps!

## Browser Support

- ✅ Chrome 57+
- ✅ Firefox 52+
- ✅ Safari 11+
- ✅ Edge 16+

Basically all modern browsers with WebAssembly support.

## Troubleshooting

### "Memory access out of bounds"

Your polynomial size doesn't match `config_size`. Use `get_polynomial_size()` to get the correct size.

### "Optimization level not supported"

Make sure you're using a recent version of Rust:
```bash
rustup update
```

### Slow performance in development

Build in release mode:
```bash
wasm-pack build --release
```

## License

MIT / Apache-2.0 (same as main Ligerito crate)

## Links

- [Ligerito Paper](https://angeris.github.io/papers/ligerito.pdf)
- [GitHub Repository](https://github.com/your-org/zeratul)
- [API Documentation](https://docs.rs/ligerito)
