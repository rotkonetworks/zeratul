// reed-solomon/src/lib.rs

mod fft;
mod encode;

pub use encode::{encode, encode_in_place};
pub use fft::{compute_twiddles, fft, ifft};

use binary_fields::BinaryFieldElement;

/// Reed-Solomon encoding configuration
pub struct ReedSolomon<F: BinaryFieldElement> {
    log_message_length: usize,
    log_block_length: usize,
    twiddles: Vec<F>,
}

impl<F: BinaryFieldElement> ReedSolomon<F> {
    pub fn new(message_length: usize, block_length: usize) -> Self {
        assert!(message_length.is_power_of_two());
        assert!(block_length.is_power_of_two());
        assert!(message_length < block_length);
        
        let log_message_length = message_length.trailing_zeros() as usize;
        let log_block_length = block_length.trailing_zeros() as usize;
        
        // Compute twiddles with beta = 0 for systematic encoding
        let twiddles = fft::compute_twiddles(log_block_length, F::zero());
        
        Self {
            log_message_length,
            log_block_length,
            twiddles,
        }
    }
    
    pub fn message_length(&self) -> usize {
        1 << self.log_message_length
    }
    
    pub fn block_length(&self) -> usize {
        1 << self.log_block_length
    }
}

/// Create a Reed-Solomon encoder
pub fn reed_solomon<F: BinaryFieldElement>(
    message_length: usize,
    block_length: usize,
) -> ReedSolomon<F> {
    ReedSolomon::new(message_length, block_length)
}
