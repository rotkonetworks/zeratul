// reed-solomon/src/encode.rs
use crate::{ReedSolomon, fft, short_from_long_twiddles};
use binary_fields::BinaryFieldElement;

/// Encode a message using Reed-Solomon
pub fn encode<F: BinaryFieldElement + 'static>(rs: &ReedSolomon<F>, message: &[F]) -> Vec<F> {
    let mut encoded = vec![F::zero(); rs.block_length()];
    encoded[..message.len()].copy_from_slice(message);
    
    encode_in_place(rs, &mut encoded);
    encoded
}

/// Encode in-place (systematic encoding)
pub fn encode_in_place<F: BinaryFieldElement + 'static>(rs: &ReedSolomon<F>, data: &mut [F]) {
    encode_in_place_with_parallel(rs, data, true)
}

/// Encode in-place with configurable parallelization
pub fn encode_in_place_with_parallel<F: BinaryFieldElement + 'static>(
    rs: &ReedSolomon<F>,
    data: &mut [F],
    parallel: bool,
) {
    use binary_fields::BinaryElem32;
    use std::any::TypeId;

    // Fast path for BinaryElem32 using SIMD
    if TypeId::of::<F>() == TypeId::of::<BinaryElem32>() {
        let data_gf32 = unsafe { std::slice::from_raw_parts_mut(data.as_mut_ptr() as *mut BinaryElem32, data.len()) };
        let twiddles_gf32 = unsafe { std::slice::from_raw_parts(rs.twiddles.as_ptr() as *const BinaryElem32, rs.twiddles.len()) };

        let message_len = rs.message_length();
        let short_twiddles = short_from_long_twiddles(twiddles_gf32, rs.log_block_length, rs.log_message_length);

        crate::fft_gf32::ifft_gf32(&mut data_gf32[..message_len], &short_twiddles);
        crate::fft_gf32::fft_gf32(data_gf32, twiddles_gf32, parallel);
        return;
    }

    // Generic fallback
    let message_len = rs.message_length();
    let short_twiddles = short_from_long_twiddles(&rs.twiddles, rs.log_block_length, rs.log_message_length);
    fft::ifft(&mut data[..message_len], &short_twiddles);
    fft::fft(data, &rs.twiddles, parallel);
}

/// Non-systematic encoding for Ligero
pub fn encode_non_systematic<F: BinaryFieldElement + 'static>(
    rs: &ReedSolomon<F>,
    data: &mut [F]
) {
    use binary_fields::BinaryElem32;
    use std::any::TypeId;

    assert_eq!(data.len(), rs.block_length());

    // Scale by pi polynomials before FFT
    let message_len = rs.message_length();
    for i in 0..message_len {
        data[i] = data[i].mul(&rs.pis[i]);
    }

    // Fast path for BinaryElem32 using SIMD
    if TypeId::of::<F>() == TypeId::of::<BinaryElem32>() {
        let data_gf32 = unsafe { std::slice::from_raw_parts_mut(data.as_mut_ptr() as *mut BinaryElem32, data.len()) };
        let twiddles_gf32 = unsafe { std::slice::from_raw_parts(rs.twiddles.as_ptr() as *const BinaryElem32, rs.twiddles.len()) };
        crate::fft_gf32::fft_gf32(data_gf32, twiddles_gf32, true);
        return;
    }

    // Generic fallback
    fft::fft(data, &rs.twiddles, true);
}
