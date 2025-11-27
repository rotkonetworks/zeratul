/* tslint:disable */
/* eslint-disable */
/**
 * Initialize the WASM module (sets up panic hook for better error messages)
 */
export function init(): void;
/**
 * Get the expected polynomial size for a given config
 *
 * # Example (JavaScript)
 * ```javascript
 * const size = get_polynomial_size(12); // Returns 4096 (2^12)
 * ```
 */
export function get_polynomial_size(config_size: number): number;
/**
 * Generate random polynomial and prove it entirely within WASM
 *
 * This avoids copying large polynomials from JS to WASM, which is crucial
 * for large sizes like 2^28 (1GB of data).
 *
 * # Arguments
 * * `config_size` - Log2 of polynomial size (12, 16, 20, 24, 28, 30)
 * * `seed` - Random seed for reproducibility
 * * `transcript` - Optional transcript type: "sha256" (default), "merlin", or "blake2b"
 *
 * # Returns
 * Serialized proof bytes
 */
export function generate_and_prove(config_size: number, seed: bigint, transcript?: string | null): Uint8Array;
/**
 * Generate a Ligerito proof from a polynomial
 *
 * # Arguments
 * * `polynomial` - Polynomial coefficients as u32 array
 * * `config_size` - Log2 of polynomial size (12, 20, or 24)
 * * `transcript` - Optional transcript type: "sha256" (default), "merlin", or "blake2b"
 *
 * # Returns
 * Serialized proof bytes
 *
 * # Example (JavaScript)
 * ```javascript
 * const polynomial = new Uint32Array(4096); // 2^12
 * // Fill with data...
 * const proof = prove(polynomial, 12, "sha256");
 * ```
 */
export function prove(polynomial: Uint32Array, config_size: number, transcript?: string | null): Uint8Array;
/**
 * Verify a Ligerito proof
 *
 * # Arguments
 * * `proof_bytes` - Serialized proof bytes (from `prove()`)
 * * `config_size` - Log2 of polynomial size (12, 20, or 24)
 * * `transcript` - Optional transcript type: "sha256" (default), "merlin", or "blake2b"
 *   Must match the transcript used when generating the proof!
 *
 * # Returns
 * true if proof is valid, false otherwise
 *
 * # Example (JavaScript)
 * ```javascript
 * const isValid = verify(proofBytes, 12, "sha256");
 * console.log('Valid:', isValid);
 * ```
 */
export function verify(proof_bytes: Uint8Array, config_size: number, transcript?: string | null): boolean;
export function wbg_rayon_start_worker(receiver: number): void;
export function initThreadPool(num_threads: number): Promise<any>;
export class wbg_rayon_PoolBuilder {
  private constructor();
  free(): void;
  [Symbol.dispose](): void;
  numThreads(): number;
  build(): void;
  receiver(): number;
}

export type InitInput = RequestInfo | URL | Response | BufferSource | WebAssembly.Module;

export interface InitOutput {
  readonly main: (a: number, b: number) => number;
  readonly generate_and_prove: (a: number, b: bigint, c: number, d: number) => [number, number, number, number];
  readonly get_polynomial_size: (a: number) => [number, number, number];
  readonly init: () => void;
  readonly prove: (a: number, b: number, c: number, d: number, e: number) => [number, number, number, number];
  readonly verify: (a: number, b: number, c: number, d: number, e: number) => [number, number, number];
  readonly __wbg_wbg_rayon_poolbuilder_free: (a: number, b: number) => void;
  readonly initThreadPool: (a: number) => any;
  readonly wbg_rayon_poolbuilder_build: (a: number) => void;
  readonly wbg_rayon_poolbuilder_numThreads: (a: number) => number;
  readonly wbg_rayon_poolbuilder_receiver: (a: number) => number;
  readonly wbg_rayon_start_worker: (a: number) => void;
  readonly memory: WebAssembly.Memory;
  readonly __wbindgen_exn_store: (a: number) => void;
  readonly __externref_table_alloc: () => number;
  readonly __wbindgen_externrefs: WebAssembly.Table;
  readonly __wbindgen_free: (a: number, b: number, c: number) => void;
  readonly __wbindgen_malloc: (a: number, b: number) => number;
  readonly __wbindgen_realloc: (a: number, b: number, c: number, d: number) => number;
  readonly __externref_table_dealloc: (a: number) => void;
  readonly __wbindgen_thread_destroy: (a?: number, b?: number, c?: number) => void;
  readonly __wbindgen_start: (a: number) => void;
}

export type SyncInitInput = BufferSource | WebAssembly.Module;
/**
* Instantiates the given `module`, which can either be bytes or
* a precompiled `WebAssembly.Module`.
*
* @param {{ module: SyncInitInput, memory?: WebAssembly.Memory, thread_stack_size?: number }} module - Passing `SyncInitInput` directly is deprecated.
* @param {WebAssembly.Memory} memory - Deprecated.
*
* @returns {InitOutput}
*/
export function initSync(module: { module: SyncInitInput, memory?: WebAssembly.Memory, thread_stack_size?: number } | SyncInitInput, memory?: WebAssembly.Memory): InitOutput;

/**
* If `module_or_path` is {RequestInfo} or {URL}, makes a request and
* for everything else, calls `WebAssembly.instantiate` directly.
*
* @param {{ module_or_path: InitInput | Promise<InitInput>, memory?: WebAssembly.Memory, thread_stack_size?: number }} module_or_path - Passing `InitInput` directly is deprecated.
* @param {WebAssembly.Memory} memory - Deprecated.
*
* @returns {Promise<InitOutput>}
*/
export default function __wbg_init (module_or_path?: { module_or_path: InitInput | Promise<InitInput>, memory?: WebAssembly.Memory, thread_stack_size?: number } | InitInput | Promise<InitInput>, memory?: WebAssembly.Memory): Promise<InitOutput>;
