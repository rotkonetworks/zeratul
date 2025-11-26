// Test WASM prove + verify roundtrip
import { readFileSync, writeFileSync } from 'fs';

const wasmBytes = readFileSync('./ligerito_raw.wasm');

// Load WASM
const module = await WebAssembly.compile(wasmBytes);
const instance = await WebAssembly.instantiate(module, {});

const {
  memory,
  wasm_alloc,
  wasm_dealloc,
  prove_raw,
  verify_raw,
  get_polynomial_size,
  result_status,
  result_len,
  result_data_ptr,
  result_free,
  set_random_seed
} = instance.exports;

const configSize = 12;
const polySize = get_polynomial_size(configSize);
console.log('Polynomial size:', polySize, 'elements');

// Seed RNG
const seedPtr = wasm_alloc(32);
const seedBytes = new Uint8Array(memory.buffer, seedPtr, 32);
for (let i = 0; i < 32; i++) seedBytes[i] = Math.floor(Math.random() * 256);
set_random_seed(seedPtr);
wasm_dealloc(seedPtr, 32);

// Generate random polynomial
const polyPtr = wasm_alloc(polySize * 4);
const polyU32 = new Uint32Array(memory.buffer, polyPtr, polySize);
for (let i = 0; i < polySize; i++) {
  polyU32[i] = Math.floor(Math.random() * 0xFFFFFFFF);
}

console.log('Polynomial preview:', Array.from(polyU32.slice(0, 4)).map(x => '0x' + x.toString(16).padStart(8, '0')).join(' '));

// Prove
console.log('Proving in WASM...');
const proveStart = performance.now();
const resultPtr = prove_raw(polyPtr, polySize, configSize);
const proveTime = performance.now() - proveStart;

const status = result_status(resultPtr);
const dataLen = result_len(resultPtr);
const dataPtr = result_data_ptr(resultPtr);

if (status !== 0) {
  const errBytes = new Uint8Array(memory.buffer, dataPtr, dataLen);
  const errMsg = new TextDecoder().decode(errBytes);
  console.error('Proving failed:', errMsg);
  result_free(resultPtr);
  wasm_dealloc(polyPtr, polySize * 4);
  process.exit(1);
}

// Copy proof
const wasmProof = new Uint8Array(memory.buffer, dataPtr, dataLen).slice();
console.log('Proof size:', wasmProof.length, 'bytes');
console.log('Prove time:', proveTime.toFixed(2), 'ms');

result_free(resultPtr);
wasm_dealloc(polyPtr, polySize * 4);

// Save WASM-generated proof for comparison
writeFileSync('wasm-proof-12.proof', wasmProof);
console.log('Saved WASM proof to wasm-proof-12.proof');

// Verify WASM proof in WASM
const proofPtr = wasm_alloc(wasmProof.length);
new Uint8Array(memory.buffer, proofPtr, wasmProof.length).set(wasmProof);

console.log('\nVerifying WASM-generated proof...');
const verifyStart = performance.now();
const result = verify_raw(proofPtr, wasmProof.length, configSize);
const verifyTime = performance.now() - verifyStart;

console.log('WASM verify result:', result, '(0=invalid, 1=valid, 2=error)');
console.log('Verify time:', verifyTime.toFixed(2), 'ms');

wasm_dealloc(proofPtr, wasmProof.length);

// Now try to verify CLI proof
const cliProofBuf = readFileSync('./test-proof-12.proof');
const cliProof = new Uint8Array(cliProofBuf);

const cliProofPtr = wasm_alloc(cliProof.length);
new Uint8Array(memory.buffer, cliProofPtr, cliProof.length).set(cliProof);

console.log('\nVerifying CLI-generated proof...');
const cliVerifyStart = performance.now();
const cliResult = verify_raw(cliProofPtr, cliProof.length, configSize);
const cliVerifyTime = performance.now() - cliVerifyStart;

console.log('CLI proof verify result:', cliResult, '(0=invalid, 1=valid, 2=error)');
console.log('Verify time:', cliVerifyTime.toFixed(2), 'ms');

wasm_dealloc(cliProofPtr, cliProof.length);

// Compare proof sizes
console.log('\n=== Proof comparison ===');
console.log('WASM proof size:', wasmProof.length, 'bytes');
console.log('CLI proof size:', cliProof.length, 'bytes');
console.log('First 32 bytes WASM:', Array.from(wasmProof.slice(0, 32)).map(b => b.toString(16).padStart(2, '0')).join(' '));
console.log('First 32 bytes CLI:', Array.from(cliProof.slice(0, 32)).map(b => b.toString(16).padStart(2, '0')).join(' '));
