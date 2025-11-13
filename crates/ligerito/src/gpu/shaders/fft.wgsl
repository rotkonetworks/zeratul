// Additive FFT for Binary Extension Fields
// Optimized for GF(2^128) operations

// Import binary field operations
// (In Rust, we'll concatenate this with binary_field.wgsl)

struct FFTParams {
    size: u32,           // Total size of data array
    stride: u32,         // Butterfly stride for this pass
    log_stride: u32,     // log2(stride) - for bit reversal if needed
}

@group(0) @binding(0) var<storage, read_write> data: array<vec4<u32>>;
@group(0) @binding(1) var<uniform> params: FFTParams;

//
// Additive FFT Butterfly
//
// In binary extension fields, the FFT butterfly is trivial:
// out[i] = in[i] + in[i + stride]     (addition is XOR!)
// out[i + stride] = in[i]
//
// No twiddle factors needed! This is the power of additive FFT.
//

@compute @workgroup_size(256)
fn fft_butterfly(@builtin(global_invocation_id) id: vec3<u32>) {
    let idx = id.x;
    let half_size = params.size / 2u;

    // Early exit for out-of-bounds threads
    if (idx >= half_size) {
        return;
    }

    // Calculate butterfly indices
    // Each thread handles one butterfly operation
    let block_size = params.stride * 2u;
    let block = idx / params.stride;
    let offset = idx % params.stride;

    let i = block * block_size + offset;
    let j = i + params.stride;

    // Load values
    let lo = data[i];
    let hi = data[j];

    // Butterfly operation (just XOR!)
    data[i] = lo ^ hi;  // Addition in GF(2^n)
    data[j] = lo;       // Keep original
}

//
// In-place bit-reversal permutation (if needed for FFT)
//

fn bit_reverse(x: u32, bits: u32) -> u32 {
    var result: u32 = 0u;
    for (var i = 0u; i < bits; i++) {
        result = (result << 1u) | ((x >> i) & 1u);
    }
    return result;
}

@compute @workgroup_size(256)
fn bit_reversal_permutation(@builtin(global_invocation_id) id: vec3<u32>) {
    let idx = id.x;

    if (idx >= params.size) {
        return;
    }

    let rev_idx = bit_reverse(idx, params.log_stride);

    // Only swap if idx < rev_idx to avoid double-swapping
    if (idx < rev_idx) {
        let temp = data[idx];
        data[idx] = data[rev_idx];
        data[rev_idx] = temp;
    }
}

//
// Inverse FFT (same as forward FFT in binary fields!)
//
// In GF(2^n), IFFT = FFT because addition is self-inverse
//

@compute @workgroup_size(256)
fn ifft_butterfly(@builtin(global_invocation_id) id: vec3<u32>) {
    // Same as fft_butterfly!
    let idx = id.x;
    let half_size = params.size / 2u;

    if (idx >= half_size) {
        return;
    }

    let block_size = params.stride * 2u;
    let block = idx / params.stride;
    let offset = idx % params.stride;

    let i = block * block_size + offset;
    let j = i + params.stride;

    let lo = data[i];
    let hi = data[j];

    data[i] = lo ^ hi;
    data[j] = lo;
}
