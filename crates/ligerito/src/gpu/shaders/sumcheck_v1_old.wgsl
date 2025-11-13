//! Parallel Sumcheck Polynomial Computation for Binary Extension Fields
//!
//! This shader computes sumcheck polynomial contributions in parallel.
//! Each workgroup processes one opened row and produces its local basis polynomial.
//!
//! Algorithm (per row i):
//! 1. Compute tensorized dot product: dot = ⟨row, L(v_challenges)⟩
//! 2. Scale by alpha^i: contribution = dot * alpha^i
//! 3. Evaluate scaled basis at query point
//! 4. Output local_basis[i] for later reduction
//!
//! This replaces the CPU loop that processes 148+ rows sequentially.

// Import binary field operations from binary_field.wgsl
// (Concatenated at compile time by Rust)

//
// Sumcheck Parameters
//

struct SumcheckParams {
    n: u32,                  // log size of basis polynomial (e.g., 10 for 2^10 = 1024)
    num_queries: u32,        // Number of opened rows (typically 148)
    k: u32,                  // Number of v_challenges (row width in log space)
    row_size: u32,           // Actual row size = 2^k
}

//
// Input Buffers (read-only)
//

// opened_rows[query][element]: The opened merkle rows (num_queries x row_size)
@group(0) @binding(0) var<storage, read> opened_rows: array<vec4<u32>>;

// v_challenges[i]: The verifier challenges (k elements)
@group(0) @binding(1) var<storage, read> v_challenges: array<vec4<u32>>;

// alpha_pows[i]: Precomputed powers of alpha (alpha^0, alpha^1, ..., alpha^(n-1))
@group(0) @binding(2) var<storage, read> alpha_pows: array<vec4<u32>>;

// DEBUG: Store raw dot products before scaling by alpha (reusing binding 3)
@group(0) @binding(3) var<storage, read_write> debug_dots: array<vec4<u32>>;

// sorted_queries[i]: Precomputed basis array indices (NOT query values!)
// These are computed on CPU by searching for F::from_bits(idx) == query
@group(0) @binding(4) var<storage, read> sorted_queries: array<u32>;

//
// Output Buffers (write)
//

// local_basis[query][coeff]: Per-query local basis polynomials (num_queries x 2^n)
@group(0) @binding(5) var<storage, read_write> local_basis: array<vec4<u32>>;

// contributions[query]: Dot product contributions (num_queries)
@group(0) @binding(6) var<storage, read_write> contributions: array<vec4<u32>>;

// Uniform buffer
@group(0) @binding(7) var<uniform> params: SumcheckParams;

// basis_poly_output[coeff]: Final reduced basis polynomial (2^n elements)
@group(0) @binding(8) var<storage, read_write> basis_poly_output: array<vec4<u32>>;

//
// Tensorized Dot Product
//
// Computes ⟨row, L(v_challenges)⟩ where L is the Lagrange basis.
// This is the inner loop hotspot - optimized for GPU.
//
// Algorithm: Fold the row vector by each challenge in reverse order:
//   for each challenge r (from last to first):
//     current[i] = (1-r) * current[2i] + r * current[2i+1]
//     (in binary fields: 1-r = 1+r since subtraction = addition)
//

fn tensorized_dot_product(
    row_offset: u32,
    row_size: u32,
    num_challenges: u32
) -> vec4<u32> {
    // Three-tier strategy based on row size:
    // - Small (≤128): Use local buffer, 2KB private memory (acceptable for most GPUs)
    // - Medium (>128, ≤512): Use ping-pong between local buffers
    // - Large (>512): Use global scratch buffer (requires additional binding)

    if (row_size <= 128u) {
        // Fast path: entire row fits in local memory (2KB max)
        // Modern GPUs can handle 2KB private memory without occupancy issues
        var buffer: array<vec4<u32>, 128>;

        // IMPORTANT: Zero initialize buffer to avoid undefined behavior!
        // WGSL doesn't guarantee zero-init for local arrays
        for (var i = 0u; i < 128u; i++) {
            buffer[i] = ZERO;
        }

        // Load initial row
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

            // Fold in-place (write back to same buffer at lower indices)
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
    } else if (row_size <= 512u) {
        // Medium path: Use double buffering with 64-element chunks
        // Process the row in chunks, gradually reducing size
        var buffer_a: array<vec4<u32>, 64>;
        var buffer_b: array<vec4<u32>, 64>;

        // Zero initialize both buffers
        for (var i = 0u; i < 64u; i++) {
            buffer_a[i] = ZERO;
            buffer_b[i] = ZERO;
        }

        var current_size = row_size;

        // Fold from last challenge to first
        for (var c = 0u; c < num_challenges; c++) {
            let challenge_idx = num_challenges - 1u - c;
            let r = v_challenges[challenge_idx];
            let one_minus_r = gf128_add(ONE, r);

            let half_size = current_size / 2u;

            // Once data fits in single buffer, switch to fast in-place folding
            if (current_size <= 64u) {
                // Load remaining data into buffer_a
                for (var i = 0u; i < current_size; i++) {
                    buffer_a[i] = opened_rows[row_offset + i];
                }

                // Finish remaining folds in local memory
                var size = current_size;
                for (var cc = c; cc < num_challenges; cc++) {
                    let cidx = num_challenges - 1u - cc;
                    let rr = v_challenges[cidx];
                    let omr = gf128_add(ONE, rr);
                    let hs = size / 2u;

                    for (var i = 0u; i < hs; i++) {
                        let left = gf128_mul(omr, buffer_a[2u * i]);
                        let right = gf128_mul(rr, buffer_a[2u * i + 1u]);
                        buffer_a[i] = gf128_add(left, right);
                    }
                    size = hs;
                }

                return buffer_a[0];
            }

            // Process in 64-element chunks, folding and writing back
            // This is still a placeholder - proper implementation would require
            // temporary storage in global memory or a scratch buffer
            // For now, limit to row_size <= 128 for correctness
            current_size = half_size;
        }

        return ZERO;  // Should not reach here for row_size <= 512
    } else {
        // Large path: Not yet implemented - requires global scratch buffer
        // For row_size > 512 (k >= 10), we need a dedicated scratch buffer
        // binding to store intermediate folding results

        // CRITICAL: This path is not implemented yet!
        // Returning ZERO will cause incorrect results
        // TODO: Implement global memory folding with scratch buffer
        return ZERO;
    }
}

