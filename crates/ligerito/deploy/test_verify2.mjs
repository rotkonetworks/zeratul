// Test using the JS wrapper API
import { readFileSync } from 'fs';
import { init, verify, get_polynomial_size } from './ligerito_raw.js';

const proofBytes = readFileSync('./test-proof-12.proof');
console.log('Proof size:', proofBytes.length, 'bytes');

await init('./ligerito_raw.wasm');
console.log('WASM initialized');

console.log('Expected polynomial size for config 12:', get_polynomial_size(12));

try {
  const result = verify(proofBytes, 12);
  console.log('Verification result:', result ? 'VALID' : 'INVALID');
} catch (e) {
  console.error('Verification error:', e.message);
}
