-- Add required headers for SharedArrayBuffer and multi-threaded WASM
ProgramHeader('Cross-Origin-Opener-Policy', 'same-origin')
ProgramHeader('Cross-Origin-Embedder-Policy', 'require-corp')
ProgramHeader('Cross-Origin-Resource-Policy', 'cross-origin')
ProgramHeader('Access-Control-Allow-Origin', '*')