//
// Evaluate Scaled Basis
//
// Sets basis[query] = contribution, all others to zero.
// This matches the CPU implementation which just sets one index.
//

fn evaluate_scaled_basis(
    query: u32,
    contribution: vec4<u32>,
    basis_size: u32,
    output_offset: u32
) {
    // SECURITY: Check both bounds and overflow before writing
    // 1. Check query is within basis_size
    // 2. Check that output_offset + query doesn't overflow
    // 3. Combined check prevents out-of-bounds writes
    if (query < basis_size && query <= (0xFFFFFFFFu - output_offset)) {
        local_basis[output_offset + query] = contribution;
    }
}

//
// Main Sumcheck Kernel
//
// Each workgroup processes one query (opened row).
// This achieves massive parallelism: 148+ queries computed simultaneously!
//

@compute @workgroup_size(1)  // One thread per query (for now)
fn sumcheck_contribution(@builtin(global_invocation_id) id: vec3<u32>) {
    let query_idx = id.x;

    if (query_idx >= params.num_queries) {
        return;
    }

    // SECURITY: Check for integer overflow in row_offset calculation
    // This prevents out-of-bounds access if query_idx * row_size overflows
    let max_row_offset = 0xFFFFFFFFu / params.row_size;
    if (query_idx > max_row_offset) {
        // Overflow would occur - abort this thread
        return;
    }

    // 1. Compute tensorized dot product
    let row_offset = query_idx * params.row_size;
    let dot = tensorized_dot_product(row_offset, params.row_size, params.k);

    // 2. Scale by alpha^i
    let alpha_pow = alpha_pows[query_idx];
    let contribution = gf128_mul(dot, alpha_pow);

    // DEBUG: Store alpha power (for debugging, we can compare CPU vs GPU alpha pows)
    debug_dots[query_idx] = alpha_pow;

    // Store contribution for final reduction
    contributions[query_idx] = contribution;

    // 3. Evaluate scaled basis at precomputed basis index
    // sorted_queries[query_idx] is the precomputed basis array index (NOT the query value!)
    let basis_idx = sorted_queries[query_idx];
    let basis_size = 1u << params.n;

    // SECURITY: Check for overflow in output_offset calculation
    let max_query_for_basis = 0xFFFFFFFFu / basis_size;
    if (query_idx > max_query_for_basis) {
        // Overflow would occur - abort
        return;
    }

    let output_offset = query_idx * basis_size;

    evaluate_scaled_basis(basis_idx, contribution, basis_size, output_offset);
}

//
// Reduction Kernel
//
// Sums all local_basis polynomials into final basis_poly.
// Each thread handles one coefficient across all queries.
//

@compute @workgroup_size(256)
fn reduce_basis(@builtin(global_invocation_id) id: vec3<u32>) {
    let coeff_idx = id.x;
    let basis_size = 1u << params.n;

    if (coeff_idx >= basis_size) {
        return;
    }

    // SECURITY: Check for overflow in offset calculation
    let max_query_safe = 0xFFFFFFFFu / basis_size;

    var sum = ZERO;

    // Sum across all queries
    for (var query = 0u; query < params.num_queries; query++) {
        // Skip this iteration if overflow would occur
        if (query > max_query_safe) {
            continue;
        }

        let offset = query * basis_size + coeff_idx;
        sum = gf128_add(sum, local_basis[offset]);
    }

    // Write to separate output buffer (NO race condition!)
    basis_poly_output[coeff_idx] = sum;
}

//
// Sum Contributions
//
// Final reduction of all contribution values into enforced_sum.
//

@compute @workgroup_size(256)
fn reduce_contributions(@builtin(global_invocation_id) id: vec3<u32>) {
    // Use parallel reduction in shared memory
    // For simplicity, just do sequential reduction in one thread

    if (id.x != 0u) {
        return;
    }

    var sum = ZERO;
    for (var i = 0u; i < params.num_queries; i++) {
        sum = gf128_add(sum, contributions[i]);
    }

    // Store final sum in contributions[0]
    contributions[0] = sum;
}
