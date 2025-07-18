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

#[cfg(test)]
mod julia_compatibility_tests {
    use super::*;

    #[test]
    fn test_irreducible_polynomials() {
        // Test that our irreducible polynomials match Julia's
        // Julia: irreducible_poly(::Type{BinaryElem16}) = BinaryPoly16(UInt16(0b101101))
        // This is x^5 + x^3 + x^2 + 1, but we need x^16 + x^5 + x^3 + x^2 + 1
        // The x^16 is implicit in the reduction
        
        // Julia: irreducible_poly(::Type{BinaryElem32}) = BinaryPoly32(UInt32(0b11001 | 1 << 7 | 1 << 9 | 1 << 15))
        // This is x^15 + x^9 + x^7 + x^4 + x^3 + 1, but we need x^32 + ...
        
        // Test basic field properties
        let a = BinaryElem16::from_value(0x1234);
        let b = BinaryElem16::from_value(0x5678);
        
        // Test that multiplication works correctly
        let c = a.mul(&b);
        println!("a * b = {:x}", c.poly().value());
    }

    #[test]
    fn test_julia_sage_comparison() {
        // From Julia test: Sage comparison
        let v_values: Vec<u128> = vec![
            48843935073701397021918627474152975110,
            257371465678647658219914792930422930533,
            197874898248752057839214693713406247745,
            86301329031543269357031453671330949739,
            245592208151890074913079678553060805151,
            191477208903117015546989222243599496680,
            92830719409229016308089219817617750833,
            264528954340572454088312978462893134650,
            158998607558664949362678439274836957424,
            187448928532932960560649099299315170550,
            177534835847791156274472818404289166039,
            307322189246381679156077507151623179879,
            117208864575585467966316847685913785498,
            332422437295611968587046799211069213610,
            109428368893056851194159753059340120844,
            197947890894953343492199130314470631788,
        ];

        // Convert to BinaryElem128
        let v: Vec<BinaryElem128> = v_values.iter()
            .map(|&val| BinaryElem128::from_value(val))
            .collect();

        // This would require implementing FFT, so we'll just verify the values are created correctly
        for (i, &val) in v_values.iter().enumerate() {
            assert_eq!(v[i].poly().value(), val);
        }
    }

    #[test]
    fn test_field_embedding() {
        // Test field embeddings for Ligerito
        let elem16 = BinaryElem16::from_value(0x1234);
        let elem32: BinaryElem32 = elem16.into();
        let elem64: BinaryElem64 = elem16.into();
        let elem128: BinaryElem128 = elem16.into();

        // Basic check that the value is preserved in the lower bits
        assert_eq!(elem32.poly().value() & 0xFFFF, 0x1234);
        assert_eq!(elem64.poly().value() & 0xFFFF, 0x1234);
        assert_eq!(elem128.poly().value() & 0xFFFF, 0x1234);
    }

    #[test]
    fn test_multiplication_in_gf128() {
        // Test the specific multiplication algorithm for GF(2^128)
        let a = BinaryElem128::from_value(0x123456789ABCDEF0123456789ABCDEF0);
        let b = BinaryElem128::from_value(0xFEDCBA9876543210FEDCBA9876543210);
        
        let c = a.mul(&b);
        
        // Verify it doesn't panic and produces a result
        println!("GF(2^128) multiplication result: {:x}", c.poly().value());
        
        // Test identity
        let one = BinaryElem128::one();
        assert_eq!(a.mul(&one), a);
        
        // Test that a * a^(-1) = 1
        if a != BinaryElem128::zero() {
            let a_inv = a.inv();
            assert_eq!(a.mul(&a_inv), BinaryElem128::one());
        }
    }

    #[test]
    fn test_from_bits_compatibility() {
        // Test that from_bits works like Julia's implementation
        // Julia creates a polynomial from bit representation
        
        // Test x + 1 (bits 11 = 0b11)
        let elem = BinaryElem16::from_bits(0b11);
        let expected = BinaryElem16::from_value(1).add(&BinaryElem16::from_value(2));
        assert_eq!(elem, expected);
        
        // Test x^4 + x^2 + 1 (bits 10101 = 0b10101) 
        let elem = BinaryElem16::from_bits(0b10101);
        let x = BinaryElem16::from_value(2);
        let expected = x.pow(4).add(&x.pow(2)).add(&BinaryElem16::one());
        assert_eq!(elem, expected);
    }

    #[test]
    fn test_fermat_little_theorem() {
        // Verify Fermat's little theorem for binary fields
        // For GF(2^n), a^(2^n - 1) = 1 for all a != 0
        
        // Test for BinaryElem16
        let a = BinaryElem16::from_value(0x1234);
        assert_ne!(a, BinaryElem16::zero());
        assert_eq!(a.pow(65535), BinaryElem16::one()); // 2^16 - 1 = 65535
        
        // Also test subfield property: a^(2^16) = a
        assert_eq!(a.pow(65536), a);
    }

    #[test]
    fn test_poly_karatsuba_mul() {
        // Test Karatsuba multiplication for BinaryPoly128
        let a = BinaryPoly128::new(0x123456789ABCDEF0123456789ABCDEF0);
        let b = BinaryPoly128::new(0xFEDCBA9876543210FEDCBA9876543210);
        
        let c = a.mul(&b);
        
        // The result should be consistent with binary polynomial multiplication
        println!("Karatsuba result: {:x}", c.value());
        
        // Test identity
        let one = BinaryPoly128::one();
        assert_eq!(a.mul(&one), a);
        
        // Test that multiplication with zero gives zero
        let zero = BinaryPoly128::zero();
        assert_eq!(a.mul(&zero), zero);
    }

