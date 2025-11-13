use std::fmt;

/// Trait for binary field elements
pub trait BinaryFieldElement: 
    Copy + Clone + PartialEq + Eq + Send + Sync + 
    fmt::Debug + fmt::Display
{
    /// Zero element
    fn zero() -> Self;
    
    /// Multiplicative identity
    fn one() -> Self;
    
    /// Field addition (XOR)
    fn add(&self, other: &Self) -> Self;
    
    /// Field multiplication
    fn mul(&self, other: &Self) -> Self;
    
    /// Multiplicative inverse
    fn inv(&self) -> Self;
    
    /// Create from bit representation
    fn from_bits(bits: u64) -> Self;
}

// Implement conversions between different field sizes
impl<T: BinaryFieldElement> From<bool> for T {
    fn from(b: bool) -> Self {
        if b { Self::one() } else { Self::zero() }
    }
}
