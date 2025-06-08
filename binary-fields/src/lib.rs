//! Binary extension fields GF(2^n) implementation
//! Mirrors the Julia BinaryFields module

mod elem;
mod poly;
mod simd;

pub use elem::{ BinaryElem16, BinaryElem32, BinaryElem64, BinaryElem128};
pub use poly::{ BinaryPoly16, BinaryPoly32, BinaryPoly64, BinaryPoly128};

// Re-export traits
pub trait BinaryFieldElement: Send + Sync + 
    Sized + Copy + Clone + Default + PartialEq + Eq + std::fmt::Debug 
{
    type Poly: BinaryPolynomial;
    
    fn zero() -> Self;
    fn one() -> Self;
    fn from_poly(poly: Self::Poly) -> Self;
    fn poly(&self) -> Self::Poly;
    fn add(&self, other: &Self) -> Self;
    fn mul(&self, other: &Self) -> Self;
    fn inv(&self) -> Self;
    fn pow(&self, exp: u64) -> Self;
    
    fn from_bits(bits: u64) -> Self {
        let mut result = Self::zero();
        let mut current_bit = 0u64;
        
        while current_bit < 64 && (bits >> current_bit) != 0 {
            if (bits >> current_bit) & 1 == 1 {
                // Add x^current_bit
                let mut power = Self::one();
                for _ in 0..current_bit {
                    power = power.add(&power);
                }
                result = result.add(&power);
            }
            current_bit += 1;
        }
        result
    }
}

pub trait BinaryPolynomial: 
    Sized + Copy + Clone + Default + PartialEq + Eq
{
    type Value: Copy + Clone;
    
    fn zero() -> Self;
    fn one() -> Self;
    fn value(&self) -> Self::Value;
    fn add(&self, other: &Self) -> Self;
    fn mul(&self, other: &Self) -> Self;
    fn div_rem(&self, other: &Self) -> (Self, Self);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_field_axioms() {
        let a = BinaryElem16::from_poly(BinaryPoly16::from(0x1234));
        let b = BinaryElem16::from_poly(BinaryPoly16::from(0x5678));
        
        // Addition is its own inverse
        assert_eq!(a.add(&a), BinaryElem16::zero());
        
        // Multiplicative identity
        assert_eq!(a.mul(&BinaryElem16::one()), a);
        
        // Field closure
        let c = a.add(&b);
        let d = a.mul(&b);
        assert_ne!(c, BinaryElem16::zero());
        assert_ne!(d, BinaryElem16::zero());
    }
}