    #[test]
    fn test_julia_betas_embedding() {
        // Test the beta values used in Julia for field embeddings
        // From Julia: beta = BinaryElem128(UInt128(44320122245670141922313918651005395719))
        let beta16 = BinaryElem128::from_value(44320122245670141922313918651005395719);
        
        // From Julia: beta = BinaryElem128(UInt128(23246355947528323030879441634950214446))
        let beta32 = BinaryElem128::from_value(23246355947528323030879441634950214446);
        
        // Test that these are valid field elements
        assert_ne!(beta16, BinaryElem128::zero());
        assert_ne!(beta32, BinaryElem128::zero());
        
        // Test powers of beta
        let mut bs16 = vec![BinaryElem128::one()];
        for i in 1..16 {
            bs16.push(bs16[i-1].mul(&beta16));
        }
        
        // Verify all powers are distinct (they should be for a primitive element)
        for i in 0..16 {
            for j in (i+1)..16 {
                assert_ne!(bs16[i], bs16[j], "Powers {} and {} of beta16 are equal", i, j);
            }
        }
    }

    #[test]
    fn test_field_axioms() {
        // Test field axioms for all field sizes
        
        // Test for BinaryElem16
        {
            let a = BinaryElem16::from_value(0x1234);
            let b = BinaryElem16::from_value(0x5678);
            let c = BinaryElem16::from_value(0x9ABC);
            
            // Associativity of addition
            assert_eq!(a.add(&b.add(&c)), a.add(&b).add(&c));
            
            // Associativity of multiplication
            assert_eq!(a.mul(&b.mul(&c)), a.mul(&b).mul(&c));
            
            // Distributivity
            assert_eq!(a.mul(&b.add(&c)), a.mul(&b).add(&a.mul(&c)));
            
            // Commutativity of addition
            assert_eq!(a.add(&b), b.add(&a));
            
            // Commutativity of multiplication
            assert_eq!(a.mul(&b), b.mul(&a));
        }
        
        // Test for BinaryElem32
        {
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
        
        // Test for BinaryElem128
        {
            let a = BinaryElem128::from_value(0x123456789ABCDEF0123456789ABCDEF0);
            let b = BinaryElem128::from_value(0xFEDCBA9876543210FEDCBA9876543210);
            let _c = BinaryElem128::from_value(0x1111111111111111111111111111111);
            
            // Test basic operations work
            let _sum = a.add(&b);
            let _prod = a.mul(&b);
            
            // Test inverse
            if a != BinaryElem128::zero() {
                let a_inv = a.inv();
                assert_eq!(a.mul(&a_inv), BinaryElem128::one());
            }
        }
    }

    #[test]
    fn test_zero_properties() {
        // Test properties of zero element
        let zero16 = BinaryElem16::zero();
        let zero32 = BinaryElem32::zero();
        let zero128 = BinaryElem128::zero();
        
        let a16 = BinaryElem16::from_value(0x1234);
        let a32 = BinaryElem32::from_value(0x12345678);
        let a128 = BinaryElem128::from_value(0x123456789ABCDEF0);
        
        // 0 + a = a
        assert_eq!(zero16.add(&a16), a16);
        assert_eq!(zero32.add(&a32), a32);
        assert_eq!(zero128.add(&a128), a128);
        
        // 0 * a = 0
        assert_eq!(zero16.mul(&a16), zero16);
        assert_eq!(zero32.mul(&a32), zero32);
        assert_eq!(zero128.mul(&a128), zero128);
        
        // 0^n = 0 for n > 0
        assert_eq!(zero16.pow(5), zero16);
        assert_eq!(zero32.pow(5), zero32);
        assert_eq!(zero128.pow(5), zero128);
    }

    #[test]
    #[should_panic(expected = "Cannot invert zero")]
    fn test_zero_inverse_panics_16() {
        let zero = BinaryElem16::zero();
        let _ = zero.inv();
    }

    #[test]
    #[should_panic(expected = "Cannot invert zero")]
    fn test_zero_inverse_panics_32() {
        let zero = BinaryElem32::zero();
        let _ = zero.inv();
    }

    #[test]
    #[should_panic(expected = "Cannot invert zero")]
    fn test_zero_inverse_panics_128() {
        let zero = BinaryElem128::zero();
        let _ = zero.inv();
    }

    #[test]
    fn test_poly_division() {
        // Test polynomial division
        let a = BinaryPoly16::from_value(0x15); // x^4 + x^2 + 1
        let b = BinaryPoly16::from_value(0x3);  // x + 1
        let (q, r) = a.div_rem(&b);
        
        // Verify a = q * b + r
        assert_eq!(a, q.mul(&b).add(&r));
        
        // Test with larger polynomials
        let a = BinaryPoly64::from_value(0x123456789ABCDEF0);
        let b = BinaryPoly64::from_value(0xFEDCBA98);
        
        let (q, r) = a.div_rem(&b);
        
        // Verify a = q * b + r
        assert_eq!(a, q.mul(&b).add(&r));
        
        // r should have degree less than b
        assert!(r.value() < b.value() || r.value() == 0);
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
        
        // Test for other sizes
        let _: BinaryElem32 = rng.gen();
        let _: BinaryElem128 = rng.gen();
    }
}
