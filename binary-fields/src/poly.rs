use crate::BinaryPolynomial;

// Macro to implement binary polynomials for different sizes
macro_rules! impl_binary_poly {
    ($name:ident, $value_type:ty, $double_name:ident) => {
        #[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
        #[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
        pub struct $name($value_type);

        impl $name {
            pub const fn new(val: $value_type) -> Self {
                Self(val)
            }

            pub fn value(&self) -> $value_type {
                self.0
            }

            pub fn shl(&self, n: u32) -> Self {
                Self(self.0 << n)
            }

            pub fn shr(&self, n: u32) -> Self {
                Self(self.0 >> n)
            }

            pub fn leading_zeros(&self) -> u32 {
                self.0.leading_zeros()
            }

            #[allow(dead_code)]
            pub fn split(&self) -> (Self, Self) {
                let half_bits = std::mem::size_of::<$value_type>() * 4;
                let mask = ((1u64 << half_bits) - 1) as $value_type;
                let lo = Self(self.0 & mask);
                let hi = Self(self.0 >> half_bits);
                (hi, lo)
            }
        }

        impl BinaryPolynomial for $name {
            type Value = $value_type;

            fn zero() -> Self {
                Self(0)
            }

            fn one() -> Self {
                Self(1)
            }

            fn from_value(val: u64) -> Self {
                Self(val as $value_type)
            }

            fn value(&self) -> Self::Value {
                self.0
            }

            fn add(&self, other: &Self) -> Self {
                Self(self.0 ^ other.0)
            }

            fn mul(&self, other: &Self) -> Self {
                // Software carryless multiplication
                // Note: This will truncate if the result doesn't fit
                let mut result = 0 as $value_type;
                let a = self.0;
                let b = other.0;

                for i in 0..std::mem::size_of::<$value_type>() * 8 {
                    if (b >> i) & 1 == 1 {
                        result ^= a.wrapping_shl(i as u32);
                    }
                }

                Self(result)
            }

            fn div_rem(&self, divisor: &Self) -> (Self, Self) {
                assert_ne!(divisor.0, 0, "Division by zero");

                let mut quotient = Self::zero();
                let mut remainder = *self;

                if remainder.0 == 0 {
                    return (quotient, remainder);
                }

                let divisor_bits = (std::mem::size_of::<$value_type>() * 8) as u32 - divisor.leading_zeros();
                let mut remainder_bits = (std::mem::size_of::<$value_type>() * 8) as u32 - remainder.leading_zeros();

                while remainder_bits >= divisor_bits && remainder.0 != 0 {
                    let shift = remainder_bits - divisor_bits;
                    quotient.0 |= 1 << shift;
                    remainder.0 ^= divisor.0 << shift;
                    remainder_bits = (std::mem::size_of::<$value_type>() * 8) as u32 - remainder.leading_zeros();
                }

                (quotient, remainder)
            }
        }

        impl From<$value_type> for $name {
            fn from(val: $value_type) -> Self {
                Self(val)
            }
        }
    };
}

// Define polynomial types
impl_binary_poly!(BinaryPoly16, u16, BinaryPoly32);
impl_binary_poly!(BinaryPoly32, u32, BinaryPoly64);

// BinaryPoly64 with SIMD support
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct BinaryPoly64(u64);

impl BinaryPoly64 {
    pub const fn new(val: u64) -> Self {
        Self(val)
    }

    pub fn value(&self) -> u64 {
        self.0
    }

    pub fn shl(&self, n: u32) -> Self {
        Self(self.0 << n)
    }

    pub fn shr(&self, n: u32) -> Self {
        Self(self.0 >> n)
    }

    pub fn leading_zeros(&self) -> u32 {
        self.0.leading_zeros()
    }
}

impl BinaryPolynomial for BinaryPoly64 {
    type Value = u64;

    fn zero() -> Self {
        Self(0)
    }

    fn one() -> Self {
        Self(1)
    }

    fn from_value(val: u64) -> Self {
        Self(val)
    }

    fn value(&self) -> Self::Value {
        self.0
    }

    fn add(&self, other: &Self) -> Self {
        Self(self.0 ^ other.0)
    }

