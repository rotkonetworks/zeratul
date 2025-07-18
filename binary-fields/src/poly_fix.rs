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

            #[allow(dead_code)]
            pub fn shl(&self, n: u32) -> Self {
                Self(self.0 << n)
            }

            #[allow(dead_code)]
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
