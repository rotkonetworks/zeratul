use crate::{BinaryFieldElement, BinaryPolynomial};
use crate::poly::{BinaryPoly16, BinaryPoly32, BinaryPoly64, BinaryPoly128};

// Irreducible polynomials (matching Julia implementation)
const IRREDUCIBLE_16: u32 = 0x1002D;  // x^16 + x^5 + x^3 + x^2 + 1 (need to store in larger type)
const IRREDUCIBLE_32: u64 = (1u64 << 32) | 0b11001 | (1 << 7) | (1 << 9) | (1 << 15);  // x^32 + Conway polynomial
const IRREDUCIBLE_128: u128 = 0b10000111;  // x^128 + x^7 + x^2 + x + 1 (AES) - the x^128 is implicit

macro_rules! impl_binary_elem {
    ($name:ident, $poly_type:ident, $poly_double:ident, $value_type:ty, $value_double:ty, $irreducible:expr, $bitsize:expr) => {
        #[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
        #[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
        pub struct $name($poly_type);

        impl $name {
            pub const fn from_value(val: $value_type) -> Self {
                Self($poly_type::new(val))
            }

            fn mod_irreducible_wide(poly: $poly_double) -> Self {
                // Reduce a double-width polynomial modulo the irreducible
                let mut p = poly.value();
                let irr = $irreducible;
                let n = $bitsize;
                
                // Find highest bit set in p
                let mut high_bit = 0;
                for i in (n..std::mem::size_of::<$value_double>() * 8).rev() {
                    if (p >> i) & 1 == 1 {
                        high_bit = i;
                        break;
                    }
                }
                
                // Reduce
                while high_bit >= n {
                    p ^= irr << (high_bit - n);
                    // Find new high bit
                    high_bit = 0;
                    for i in (n..std::mem::size_of::<$value_double>() * 8).rev() {
                        if (p >> i) & 1 == 1 {
                            high_bit = i;
                            break;
                        }
                    }
                    if high_bit < n {
                        break;
                    }
                }
                
                Self($poly_type::new(p as $value_type))
            }
        }

        impl BinaryFieldElement for $name {
            type Poly = $poly_type;

            fn zero() -> Self {
                Self($poly_type::zero())
            }

            fn one() -> Self {
                Self($poly_type::one())
            }

            fn from_poly(poly: Self::Poly) -> Self {
                // For from_poly, we assume the polynomial is already reduced
                Self(poly)
            }

            fn poly(&self) -> Self::Poly {
                self.0
            }

            fn add(&self, other: &Self) -> Self {
                Self(self.0.add(&other.0))
            }

            fn mul(&self, other: &Self) -> Self {
                // Perform full multiplication using double-width type
                let a_wide = $poly_double::from_value(self.0.value() as u64);
                let b_wide = $poly_double::from_value(other.0.value() as u64);
                let prod_wide = a_wide.mul(&b_wide);
                
                // Reduce modulo irreducible polynomial
                Self::mod_irreducible_wide(prod_wide)
            }

            fn inv(&self) -> Self {
                assert_ne!(self.0.value(), 0, "Cannot invert zero");
                
                // For binary fields, we can use Fermat's little theorem efficiently
                // a^(2^n - 2) = a^(-1) in GF(2^n)
                
                // For small fields, use direct exponentiation
                if $bitsize <= 16 {
                    let exp = (1u64 << $bitsize) - 2;
                    return self.pow(exp);
                }
                
                // For larger fields, use the addition chain method
                // 2^n - 2 = 2 + 4 + 8 + ... + 2^(n-1)
                
                // Start with a^2
                let mut acc = self.mul(self);
                let mut result = acc; // a^2
                
                // Compute a^4, a^8, ..., a^(2^(n-1)) and multiply them all
                for _ in 2..$bitsize {
                    acc = acc.mul(&acc); // Square to get next power of 2
                    result = result.mul(&acc);
                }
                
                result
            }

            fn pow(&self, mut exp: u64) -> Self {
                if *self == Self::zero() {
                    return Self::zero();
                }

                let mut result = Self::one();
                let mut base = *self;

                while exp > 0 {
                    if exp & 1 == 1 {
                        result = result.mul(&base);
                    }
                    base = base.mul(&base);
                    exp >>= 1;
                }

                result
            }
        }

        impl From<$value_type> for $name {
            fn from(val: $value_type) -> Self {
                Self::from_value(val)
            }
        }

        impl rand::distributions::Distribution<$name> for rand::distributions::Standard {
            fn sample<R: rand::Rng + ?Sized>(&self, rng: &mut R) -> $name {
                $name::from_value(rng.gen())
            }
        }
    };
}

impl_binary_elem!(BinaryElem16, BinaryPoly16, BinaryPoly32, u16, u32, IRREDUCIBLE_16, 16);
impl_binary_elem!(BinaryElem32, BinaryPoly32, BinaryPoly64, u32, u64, IRREDUCIBLE_32, 32);

// BinaryElem128 needs special handling since we don't have BinaryPoly256
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct BinaryElem128(BinaryPoly128);

impl BinaryElem128 {
    pub const fn from_value(val: u128) -> Self {
        Self(BinaryPoly128::new(val))
    }

    fn mod_irreducible(poly: BinaryPoly128) -> Self {
        // For 128-bit, we can't easily do wide multiplication
        // The irreducible polynomial x^128 + x^7 + x^2 + x + 1 has the x^128 implicit
        // So we just need to handle reduction if we had overflow
        let p = poly.value();
        
        // Since we're already in u128, we can't detect overflow from multiplication
        // The multiplication already truncated the result
        // This is a limitation of not having BinaryPoly256
        Self(BinaryPoly128::new(p))
    }
}