    fn mul(&self, other: &Self) -> Self {
        #[cfg(target_arch = "x86_64")]
        {
            use crate::simd::carryless_mul;
            carryless_mul(*self, *other).truncate_to_64()
        }

        #[cfg(not(target_arch = "x86_64"))]
        {
            // Software fallback
            let mut result = 0u64;
            let a = self.0;
            let b = other.0;

            for i in 0..64 {
                if (b >> i) & 1 == 1 {
                    result ^= a << i;
                }
            }

            Self(result)
        }
    }

    fn div_rem(&self, divisor: &Self) -> (Self, Self) {
        assert_ne!(divisor.0, 0, "Division by zero");

        let mut quotient = Self::zero();
        let mut remainder = *self;

        if remainder.0 == 0 {
            return (quotient, remainder);
        }

        let divisor_bits = 64 - divisor.leading_zeros();
        let mut remainder_bits = 64 - remainder.leading_zeros();

        while remainder_bits >= divisor_bits && remainder.0 != 0 {
            let shift = remainder_bits - divisor_bits;
            quotient.0 |= 1 << shift;
            remainder.0 ^= divisor.0 << shift;
            remainder_bits = 64 - remainder.leading_zeros();
        }

        (quotient, remainder)
    }
}

// BinaryPoly128
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct BinaryPoly128(u128);

impl BinaryPoly128 {
    pub const fn new(val: u128) -> Self {
        Self(val)
    }

    pub fn value(&self) -> u128 {
        self.0
    }

    pub fn truncate_to_64(&self) -> BinaryPoly64 {
        BinaryPoly64::new(self.0 as u64)
    }

    pub fn split(&self) -> (BinaryPoly64, BinaryPoly64) {
        let lo = BinaryPoly64::new(self.0 as u64);
        let hi = BinaryPoly64::new((self.0 >> 64) as u64);
        (hi, lo)
    }

    pub fn leading_zeros(&self) -> u32 {
        self.0.leading_zeros()
    }
}

impl BinaryPolynomial for BinaryPoly128 {
    type Value = u128;

    fn zero() -> Self {
        Self(0)
    }

    fn one() -> Self {
        Self(1)
    }

    fn from_value(val: u64) -> Self {
        Self(val as u128)
    }

    fn value(&self) -> Self::Value {
        self.0
    }

    fn add(&self, other: &Self) -> Self {
        Self(self.0 ^ other.0)
    }

    fn mul(&self, other: &Self) -> Self {
        // Use Karatsuba for 128-bit multiplication
        let (a_hi, a_lo) = self.split();
        let (b_hi, b_lo) = other.split();

        let z0 = a_lo.mul(&b_lo);
        let z2 = a_hi.mul(&b_hi);
        let z1 = a_lo.add(&a_hi).mul(&b_lo.add(&b_hi)).add(&z0).add(&z2);

        let mut result = z0.value() as u128;
        result ^= (z1.value() as u128) << 64;
        // Note: z2 would overflow if shifted by 128, but in carryless multiplication
        // the result of 64x64 is at most 127 bits, so we don't need the full shift

        Self(result)
    }

    fn div_rem(&self, divisor: &Self) -> (Self, Self) {
        assert_ne!(divisor.0, 0, "Division by zero");

        let mut quotient = Self::zero();
        let mut remainder = *self;

        if remainder.0 == 0 {
            return (quotient, remainder);
        }

        let divisor_bits = 128 - divisor.leading_zeros();
        let mut remainder_bits = 128 - remainder.leading_zeros();

        while remainder_bits >= divisor_bits && remainder.0 != 0 {
            let shift = remainder_bits - divisor_bits;
            quotient.0 |= 1u128 << shift;
            remainder.0 ^= divisor.0 << shift;
            remainder_bits = 128 - remainder.leading_zeros();
        }

        (quotient, remainder)
    }
}

// BinaryPoly256 for intermediate calculations
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub struct BinaryPoly256 {
    hi: u128,
    lo: u128,
}

impl BinaryPoly256 {
    pub fn from_parts(hi: u128, lo: u128) -> Self {
        Self { hi, lo }
    }

    pub fn split(&self) -> (BinaryPoly128, BinaryPoly128) {
        (BinaryPoly128::new(self.hi), BinaryPoly128::new(self.lo))
    }
}
