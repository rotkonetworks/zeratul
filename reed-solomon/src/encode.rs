// reed-solomon/src/encode.rs
use crate::{ReedSolomon, fft, short_from_long_twiddles};
use binary_fields::BinaryFieldElement;

/// Encode a message using Reed-Solomon
pub fn encode<F: BinaryFieldElement>(rs: &ReedSolomon<F>, message: &[F]) -> Vec<F> {
    let mut encoded = vec![F::zero(); rs.block_length()];
    encoded[..message.len()].copy_from_slice(message);
    
    encode_in_place(rs, &mut encoded);
    encoded
}

/// Encode in-place (systematic encoding)
pub fn encode_in_place<F: BinaryFieldElement>(rs: &ReedSolomon<F>, data: &mut [F]) {
    let message_len = rs.message_length();
    
    // Extract short twiddles for IFFT on message
    let short_twiddles = short_from_long_twiddles(&rs.twiddles, 
        rs.log_block_length, 
        rs.log_message_length);
    
    // Apply IFFT to message coefficients
    fft::ifft(&mut data[..message_len], &short_twiddles);

    // Apply parallel FFT to full vector
    fft::fft(data, &rs.twiddles, true);
}

/// Non-systematic encoding for Ligero
pub fn encode_non_systematic<F: BinaryFieldElement>(
    rs: &ReedSolomon<F>, 
    data: &mut [F]
) {
    assert_eq!(data.len(), rs.block_length());
    
    // Scale by pi polynomials before FFT
    let message_len = rs.message_length();
    for i in 0..message_len {
        data[i] = data[i].mul(&rs.pis[i]);
    }

    // Apply parallel FFT to get evaluations
    fft::fft(data, &rs.twiddles, true);
}
