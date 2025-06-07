use crate::BinaryPolynomial;

// Macro to implement binary polynomials for different sizes
macro_rules! impl_binary_poly {
    ($name:ident, $value_type:ty, $double_name:ident) => {
        #[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
        #[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
        pub struct $name($value_type);

        impl $name {
            pub const fn from_value(val: $value_type) -> Self {
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

            pub fn split(&self) -> (Self, Self) {
                let half_bits = std::mem::size_of::<$value_type>() * 4;
                let mask = (1 << half_bits) - 1;
                let lo = Self(self.0 & mask);
                let hi = Self(self.0 >> half_bits);
                (hi, lo)
            }

            pub fn into_double(self) -> $double_name {
                $double_name::from_value(self.0 as _)
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

            fn value(&self) -> Self::Value {
                self.0
            }

            fn add(&self, other: &Self) -> Self {
                Self(self.0 ^ other.0)
            }

            fn mul(&self, other: &Self) -> Self {
                // For smaller types, we'll use standard multiplication
                // and rely on the field element to do reduction
                #[cfg(target_arch = "x86_64")]
                {
                    use crate::simd::carryless_mul;
                    let result = carryless_mul(
                        BinaryPoly64::from_value(self.value() as u64),
                        BinaryPoly64::from_value(other.value() as u64)
                    );

                    // Handle masking without overflow
                    let bit_size = std::mem::size_of::<$value_type>() * 8;
                    if bit_size * 2 >= 128 {
                        // For 64-bit types, just truncate
                        Self::from_value(result.value() as $value_type)
                    } else {
                        // For smaller types, apply mask
                        let mask = (1u128 << (bit_size * 2)) - 1;
                        Self::from_value((result.value() & mask) as $value_type)
                    }
                }

                #[cfg(not(target_arch = "x86_64"))]
                {
                    // Fallback software implementation
                    software_carryless_mul(*self, *other)
                }
            }

            fn div_rem(&self, _divisor: &Self) -> (Self, Self) {
                assert_ne!(_divisor.0, 0, "Division by zero");

                let mut remainder = *self;
                let mut quotient = Self::zero();

                let divisor_bits = (std::mem::size_of::<$value_type>() * 8) - _divisor.0.leading_zeros() as usize;
                let remainder_bits = (std::mem::size_of::<$value_type>() * 8) - remainder.0.leading_zeros() as usize;
                
                if remainder_bits < divisor_bits {
                    return (quotient, remainder);
                }
                
                let mut shift = remainder_bits - divisor_bits;

                loop {
                    if remainder.0 & (1 << (shift + divisor_bits - 1)) != 0 {
                        quotient.0 |= 1 << shift;
                        remainder.0 ^= _divisor.0 << shift;
                    }
                    if shift == 0 { break; }
                    shift -= 1;
                }

                (quotient, remainder)
            }
        }

        impl From<$value_type> for $name {
            fn from(val: $value_type) -> Self {
                Self::from_value(val)
            }
        }
    };
}

// Software carryless multiplication fallback
fn software_carryless_mul<T: BinaryPolynomial>(_a: T, _b: T) -> T {
    // This is a placeholder - actual implementation would do bit-by-bit multiplication
    // For now, return zero to avoid panics
    T::zero()
}

// Define polynomial types with their double-width versions
impl_binary_poly!(BinaryPoly8, u8, BinaryPoly16);
impl_binary_poly!(BinaryPoly16, u16, BinaryPoly32);
impl_binary_poly!(BinaryPoly32, u32, BinaryPoly64);
impl_binary_poly!(BinaryPoly64, u64, BinaryPoly128);

// BinaryPoly128 needs special handling as it doubles to BinaryPoly256
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct BinaryPoly128(u128);

impl BinaryPoly128 {
    pub const fn from_value(val: u128) -> Self {
        Self(val)
    }

    pub fn value(&self) -> u128 {
        self.0
    }

    pub fn shl(&self, n: u32) -> Self {
        Self(self.0 << n)
    }

    pub fn shr(&self, n: u32) -> Self {
        Self(self.0 >> n)
    }

    pub fn into_double(self) -> BinaryPoly256 {
        BinaryPoly256::from_parts(0, self.0)
    }

    pub fn split(&self) -> (BinaryPoly64, BinaryPoly64) {
        let lo = BinaryPoly64::from_value(self.0 as u64);
        let hi = BinaryPoly64::from_value((self.0 >> 64) as u64);
        (hi, lo)
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
        (BinaryPoly128(self.hi), BinaryPoly128(self.lo))
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

    fn value(&self) -> Self::Value {
        self.0
    }

    fn add(&self, other: &Self) -> Self {
        Self(self.0 ^ other.0)
    }

    fn mul(&self, other: &Self) -> Self {
        // Split into 64-bit halves
        let a_lo = self.0 as u64;
        let a_hi = (self.0 >> 64) as u64;
        let b_lo = other.0 as u64;
        let b_hi = (other.0 >> 64) as u64;
        
        // Use carryless multiplication on 64-bit values
        #[cfg(target_arch = "x86_64")]
        {
            use crate::simd::carryless_mul;
            
            // Perform 64x64 -> 128 bit multiplications
            let z0 = carryless_mul(
                BinaryPoly64::from_value(a_lo),
                BinaryPoly64::from_value(b_lo)
            ).value();
            
            let z1_1 = carryless_mul(
                BinaryPoly64::from_value(a_hi),
                BinaryPoly64::from_value(b_lo)
            ).value();
            
            let z1_2 = carryless_mul(
                BinaryPoly64::from_value(a_lo),
                BinaryPoly64::from_value(b_hi)
            ).value();
            
            // Combine results
            let mut result: u128 = z0;
            
            // Add middle terms shifted by 64
            result ^= (z1_1 & 0xFFFFFFFFFFFFFFFF) << 64;
            result ^= (z1_2 & 0xFFFFFFFFFFFFFFFF) << 64;
            
            Self(result)
        }
        
        #[cfg(not(target_arch = "x86_64"))]
        {
            // Fallback implementation
            todo!("Implement 128-bit multiplication for non-x86_64")
        }
    }

    fn div_rem(&self, _divisor: &Self) -> (Self, Self) {
        todo!("Implement 128-bit division")
    }
}
