// Ligerito Web Worker
// Offloads proving computation from main thread
// Uses ES modules like Penumbra does

import init, { prove, verify, initThreadPool } from './ligerito.js';

let wasmInitialized = false;

self.onmessage = async function(e) {
  try {
    // Initialize WASM once per worker (lazy init like Penumbra)
    // Must explicitly pass WASM path for workers
    if (!wasmInitialized) {
      await init(new URL('./ligerito_bg.wasm', import.meta.url));

      // Rayon will use available parallelism automatically
      console.log('[Worker] WASM initialized - Rayon parallel execution enabled');
      wasmInitialized = true;
    }

    const { type, polynomial, configSize } = e.data;

    if (type === 'prove') {
      // Run proving in worker thread (non-blocking for UI)
      const proofBytes = prove(polynomial, configSize);

      self.postMessage({
        type: 'success',
        proof: proofBytes
      });
    } else if (type === 'verify') {
      // Run verification in worker thread
      const isValid = verify(e.data.proof, configSize);

      self.postMessage({
        type: 'success',
        isValid: isValid
      });
    }
  } catch (error) {
    self.postMessage({
      type: 'error',
      error: error.message || String(error)
    });
  }
};
