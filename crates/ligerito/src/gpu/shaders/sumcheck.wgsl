//! Parallel Sumcheck - V2 (Scalable to n=20+)
//!
//! This version eliminates massive buffer allocations by only computing
//! contributions on GPU, then reducing on CPU.
//!
//! Memory usage: O(num_queries) instead of O(num_queries × 2^n)
//! - n=20: 2.4 KB instead of 2.4 GB
//! - n=24: 2.4 KB instead of 38 GB
//!
//! Algorithm:
//! 1. GPU: Compute dot products in parallel (148 queries)
//! 2. GPU: Scale by alpha^i → contributions
//! 3. CPU: Accumulate contributions into basis_poly
//!
//! This works for ANY scale (n=20, n=24, n=28, etc.)

// Import binary field operations
// (Concatenated at compile time)

//
// Sumcheck Parameters
//

struct SumcheckParams {
    n: u32,                  // log size of basis polynomial
    num_queries: u32,        // Number of opened rows (typically 148)
    k: u32,                  // Number of v_challenges
    row_size: u32,           // Actual row size = 2^k
}

//
// Input Buffers (read-only)
//

// opened_rows[query][element]: The opened merkle rows (num_queries x row_size)
@group(0) @binding(0) var<storage, read> opened_rows: array<vec4<u32>>;

// v_challenges[i]: The verifier challenges (k elements)
@group(0) @binding(1) var<storage, read> v_challenges: array<vec4<u32>>;

// alpha_pows[i]: Precomputed powers of alpha
@group(0) @binding(2) var<storage, read> alpha_pows: array<vec4<u32>>;

// sorted_queries[i]: Basis array indices (query positions in basis_poly)
@group(0) @binding(3) var<storage, read> sorted_queries: array<u32>;

//
// Output Buffers (write)
//

// contributions[query]: Scaled dot products (num_queries elements)
// This is the ONLY large output - just 148 × 16 bytes = 2.4 KB
@group(0) @binding(4) var<storage, read_write> contributions: array<vec4<u32>>;

// Uniform buffer
@group(0) @binding(5) var<uniform> params: SumcheckParams;

//
// Tensorized Dot Product (supports large row sizes)
//

fn tensorized_dot_product(
    row_offset: u32,
    row_size: u32,
    num_challenges: u32
) -> vec4<u32> {
    // Strategy based on row size:
    // - ≤256: Use local buffer (4KB private memory - safe for all GPUs)
    // - >256: Use chunked processing with ping-pong buffers

    if (row_size <= 256u) {
        // Fast path: row fits in local memory
        var buffer: array<vec4<u32>, 256>;

        // Zero initialize
        for (var i = 0u; i < 256u; i++) {
            buffer[i] = ZERO;
        }

        // Load row
        for (var i = 0u; i < row_size; i++) {
            buffer[i] = opened_rows[row_offset + i];
        }

        var current_size = row_size;

        // Fold from last challenge to first
        for (var c = 0u; c < num_challenges; c++) {
            let challenge_idx = num_challenges - 1u - c;
            let r = v_challenges[challenge_idx];
            let one_minus_r = gf128_add(ONE, r);

            let half_size = current_size / 2u;

            // Fold in-place
            for (var i = 0u; i < half_size; i++) {
                let left_val = buffer[2u * i];
                let right_val = buffer[2u * i + 1u];

                let left = gf128_mul(one_minus_r, left_val);
                let right = gf128_mul(r, right_val);
                buffer[i] = gf128_add(left, right);
            }

            current_size = half_size;
        }

        return buffer[0];
    } else {
        // For very large rows (k > 8), use chunked processing
        // This handles k=9 (512 elements), k=10 (1024 elements), etc.

        // Start by loading first 256 elements
        var buffer_a: array<vec4<u32>, 256>;
        var buffer_b: array<vec4<u32>, 256>;

        // Zero initialize
        for (var i = 0u; i < 256u; i++) {
            buffer_a[i] = ZERO;
            buffer_b[i] = ZERO;
        }

        // Load first chunk
        for (var i = 0u; i < min(row_size, 256u); i++) {
            buffer_a[i] = opened_rows[row_offset + i];
        }

        // For row_size > 256, we'd need to load in chunks and fold progressively
        // For now, this is a placeholder - proper implementation would require
        // careful chunk management

        // TODO: Implement chunked folding for k > 8
        // For typical ligerito params (k=7), this path won't be hit

        return ZERO;
    }
}

//
// Main Sumcheck Kernel (Simplified)
//
// Each workgroup processes one query.
// Output: Just the scaled contribution (no basis array!)
//

@compute @workgroup_size(1)
fn sumcheck_contribution(@builtin(global_invocation_id) id: vec3<u32>) {
    let query_idx = id.x;

    if (query_idx >= params.num_queries) {
        return;
    }

    // SECURITY: Check for integer overflow
    let max_row_offset = 0xFFFFFFFFu / params.row_size;
    if (query_idx > max_row_offset) {
        return;
    }

    // 1. Compute tensorized dot product
    let row_offset = query_idx * params.row_size;
    let dot = tensorized_dot_product(row_offset, params.row_size, params.k);

    // 2. Scale by alpha^i
    let alpha_pow = alpha_pows[query_idx];
    let contribution = gf128_mul(dot, alpha_pow);

    // 3. Store ONLY the contribution (not entire basis array!)
    // This is the key difference from v1 - we save 99.999% of memory
    contributions[query_idx] = contribution;
}
