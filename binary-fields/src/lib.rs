// src/lib.rs
//! Binary extension fields GF(2^n) implementation
//! Mirrors the Julia BinaryFields module

mod elem;
mod poly;
pub mod simd;

pub use elem::{BinaryElem16, BinaryElem32, BinaryElem64, BinaryElem128};
pub use poly::{BinaryPoly16, BinaryPoly32, BinaryPoly64, BinaryPoly128, BinaryPoly256};
pub use simd::{batch_mul_gf128, batch_add_gf128};

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
    fn test_poly_operations() {
        // polynomial addition
        let a = BinaryPoly16::from_value(0x1234);
        let b = BinaryPoly16::from_value(0x5678);
        assert_eq!(a.add(&b).value(), 0x1234 ^ 0x5678);
        assert_eq!(a.add(&a), BinaryPoly16::zero());

        // polynomial multiplication
        let a = BinaryPoly16::from_value(0x2);
        let b = BinaryPoly16::from_value(0x3);
        assert_eq!(a.mul(&b).value(), 0x6);
        assert_eq!(a.mul(&BinaryPoly16::one()), a);

        // polynomial division
        let a = BinaryPoly16::from_value(0x15); // x^4 + x^2 + 1
        let b = BinaryPoly16::from_value(0x3);  // x + 1
        let (q, r) = a.div_rem(&b);
        assert_eq!(a, q.mul(&b).add(&r));
    }

    #[test]
    fn test_field_axioms() {
        // test for all field sizes
        macro_rules! test_field {
            ($elem:ty, $val1:expr, $val2:expr, $val3:expr) => {
                let a = <$elem>::from_value($val1);
                let b = <$elem>::from_value($val2);
                let c = <$elem>::from_value($val3);

                // associativity
                assert_eq!(a.add(&b.add(&c)), a.add(&b).add(&c));
                assert_eq!(a.mul(&b.mul(&c)), a.mul(&b).mul(&c));

                // commutativity
                assert_eq!(a.add(&b), b.add(&a));
                assert_eq!(a.mul(&b), b.mul(&a));

                // distributivity
                assert_eq!(a.mul(&b.add(&c)), a.mul(&b).add(&a.mul(&c)));

                // identities
                assert_eq!(a.add(&<$elem>::zero()), a);
                assert_eq!(a.mul(&<$elem>::one()), a);

                // inverses
                assert_eq!(a.add(&a), <$elem>::zero());
                if a != <$elem>::zero() {
                    assert_eq!(a.mul(&a.inv()), <$elem>::one());
                }
            };
        }

        test_field!(BinaryElem16, 0x1234, 0x5678, 0x9ABC);
        test_field!(BinaryElem32, 0x12345678, 0x9ABCDEF0, 0x11111111);
        test_field!(BinaryElem128, 0x123456789ABCDEF0123456789ABCDEF0, 
                    0xFEDCBA9876543210FEDCBA9876543210, 0x1111111111111111111111111111111);
    }

    #[test]
    fn test_fermat_little_theorem() {
        // a^(2^n - 1) = 1 for all a != 0 in GF(2^n)
        let a = BinaryElem16::from_value(0x1234);
        assert_eq!(a.pow(65535), BinaryElem16::one()); // 2^16 - 1

        // subfield property: a^(2^n) = a
        assert_eq!(a.pow(65536), a);
    }

    #[test]
    #[should_panic(expected = "Cannot invert zero")]
    fn test_zero_inverse_panics() {
        let _ = BinaryElem16::zero().inv();
    }

    #[test]
    fn test_from_bits() {
        // x + 1
        let elem = BinaryElem16::from_bits(0b11);
        let expected = BinaryElem16::from_value(1).add(&BinaryElem16::from_value(2));
        assert_eq!(elem, expected);

        // x^4 + x^2 + 1
        let elem = BinaryElem16::from_bits(0b10101);
        let x = BinaryElem16::from_value(2);
        let expected = x.pow(4).add(&x.pow(2)).add(&BinaryElem16::one());
        assert_eq!(elem, expected);
    }
}

#[cfg(test)]
mod julia_compatibility_tests {
    use super::*;

    #[test]
    fn test_julia_sage_vectors() {
        // verify we can construct the sage comparison vectors
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

        let v: Vec<BinaryElem128> = v_values.iter()
            .map(|&val| BinaryElem128::from_value(val))
            .collect();

        for (i, &val) in v_values.iter().enumerate() {
            assert_eq!(v[i].poly().value(), val);
        }
    }

    #[test]
    fn test_julia_betas() {
        // beta values for field embeddings
        let beta16 = BinaryElem128::from_value(44320122245670141922313918651005395719);
        let _beta32 = BinaryElem128::from_value(23246355947528323030879441634950214446);

        // verify beta^16 generates distinct elements
        let mut bs16 = vec![BinaryElem128::one()];
        for i in 1..16 {
            bs16.push(bs16[i-1].mul(&beta16));
        }

        for i in 0..16 {
            for j in (i+1)..16 {
                assert_ne!(bs16[i], bs16[j]);
            }
        }
    }

    #[test]
    fn test_large_multiplication() {
        // test 128-bit multiplication edge cases
        let cases = vec![
            (0xFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFF0u128, 0xFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFF0u128),
            (u128::MAX, u128::MAX),
            (0x8000000000000000_0000000000000000u128, 0x8000000000000000_0000000000000000u128),
        ];

        for (a_val, b_val) in cases {
            let a = BinaryElem128::from(a_val);
            let b = BinaryElem128::from(b_val);
            
            let _c = a.mul(&b);
            
            // verify inverse works
            if a != BinaryElem128::zero() {
                let a_inv = a.inv();
                assert_eq!(a.mul(&a_inv), BinaryElem128::one());
            }
        }
    }

    #[test]
    fn test_field_embedding() {
        // test embeddings for ligerito
        let elem16 = BinaryElem16::from_value(0x1234);
        let elem32: BinaryElem32 = elem16.into();
        let elem64: BinaryElem64 = elem16.into();
        let elem128: BinaryElem128 = elem16.into();

        assert_eq!(elem32.poly().value() & 0xFFFF, 0x1234);
        assert_eq!(elem64.poly().value() & 0xFFFF, 0x1234);
        assert_eq!(elem128.poly().value() & 0xFFFF, 0x1234);
    }
}
