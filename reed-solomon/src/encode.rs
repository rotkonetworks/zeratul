use crate::ReedSolomon;
use crate::fft_simple;
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
    fft_simple::fft_systematic(&mut encoded);
    
    encoded
}

/// Encode in-place
pub fn encode_in_place<F: BinaryFieldElement>(_rs: &ReedSolomon<F>, data: &mut [F]) {
    fft_simple::fft_systematic(data);
}
