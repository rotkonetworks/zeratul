use crate::{ReedSolomon, fft};
use binary_fields::BinaryFieldElement;

/// Reed-Solomon encoder
pub struct ReedSolomonEncoder<F: BinaryFieldElement> {
    rs: ReedSolomon<F>,
}

/// Encode a message using Reed-Solomon
pub fn encode<F: BinaryFieldElement>(rs: &ReedSolomon<F>, message: &[F]) -> Vec<F> {
    let mut encoded = vec![F::zero(); rs.block_length()];
    encoded[..message.len()].copy_from_slice(message);
    
    // Apply FFT encoding
    fft::fft(&mut encoded, &rs.twiddles, false);
    
    encoded
}

/// Encode in-place
pub fn encode_in_place<F: BinaryFieldElement>(rs: &ReedSolomon<F>, data: &mut [F]) {
    fft::fft(data, &rs.twiddles, false);
}
