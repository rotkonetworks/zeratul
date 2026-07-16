// src/lib.rs
//! Binary extension fields GF(2^n) implementation
//! Mirrors the Julia BinaryFields module

mod elem;
mod poly;
mod simd;

pub use elem::{BinaryElem16, BinaryElem32, BinaryElem64, BinaryElem128};
pub use poly::{BinaryPoly16, BinaryPoly32, BinaryPoly64, BinaryPoly128, BinaryPoly256};

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
        let mut power = Self::one();
        let generator = Self::from_poly(Self::Poly::from_value(2));
        
        for i in 0..64 {
            if (bits >> i) & 1 == 1 {
                result = result.add(&power);
            }
            if i < 63 {
                power = power.mul(&generator);
            }
        }
        result
    }
}

pub trait BinaryPolynomial: 
    Sized + Copy + Clone + Default + PartialEq + Eq + std::fmt::Debug
{
    type Value: Copy + Clone + std::fmt::Debug;
    
    fn zero() -> Self;
    fn one() -> Self;
    fn from_value(val: u64) -> Self;
    fn value(&self) -> Self::Value;
    fn add(&self, other: &Self) -> Self;
    fn mul(&self, other: &Self) -> Self;
    fn div_rem(&self, other: &Self) -> (Self, Self);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_poly_addition() {
        let a = BinaryPoly16::from_value(0x1234);
        let b = BinaryPoly16::from_value(0x5678);
        let c = a.add(&b);
        assert_eq!(c.value(), 0x1234 ^ 0x5678);
        
        // Addition is its own inverse
        assert_eq!(a.add(&a), BinaryPoly16::zero());
    }

    #[test]
    fn test_poly_multiplication() {
        let a = BinaryPoly16::from_value(0x2);
        let b = BinaryPoly16::from_value(0x3);
        let c = a.mul(&b);
        assert_eq!(c.value(), 0x6); // 2 * 3 = 6 in binary polynomial
        
        // Test identity
        let one = BinaryPoly16::one();
        assert_eq!(a.mul(&one), a);
    }

    #[test]
    fn test_poly_division() {
        let a = BinaryPoly16::from_value(0x15); // x^4 + x^2 + 1
        let b = BinaryPoly16::from_value(0x3);  // x + 1
        let (q, r) = a.div_rem(&b);
        
        // Verify a = q * b + r
        assert_eq!(a, q.mul(&b).add(&r));
    }

    #[test]
    fn test_field_addition() {
        let a = BinaryElem16::from_value(0x1234);
        let b = BinaryElem16::from_value(0x5678);
        
        // Commutativity
        assert_eq!(a.add(&b), b.add(&a));
        
        // Identity
        let zero = BinaryElem16::zero();
        assert_eq!(a.add(&zero), a);
        
        // Inverse
        assert_eq!(a.add(&a), zero);
    }

    #[test]
    fn test_field_multiplication() {
        let a = BinaryElem16::from_value(0x2);
        let b = BinaryElem16::from_value(0x3);
        
        // Commutativity
        assert_eq!(a.mul(&b), b.mul(&a));
        
        // Identity
        let one = BinaryElem16::one();
        assert_eq!(a.mul(&one), a);
        
        // Zero property
        let zero = BinaryElem16::zero();
        assert_eq!(a.mul(&zero), zero);
    }

    #[test]
    fn test_field_inverse() {
        let a = BinaryElem16::from_value(0x1234);
        let a_inv = a.inv();
        
        // a * a^(-1) = 1
        assert_eq!(a.mul(&a_inv), BinaryElem16::one());
        
        // Test some known inverses
        let two = BinaryElem16::from_value(2);
        let two_inv = two.inv();
        assert_eq!(two.mul(&two_inv), BinaryElem16::one());
    }

    #[test]
    fn test_field_power() {
        let a = BinaryElem16::from_value(0x2);
        
        // a^0 = 1
        assert_eq!(a.pow(0), BinaryElem16::one());
        
        // a^1 = a
        assert_eq!(a.pow(1), a);
        
        // a^2
        assert_eq!(a.pow(2), a.mul(&a));
        
        // Fermat's little theorem: a^(2^16 - 1) = 1 for a != 0
        assert_eq!(a.pow(65535), BinaryElem16::one());
        
        // Subfield property: a^(2^16) = a
        assert_eq!(a.pow(65536), a);
    }

    #[test]
    fn test_zero_properties() {
        let zero = BinaryElem16::zero();
        let a = BinaryElem16::from_value(0x1234);
        
        // 0 + a = a
        assert_eq!(zero.add(&a), a);
        
        // 0 * a = 0
        assert_eq!(zero.mul(&a), zero);
        
        // 0^n = 0 for n > 0
        assert_eq!(zero.pow(5), zero);
    }

    #[test]
    #[should_panic(expected = "Cannot invert zero")]
    fn test_zero_inverse_panics() {
        let zero = BinaryElem16::zero();
        let _ = zero.inv();
    }

    #[test]
    fn test_field_axioms_32() {
        let a = BinaryElem32::from_value(0x12345678);
        let b = BinaryElem32::from_value(0x9ABCDEF0);
        let c = BinaryElem32::from_value(0x11111111);
        
        // Associativity of addition
        assert_eq!(a.add(&b.add(&c)), a.add(&b).add(&c));
        
        // Associativity of multiplication
        assert_eq!(a.mul(&b.mul(&c)), a.mul(&b).mul(&c));
        
        // Distributivity
        assert_eq!(a.mul(&b.add(&c)), a.mul(&b).add(&a.mul(&c)));
    }

    #[test]
    fn test_field_axioms_128() {
        let a = BinaryElem128::from_value(0x123456789ABCDEF0123456789ABCDEF0);
        let b = BinaryElem128::from_value(0xFEDCBA9876543210FEDCBA9876543210);
        
        // Test basic operations work
        let _ = a.add(&b);
        let _ = a.mul(&b);
        
        // Test inverse
        if a != BinaryElem128::zero() {
            let a_inv = a.inv();
            println!("a = {:x}", a.poly().value());
            println!("a_inv = {:x}", a_inv.poly().value());
            println!("a * a_inv = {:x}", a.mul(&a_inv).poly().value());
            assert_eq!(a.mul(&a_inv), BinaryElem128::one());
        }
    }

    #[test]
    fn test_from_bits() {
        // Test x + 1
        let elem = BinaryElem16::from_bits(0b11);
        let expected = BinaryElem16::from_value(1).add(&BinaryElem16::from_value(2));
        assert_eq!(elem, expected);
        
        // Test x^4 + x^2 + 1
        let elem = BinaryElem16::from_bits(0b10101);
        let x = BinaryElem16::from_value(2);
        let expected = x.pow(4).add(&x.pow(2)).add(&BinaryElem16::one());
        assert_eq!(elem, expected);
    }

    #[test]
    fn test_random_generation() {
        use rand::Rng;
        let mut rng = rand::thread_rng();
        
        // Generate random elements
        let a: BinaryElem16 = rng.gen();
        let b: BinaryElem16 = rng.gen();
        
        // They should satisfy field properties
        let c = a.add(&b);
        assert_eq!(c.add(&b), a);
    }

    #[test]
    fn test_poly_64_division() {
        let a = BinaryPoly64::from_value(0x123456789ABCDEF0);
        let b = BinaryPoly64::from_value(0xFEDCBA98);
        
        let (q, r) = a.div_rem(&b);
        
        // Verify a = q * b + r
        assert_eq!(a, q.mul(&b).add(&r));
        
        // r should have degree less than b
        assert!(r.value() < b.value() || r.value() == 0);
    }
}
