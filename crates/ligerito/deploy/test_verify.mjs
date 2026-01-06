// Test WASM verification directly
import { readFileSync } from 'fs';

const wasmBytes = readFileSync('./ligerito_raw.wasm');
const proofBuf = readFileSync('./test-proof-12.proof');
const proofBytes = new Uint8Array(proofBuf);

console.log('Proof size:', proofBytes.length, 'bytes');

// Load WASM
const module = await WebAssembly.compile(wasmBytes);
const instance = await WebAssembly.instantiate(module, {});

const { memory, wasm_alloc, wasm_dealloc, verify_raw, get_polynomial_size } = instance.exports;

console.log('Expected size for config 12:', get_polynomial_size(12));

// Allocate and copy proof
const proofPtr = wasm_alloc(proofBytes.length);
console.log('Allocated proof at:', proofPtr);

const wasmMem = new Uint8Array(memory.buffer, proofPtr, proofBytes.length);
wasmMem.set(proofBytes);

// Verify
console.log('Calling verify_raw...');
const start = performance.now();
const result = verify_raw(proofPtr, proofBytes.length, 12);
const elapsed = performance.now() - start;

console.log('Result:', result, '(0=invalid, 1=valid, 2=error)');
console.log('Time:', elapsed.toFixed(2), 'ms');

// Cleanup
wasm_dealloc(proofPtr, proofBytes.length);
