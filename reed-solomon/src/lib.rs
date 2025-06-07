//! Binary Reed-Solomon encoding using FFT
//! Implements O(n log n) encoding from the paper

mod fft;
mod encode;

pub use encode::{ReedSolomonEncoder, encode, encode_in_place};
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

/// Create a Reed-Solomon encoder (matching Julia's interface)
pub fn reed_solomon<F: BinaryFieldElement>(
    message_length: usize,
    block_length: usize,
) -> ReedSolomon<F> {
    ReedSolomon::new(message_length, block_length)
}

#[cfg(test)]
mod tests {
    use super::*;
    use binary_fields::BinaryElem16;

    #[test]
    fn test_reed_solomon_creation() {
        let rs = reed_solomon::<BinaryElem16>(256, 1024);
        assert_eq!(rs.message_length(), 256);
        assert_eq!(rs.block_length(), 1024);
    }

    #[test]
    fn test_reed_solomon_creation_twiddless() {
        let rs = reed_solomon::<BinaryElem16>(256, 1024);
        assert_eq!(rs.message_length(), 256);
        assert_eq!(rs.block_length(), 1024);
        assert_eq!(rs.twiddles.len(), 1023); // 2^10 - 1
    }
}
