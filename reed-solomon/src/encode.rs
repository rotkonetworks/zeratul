// reed-solomon/src/encode.rs

use crate::ReedSolomon;
use crate::fft;
use binary_fields::BinaryFieldElement;

/// Encode a message using Reed-Solomon
pub fn encode<F: BinaryFieldElement>(rs: &ReedSolomon<F>, message: &[F]) -> Vec<F> {
    let mut encoded = vec![F::zero(); rs.block_length()];
    encoded[..message.len()].copy_from_slice(message);
    
    encode_in_place(rs, &mut encoded);
    encoded
}

/// Encode in-place
pub fn encode_in_place<F: BinaryFieldElement>(rs: &ReedSolomon<F>, data: &mut [F]) {
    let message_len = rs.message_length();
    
    // Extract short twiddles for IFFT on message
    let short_twiddles = short_from_long_twiddles(&rs.twiddles, 
        rs.log_block_length, 
        rs.log_message_length);
    
    // Apply IFFT to message coefficients
    fft::ifft(&mut data[..message_len], &short_twiddles);
    
    // Apply FFT to full vector
    fft::fft(data, &rs.twiddles, false);
}

/// Extract short twiddles from long twiddles
fn short_from_long_twiddles<F: BinaryFieldElement>(
    long_twiddles: &[F],
    log_n: usize,
    log_k: usize
) -> Vec<F> {
    let k = 1 << log_k;
    let mut short_twiddles = vec![F::zero(); k - 1];
    
    let mut jump = 1 << (log_n - log_k);
    if jump > 0 && jump <= long_twiddles.len() {
        short_twiddles[0] = long_twiddles[jump - 1];
    }
    
    let mut idx = 1;
    for i in 1..log_k {
        jump *= 2;
        let take = 1 << i;
        
        for j in 0..take {
            if jump - 1 + j < long_twiddles.len() && idx + j < short_twiddles.len() {
                short_twiddles[idx + j] = long_twiddles[jump - 1 + j];
            }
        }
        idx += take;
    }
    
    short_twiddles
}
