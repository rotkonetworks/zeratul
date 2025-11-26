// Ligerito Raw WASM Interface
// No wasm-bindgen - pure WebAssembly with manual JS glue
//
// This enables multi-instance parallelism where each worker gets its own WASM instance

let instance = null;
let memory = null;

/**
 * Initialize the raw WASM module
 * @param {string|URL} wasmPath - Path to the .wasm file
 * @returns {Promise<void>}
 */
export async function init(wasmPath = './ligerito_raw.wasm') {
  const response = await fetch(wasmPath);
  const bytes = await response.arrayBuffer();

  // Compile and instantiate - no imports needed with custom getrandom!
  const module = await WebAssembly.compile(bytes);
  instance = await WebAssembly.instantiate(module, {});

  memory = instance.exports.memory;

  // Seed the RNG with cryptographic randomness
  seedRng();

  console.log('[ligerito_raw] WASM module loaded (pure raw WASM, no wasm-bindgen)');
}

/**
 * Seed the WASM RNG with cryptographic randomness
 * Called automatically on init, but can be called again for fresh entropy
 */
export function seedRng() {
  if (!instance) throw new Error('WASM not initialized');

  const exports = instance.exports;

  // Allocate 32 bytes for seed
  const seedPtr = exports.wasm_alloc(32);
  if (seedPtr === 0) throw new Error('Failed to allocate seed memory');

  try {
    // Fill with crypto randomness
    const seedBytes = new Uint8Array(memory.buffer, seedPtr, 32);
    crypto.getRandomValues(seedBytes);

    // Set the seed in WASM
    exports.set_random_seed(seedPtr);
  } finally {
    exports.wasm_dealloc(seedPtr, 32);
  }
}

/**
 * Get expected polynomial size for a config
 * @param {number} configSize - 12, 20, or 24
 * @returns {number} Number of u32 elements
 */
export function get_polynomial_size(configSize) {
  if (!instance) throw new Error('WASM not initialized');
  return instance.exports.get_polynomial_size(configSize);
}

/**
 * Generate a proof from polynomial data
 * @param {Uint32Array} polynomial - Polynomial coefficients
 * @param {number} configSize - 12, 20, or 24
 * @returns {Uint8Array} Proof bytes
 * @throws {Error} If proving fails
 */
export function prove(polynomial, configSize) {
  if (!instance) throw new Error('WASM not initialized');

  const exports = instance.exports;

  // Allocate memory for polynomial (u32 array)
  const polyBytes = polynomial.length * 4;
  const polyPtr = exports.wasm_alloc(polyBytes);
  if (polyPtr === 0) throw new Error('Failed to allocate memory for polynomial');

  try {
    // Copy polynomial to WASM memory
    new Uint32Array(memory.buffer, polyPtr, polynomial.length).set(polynomial);

    // Call prove_raw
    const resultPtr = exports.prove_raw(polyPtr, polynomial.length, configSize);
    if (resultPtr === 0) throw new Error('prove_raw returned null');

    try {
      // Read result
      const status = exports.result_status(resultPtr);
      const dataLen = exports.result_len(resultPtr);
      const dataPtr = exports.result_data_ptr(resultPtr);

      if (status === 0) {
        // Success - copy proof bytes
        const proof = new Uint8Array(memory.buffer, dataPtr, dataLen).slice();
        return proof;
      } else {
        // Error - read error message
        const errorBytes = new Uint8Array(memory.buffer, dataPtr, dataLen);
        const errorMsg = new TextDecoder().decode(errorBytes);
        throw new Error(errorMsg);
      }
    } finally {
      exports.result_free(resultPtr);
    }
  } finally {
    exports.wasm_dealloc(polyPtr, polyBytes);
  }
}

/**
 * Verify a proof
 * @param {Uint8Array} proof - Proof bytes from prove()
 * @param {number} configSize - 12, 20, or 24
 * @returns {boolean} True if valid
 * @throws {Error} If verification encounters an error
 */
export function verify(proof, configSize) {
  if (!instance) throw new Error('WASM not initialized');

  const exports = instance.exports;

  // Allocate memory for proof
  const proofPtr = exports.wasm_alloc(proof.length);
  if (proofPtr === 0) throw new Error('Failed to allocate memory for proof');

  try {
    // Copy proof to WASM memory
    new Uint8Array(memory.buffer, proofPtr, proof.length).set(proof);

    // Call verify_raw
    const result = exports.verify_raw(proofPtr, proof.length, configSize);

    if (result === 1) return true;   // valid
    if (result === 0) return false;  // invalid
    throw new Error('Verification error');
  } finally {
    exports.wasm_dealloc(proofPtr, proof.length);
  }
}

/**
 * Check if WASM module is initialized
 * @returns {boolean}
 */
export function isInitialized() {
  return instance !== null;
}

/**
 * Get the WASM memory buffer (for advanced use)
 * @returns {WebAssembly.Memory}
 */
export function getMemory() {
  return memory;
}

/**
 * Get the raw WASM instance (for advanced use)
 * @returns {WebAssembly.Instance}
 */
export function getInstance() {
  return instance;
}

// Default export for convenience
export default { init, prove, verify, get_polynomial_size, isInitialized, seedRng };
