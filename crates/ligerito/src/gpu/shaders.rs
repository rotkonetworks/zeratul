//! WGSL shader source code for WebGPU acceleration

/// Binary field operations (GF(2^128))
pub const BINARY_FIELD_SHADER: &str = include_str!("shaders/binary_field.wgsl");

/// FFT butterfly operations
pub const FFT_SHADER: &str = include_str!("shaders/fft.wgsl");

/// Combined shader (field ops + FFT)
pub fn get_fft_shader_source() -> String {
    format!("{}\n\n{}", BINARY_FIELD_SHADER, FFT_SHADER)
}

/// Legacy FFT butterfly shader (will be replaced by WGSL file)
pub const FFT_BUTTERFLY_SHADER_OLD: &str = r#"
// FFT butterfly for additive FFT over binary extension fields
// In GF(2^n), addition is XOR, so no twiddle factors needed!

struct Params {
    size: u32,
    stride: u32,
    log_stride: u32,
}

@group(0) @binding(0) var<storage, read_write> data: array<vec4<u32>>;  // GF(2^128) as 4x u32
@group(0) @binding(1) var<uniform> params: Params;

@compute @workgroup_size(256)
fn fft_butterfly(@builtin(global_invocation_id) id: vec3<u32>) {
    let idx = id.x;
    let half_size = params.size / 2u;

    if (idx >= half_size) {
        return;
    }

    // Butterfly indices
    let i = idx * 2u * params.stride;
    let j = i + params.stride;

    // Load values
    let lo = data[i];
    let hi = data[j];

    // Additive FFT butterfly (no multiplication needed!)
    // out[i] = lo + hi  (XOR in GF(2^n))
    // out[j] = lo       (keep original)
    data[i] = lo ^ hi;  // XOR = addition in binary fields
    data[j] = lo;
}
"#;

/// Parallel sumcheck contribution computation
pub const SUMCHECK_CONTRIB_SHADER: &str = r#"
// Compute sumcheck polynomial contributions in parallel

struct Params {
    n: u32,
    num_queries: u32,
}

// Input buffers
@group(0) @binding(0) var<storage, read> opened_rows: array<vec4<u32>>;     // 148 rows
@group(0) @binding(1) var<storage, read> v_challenges: array<vec4<u32>>;   // k challenges
@group(0) @binding(2) var<storage, read> alpha_pows: array<vec4<u32>>;     // 148 powers
@group(0) @binding(3) var<storage, read> sks_vks: array<vec4<u32>>;        // Basis

// Output buffer
@group(0) @binding(4) var<storage, read_write> local_basis: array<vec4<u32>>;  // 148 x 2^n

@group(0) @binding(5) var<uniform> params: Params;

// GF(2^128) multiplication (simplified - full version needs more work)
fn gf_mul(a: vec4<u32>, b: vec4<u32>) -> vec4<u32> {
    // TODO: Implement full carryless multiplication
    // For now, placeholder
    return a ^ b;
}

@compute @workgroup_size(148)
fn compute_contributions(@builtin(global_invocation_id) id: vec3<u32>) {
    let query_idx = id.x;

    if (query_idx >= params.num_queries) {
        return;
    }

    // 1. Compute tensorized dot product (simplified)
    var dot = vec4<u32>(0u);
    // TODO: Implement tensorized_dot_product

    // 2. Multiply by alpha^i
    let contribution = gf_mul(dot, alpha_pows[query_idx]);

    // 3. Compute and store local basis polynomial
    // TODO: Implement evaluate_scaled_basis_inplace
    let basis_offset = query_idx * params.n;
    for (var i = 0u; i < params.n; i++) {
        local_basis[basis_offset + i] = gf_mul(sks_vks[i], contribution);
    }
}
"#;

/// Parallel reduction for sumcheck
pub const SUMCHECK_REDUCE_SHADER: &str = r#"
// Reduce local basis polynomials into final basis_poly

struct Params {
    n: u32,
    num_queries: u32,
}

@group(0) @binding(0) var<storage, read> local_basis: array<vec4<u32>>;
@group(0) @binding(1) var<storage, read_write> basis_poly: array<vec4<u32>>;
@group(0) @binding(2) var<uniform> params: Params;

@compute @workgroup_size(256)
fn reduce_basis(@builtin(global_invocation_id) id: vec3<u32>) {
    let elem_idx = id.x;

    if (elem_idx >= params.n) {
        return;
    }

    // Sum all local_basis[query][elem_idx] into basis_poly[elem_idx]
    var sum = vec4<u32>(0u);

    for (var query = 0u; query < params.num_queries; query++) {
        let offset = query * params.n + elem_idx;
        sum ^= local_basis[offset];  // XOR = addition in GF(2^n)
    }

    basis_poly[elem_idx] = sum;
}
"#;