impl BinaryFieldElement for BinaryElem128 {
    type Poly = BinaryPoly128;

    fn zero() -> Self {
        Self(BinaryPoly128::zero())
    }

    fn one() -> Self {
        Self(BinaryPoly128::one())
    }

    fn from_poly(poly: Self::Poly) -> Self {
        Self(poly)
    }

    fn poly(&self) -> Self::Poly {
        self.0
    }

    fn add(&self, other: &Self) -> Self {
        Self(self.0.add(&other.0))
    }

    fn mul(&self, other: &Self) -> Self {
        // For 128-bit multiplication in GF(2^128), we need to handle reduction
        // The irreducible polynomial is x^128 + x^7 + x^2 + x + 1
        
        // First, do the multiplication (which may overflow)
        let a = self.0.value();
        let b = other.0.value();
        
        // We'll do this multiplication manually to handle the reduction
        let mut result = 0u128;
        let mut temp = a;
        
        for i in 0..128 {
            if (b >> i) & 1 == 1 {
                result ^= temp;
            }
            
            // Multiply temp by x (shift left by 1)
            let high_bit = (temp >> 127) & 1;
            temp <<= 1;
            
            // If we shifted out a 1, we need to reduce by the polynomial
            // x^128 = x^7 + x^2 + x + 1
            if high_bit == 1 {
                temp ^= 0b10000111; // x^7 + x^2 + x + 1
            }
        }
        
        Self(BinaryPoly128::new(result))
    }

    fn inv(&self) -> Self {
        assert_ne!(self.0.value(), 0, "Cannot invert zero");
        
        // For GF(2^128), use Fermat's little theorem
        // a^(2^128 - 2) = a^(-1)
        // 2^128 - 2 = 111...110 in binary (127 ones followed by a zero)
        
        // We can compute this efficiently using the fact that:
        // 2^128 - 2 = 2 + 4 + 8 + ... + 2^127
        // So a^(2^128 - 2) = a^2 * a^4 * a^8 * ... * a^(2^127)
        
        // Start with a^2
        let mut square = self.mul(self);
        let mut result = square; // a^2
        
        // Compute a^4, a^8, ..., a^(2^127) and multiply them all together
        for _ in 1..127 {
            square = square.mul(&square); // Square to get next power of 2
            result = result.mul(&square); // Multiply into result
        }
        
        result
    }

    fn pow(&self, mut exp: u64) -> Self {
        if *self == Self::zero() {
            return Self::zero();
        }

        let mut result = Self::one();
        let mut base = *self;

        while exp > 0 {
            if exp & 1 == 1 {
                result = result.mul(&base);
            }
            base = base.mul(&base);
            exp >>= 1;
        }

        result
    }
}

impl From<u128> for BinaryElem128 {
    fn from(val: u128) -> Self {
        Self::from_value(val)
    }
}

impl rand::distributions::Distribution<BinaryElem128> for rand::distributions::Standard {
    fn sample<R: rand::Rng + ?Sized>(&self, rng: &mut R) -> BinaryElem128 {
        BinaryElem128::from_value(rng.gen())
    }
}

// BinaryElem64 needs special handling
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub struct BinaryElem64(BinaryPoly64);

impl BinaryElem64 {
    pub const fn from_value(val: u64) -> Self {
        Self(BinaryPoly64::new(val))
    }
}

impl BinaryFieldElement for BinaryElem64 {
    type Poly = BinaryPoly64;

    fn zero() -> Self {
        Self(BinaryPoly64::zero())
    }

    fn one() -> Self {
        Self(BinaryPoly64::one())
    }

    fn from_poly(poly: Self::Poly) -> Self {
        // For now, no reduction for 64-bit field
        Self(poly)
    }

    fn poly(&self) -> Self::Poly {
        self.0
    }

    fn add(&self, other: &Self) -> Self {
        Self(self.0.add(&other.0))
    }

    fn mul(&self, other: &Self) -> Self {
        Self(self.0.mul(&other.0))
    }

    fn inv(&self) -> Self {
        assert_ne!(self.0.value(), 0, "Cannot invert zero");
        // Fermat's little theorem: a^(2^64 - 2) = a^(-1)
        self.pow(0xFFFFFFFFFFFFFFFE)
    }

    fn pow(&self, mut exp: u64) -> Self {
        if *self == Self::zero() {
            return Self::zero();
        }

        let mut result = Self::one();
        let mut base = *self;

        while exp > 0 {
            if exp & 1 == 1 {
                result = result.mul(&base);
            }
            base = base.mul(&base);
            exp >>= 1;
        }

        result
    }
}

// Field embeddings for Ligerito
impl From<BinaryElem16> for BinaryElem32 {
    fn from(elem: BinaryElem16) -> Self {
        BinaryElem32::from(elem.0.value() as u32)
    }
}

impl From<BinaryElem16> for BinaryElem64 {
    fn from(elem: BinaryElem16) -> Self {
        BinaryElem64(BinaryPoly64::new(elem.0.value() as u64))
    }
}

impl From<BinaryElem16> for BinaryElem128 {
    fn from(elem: BinaryElem16) -> Self {
        BinaryElem128::from(elem.0.value() as u128)
    }
}

impl From<BinaryElem32> for BinaryElem64 {
    fn from(elem: BinaryElem32) -> Self {
        BinaryElem64(BinaryPoly64::new(elem.0.value() as u64))
    }
}

impl From<BinaryElem32> for BinaryElem128 {
    fn from(elem: BinaryElem32) -> Self {
        BinaryElem128::from(elem.0.value() as u128)
    }
}

impl From<BinaryElem64> for BinaryElem128 {
    fn from(elem: BinaryElem64) -> Self {
        BinaryElem128::from(elem.0.value() as u128)
    }
}
