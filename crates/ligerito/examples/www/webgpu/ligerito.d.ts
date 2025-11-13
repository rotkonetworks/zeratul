/* tslint:disable */
/* eslint-disable */
/**
 * Generate a Ligerito proof from a polynomial
 *
 * # Arguments
 * * `polynomial` - Polynomial coefficients as u32 array
 * * `config_size` - Log2 of polynomial size (12, 20, or 24)
 *
 * # Returns
 * Serialized proof bytes
 *
 * # Example (JavaScript)
 * ```javascript
 * const polynomial = new Uint32Array(4096); // 2^12
 * // Fill with data...
 * const proof = prove(polynomial, 12);
 * ```
 */
export function prove(polynomial: Uint32Array, config_size: number): Uint8Array;
/**
 * Verify a Ligerito proof
 *
 * # Arguments
 * * `proof_bytes` - Serialized proof bytes (from `prove()`)
 * * `config_size` - Log2 of polynomial size (12, 20, or 24)
 *
 * # Returns
 * true if proof is valid, false otherwise
 *
 * # Example (JavaScript)
 * ```javascript
 * const isValid = verify(proofBytes, 12);
 * console.log('Valid:', isValid);
 * ```
 */
export function verify(proof_bytes: Uint8Array, config_size: number): boolean;
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
 * Initialize the WASM module (sets up panic hook for better error messages)
 */
export function init(): void;
/**
 * Run CPU sumcheck benchmark
 *
 * # Example (JavaScript)
 * ```javascript
 * const config = new BenchConfig(10, 6, 32);  // n=10, k=6, q=32
 * const result = await benchCpuSumcheck(config);
 * console.log(`CPU time: ${result.time_ms}ms`);
 * ```
 */
export function bench_cpu_sumcheck(config: BenchConfig): Promise<any>;
/**
 * Run GPU sumcheck benchmark (requires WebGPU support)
 *
 * # Example (JavaScript)
 * ```javascript
 * const config = new BenchConfig(10, 6, 32);
 * const result = await benchGpuSumcheck(config);
 * console.log(`GPU time: ${result.time_ms}ms`);
 * ```
 */
export function bench_gpu_sumcheck(config: BenchConfig): Promise<any>;
/**
 * Check if WebGPU is available
 */
export function check_webgpu_available(): Promise<boolean>;
/**
 * Benchmark configuration for sumcheck tests
 */
export class BenchConfig {
  free(): void;
  [Symbol.dispose](): void;
  constructor(n: number, k: number, q: number);
  n: number;
  k: number;
  q: number;
}
/**
 * Result from a sumcheck benchmark
 */
export class BenchResult {
  private constructor();
  free(): void;
  [Symbol.dispose](): void;
  readonly time_ms: number;
  readonly success: boolean;
  readonly error: string | undefined;
}

export type InitInput = RequestInfo | URL | Response | BufferSource | WebAssembly.Module;

export interface InitOutput {
  readonly memory: WebAssembly.Memory;
  readonly prove: (a: number, b: number, c: number, d: number) => void;
  readonly verify: (a: number, b: number, c: number, d: number) => void;
  readonly get_polynomial_size: (a: number, b: number) => void;
  readonly init: () => void;
  readonly __wbg_benchconfig_free: (a: number, b: number) => void;
  readonly __wbg_get_benchconfig_n: (a: number) => number;
  readonly __wbg_set_benchconfig_n: (a: number, b: number) => void;
  readonly __wbg_get_benchconfig_k: (a: number) => number;
  readonly __wbg_set_benchconfig_k: (a: number, b: number) => void;
  readonly __wbg_get_benchconfig_q: (a: number) => number;
  readonly __wbg_set_benchconfig_q: (a: number, b: number) => void;
  readonly benchconfig_new: (a: number, b: number, c: number) => number;
  readonly __wbg_benchresult_free: (a: number, b: number) => void;
  readonly benchresult_time_ms: (a: number) => number;
  readonly benchresult_success: (a: number) => number;
  readonly benchresult_error: (a: number, b: number) => void;
  readonly bench_cpu_sumcheck: (a: number) => number;
  readonly bench_gpu_sumcheck: (a: number) => number;
  readonly check_webgpu_available: () => number;
  readonly wasm_bindgen__convert__closures_____invoke__h37d500315fc510f4: (a: number, b: number, c: number) => void;
  readonly wasm_bindgen__closure__destroy__hafdb9ab3e38cf30c: (a: number, b: number) => void;
  readonly wasm_bindgen__convert__closures_____invoke__h3bd99db2baf4b10c: (a: number, b: number, c: number) => void;
  readonly wasm_bindgen__closure__destroy__hd7038e97be204e89: (a: number, b: number) => void;
  readonly wasm_bindgen__convert__closures_____invoke__h5b72a26126d41099: (a: number, b: number, c: number, d: number) => void;
  readonly __wbindgen_malloc: (a: number, b: number) => number;
  readonly __wbindgen_realloc: (a: number, b: number, c: number, d: number) => number;
  readonly __wbindgen_exn_store: (a: number) => void;
  readonly __wbindgen_free: (a: number, b: number, c: number) => void;
  readonly __wbindgen_add_to_stack_pointer: (a: number) => number;
  readonly __wbindgen_start: () => void;
}

export type SyncInitInput = BufferSource | WebAssembly.Module;
/**
* Instantiates the given `module`, which can either be bytes or
* a precompiled `WebAssembly.Module`.
*
* @param {{ module: SyncInitInput }} module - Passing `SyncInitInput` directly is deprecated.
*
* @returns {InitOutput}
*/
export function initSync(module: { module: SyncInitInput } | SyncInitInput): InitOutput;

/**
* If `module_or_path` is {RequestInfo} or {URL}, makes a request and
* for everything else, calls `WebAssembly.instantiate` directly.
*
* @param {{ module_or_path: InitInput | Promise<InitInput> }} module_or_path - Passing `InitInput` directly is deprecated.
*
* @returns {Promise<InitOutput>}
*/
export default function __wbg_init (module_or_path?: { module_or_path: InitInput | Promise<InitInput> } | InitInput | Promise<InitInput>): Promise<InitOutput>;
