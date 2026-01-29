//! Curve abstraction for OSST
//!
//! Defines traits for curve operations, allowing OSST to work with different
//! elliptic curve backends:
//! - ristretto255 (Polkadot/sr25519 compatible)
//! - Pallas (Zcash Orchard compatible)
//! - Jubjub (Zcash Sapling compatible) - future

use core::fmt::Debug;

/// Scalar field element trait
pub trait OsstScalar: Clone + Debug + Sized + PartialEq + Send + Sync {
    /// The zero element
    fn zero() -> Self;

    /// The one element
    fn one() -> Self;

    /// Create from u32
    fn from_u32(v: u32) -> Self;

    /// Addition
    fn add(&self, other: &Self) -> Self;

    /// Subtraction
    fn sub(&self, other: &Self) -> Self;

    /// Multiplication
    fn mul(&self, other: &Self) -> Self;

    /// Negation
    fn neg(&self) -> Self;

    /// Compute multiplicative inverse
    fn invert(&self) -> Self;

    /// Generate random scalar
    fn random<R: rand_core::RngCore + rand_core::CryptoRng>(rng: &mut R) -> Self;

    /// Create from 64-byte wide hash output (reduction mod order)
    fn from_bytes_wide(bytes: &[u8; 64]) -> Self;

    /// Serialize to bytes
    fn to_bytes(&self) -> [u8; 32];

    /// Deserialize from canonical bytes
    fn from_canonical_bytes(bytes: &[u8; 32]) -> Option<Self>;
}

/// Curve point trait
pub trait OsstPoint: Clone + Debug + Sized + PartialEq + Send + Sync {
    type Scalar: OsstScalar;

    /// Compressed point size in bytes (32 for pallas/ristretto, 33 for secp256k1)
    const COMPRESSED_SIZE: usize;

    /// The identity element
    fn identity() -> Self;

    /// The generator point
    fn generator() -> Self;

    /// Scalar multiplication
    fn mul_scalar(&self, scalar: &Self::Scalar) -> Self;

    /// Point addition
    fn add(&self, other: &Self) -> Self;

    /// Multiscalar multiplication (optimized)
    fn multiscalar_mul(scalars: &[Self::Scalar], points: &[Self]) -> Self;

    /// Compress to bytes (32 bytes for most curves)
    fn compress(&self) -> [u8; 32];

    /// Decompress from bytes (32 bytes for most curves)
    fn decompress(bytes: &[u8; 32]) -> Option<Self>;

    /// Compress to variable-size bytes (for secp256k1 compatibility)
    fn compress_vec(&self) -> alloc::vec::Vec<u8> {
        self.compress().to_vec()
    }

    /// Decompress from variable-size bytes (for secp256k1 compatibility)
    fn decompress_slice(bytes: &[u8]) -> Option<Self> {
        if bytes.len() == 32 {
            let arr: [u8; 32] = bytes.try_into().ok()?;
            Self::decompress(&arr)
        } else {
            None
        }
    }
}

extern crate alloc;

/// Complete curve backend
pub trait OsstCurve: Clone + Debug + Default {
    type Scalar: OsstScalar;
    type Point: OsstPoint<Scalar = Self::Scalar>;
}

// ============================================================================
// Ristretto255 implementation
// ============================================================================

#[cfg(feature = "ristretto255")]
pub mod ristretto {
    use super::*;
    use curve25519_dalek::{
        constants::RISTRETTO_BASEPOINT_POINT,
        ristretto::{CompressedRistretto, RistrettoPoint},
        scalar::Scalar,
        traits::MultiscalarMul,
    };

    impl OsstScalar for Scalar {
        fn zero() -> Self {
            Scalar::ZERO
        }

        fn one() -> Self {
            Scalar::ONE
        }

        fn from_u32(v: u32) -> Self {
            Scalar::from(v)
        }

        fn add(&self, other: &Self) -> Self {
            self + other
        }

        fn sub(&self, other: &Self) -> Self {
            self - other
        }

        fn mul(&self, other: &Self) -> Self {
            self * other
        }

        fn neg(&self) -> Self {
            -self
        }

        fn invert(&self) -> Self {
            Scalar::invert(self)
        }

        fn random<R: rand_core::RngCore + rand_core::CryptoRng>(rng: &mut R) -> Self {
            Scalar::random(rng)
        }

        fn from_bytes_wide(bytes: &[u8; 64]) -> Self {
            Scalar::from_bytes_mod_order_wide(bytes)
        }

        fn to_bytes(&self) -> [u8; 32] {
            Scalar::to_bytes(self)
        }

        fn from_canonical_bytes(bytes: &[u8; 32]) -> Option<Self> {
            Scalar::from_canonical_bytes(*bytes).into_option()
        }
    }

    impl OsstPoint for RistrettoPoint {
        type Scalar = Scalar;

        const COMPRESSED_SIZE: usize = 32;

        fn identity() -> Self {
            curve25519_dalek::traits::Identity::identity()
        }

        fn generator() -> Self {
            RISTRETTO_BASEPOINT_POINT
        }

        fn mul_scalar(&self, scalar: &Self::Scalar) -> Self {
            self * scalar
        }

        fn add(&self, other: &Self) -> Self {
            self + other
        }

        fn multiscalar_mul(scalars: &[Self::Scalar], points: &[Self]) -> Self {
            <RistrettoPoint as MultiscalarMul>::multiscalar_mul(scalars, points)
        }

        fn compress(&self) -> [u8; 32] {
            RistrettoPoint::compress(self).to_bytes()
        }

        fn decompress(bytes: &[u8; 32]) -> Option<Self> {
            CompressedRistretto::from_slice(bytes).ok()?.decompress()
        }
    }

    /// Ristretto255 curve backend
    #[derive(Clone, Debug, Default)]
    pub struct Ristretto255;

    impl OsstCurve for Ristretto255 {
        type Scalar = Scalar;
        type Point = RistrettoPoint;
    }
}

// ============================================================================
// Pallas implementation (Zcash Orchard)
// ============================================================================

#[cfg(feature = "pallas")]
pub mod pallas {
    use super::*;
    use pasta_curves::{
        group::{
            ff::{Field, FromUniformBytes, PrimeField},
            Group, GroupEncoding,
        },
        pallas::{Point, Scalar},
    };

    impl OsstScalar for Scalar {
        fn zero() -> Self {
            Scalar::ZERO
        }

        fn one() -> Self {
            Scalar::ONE
        }

        fn from_u32(v: u32) -> Self {
            Scalar::from(v as u64)
        }

        fn add(&self, other: &Self) -> Self {
            *self + *other
        }

        fn sub(&self, other: &Self) -> Self {
            *self - *other
        }

        fn mul(&self, other: &Self) -> Self {
            *self * *other
        }

        fn neg(&self) -> Self {
            -(*self)
        }

        fn invert(&self) -> Self {
            Field::invert(self).unwrap_or(Scalar::ZERO)
        }

        fn random<R: rand_core::RngCore + rand_core::CryptoRng>(rng: &mut R) -> Self {
            <Scalar as Field>::random(rng)
        }

        fn from_bytes_wide(bytes: &[u8; 64]) -> Self {
            <Scalar as FromUniformBytes<64>>::from_uniform_bytes(bytes)
        }

        fn to_bytes(&self) -> [u8; 32] {
            self.to_repr()
        }

        fn from_canonical_bytes(bytes: &[u8; 32]) -> Option<Self> {
            Scalar::from_repr(*bytes).into_option()
        }
    }

    impl OsstPoint for Point {
        type Scalar = Scalar;

        const COMPRESSED_SIZE: usize = 32;

        fn identity() -> Self {
            <Point as Group>::identity()
        }

        fn generator() -> Self {
            <Point as Group>::generator()
        }

        fn mul_scalar(&self, scalar: &Self::Scalar) -> Self {
            self * scalar
        }

        fn add(&self, other: &Self) -> Self {
            *self + *other
        }

        fn multiscalar_mul(scalars: &[Self::Scalar], points: &[Self]) -> Self {
            // Basic implementation - could use more optimized version
            scalars
                .iter()
                .zip(points.iter())
                .fold(<Point as Group>::identity(), |acc, (s, p)| {
                    acc + p.mul_scalar(s)
                })
        }

        fn compress(&self) -> [u8; 32] {
            self.to_bytes()
        }

        fn decompress(bytes: &[u8; 32]) -> Option<Self> {
            Point::from_bytes(bytes).into_option()
        }
    }

    /// Pallas curve backend (Zcash Orchard)
    #[derive(Clone, Debug, Default)]
    pub struct PallasCurve;

    impl OsstCurve for PallasCurve {
        type Scalar = Scalar;
        type Point = Point;
    }
}

// ============================================================================
// secp256k1 implementation (Bitcoin)
// ============================================================================

#[cfg(feature = "secp256k1")]
pub mod secp256k1 {
    use super::*;
    use alloc::vec::Vec;
    use k256::{
        elliptic_curve::{
            ops::Reduce,
            sec1::{FromEncodedPoint, ToEncodedPoint},
            Field, PrimeField,
        },
        ProjectivePoint, Scalar, U256,
    };

    impl OsstScalar for Scalar {
        fn zero() -> Self {
            Scalar::ZERO
        }

        fn one() -> Self {
            Scalar::ONE
        }

        fn from_u32(v: u32) -> Self {
            Scalar::from(v as u64)
        }

        fn add(&self, other: &Self) -> Self {
            *self + *other
        }

        fn sub(&self, other: &Self) -> Self {
            *self - *other
        }

        fn mul(&self, other: &Self) -> Self {
            *self * *other
        }

        fn neg(&self) -> Self {
            -(*self)
        }

        fn invert(&self) -> Self {
            <Scalar as Field>::invert(self).unwrap_or(Scalar::ZERO)
        }

        fn random<R: rand_core::RngCore + rand_core::CryptoRng>(rng: &mut R) -> Self {
            <Scalar as Field>::random(rng)
        }

        fn from_bytes_wide(bytes: &[u8; 64]) -> Self {
            // reduce 512-bit value modulo curve order
            let wide = U256::from_be_slice(&bytes[32..64]);
            <Scalar as Reduce<U256>>::reduce(wide)
        }

        fn to_bytes(&self) -> [u8; 32] {
            self.to_bytes().into()
        }

        fn from_canonical_bytes(bytes: &[u8; 32]) -> Option<Self> {
            let arr: &k256::FieldBytes = bytes.into();
            Scalar::from_repr(*arr).into_option()
        }
    }

    impl OsstPoint for ProjectivePoint {
        type Scalar = Scalar;

        // secp256k1 uses 33-byte compressed points
        const COMPRESSED_SIZE: usize = 33;

        fn identity() -> Self {
            Self::IDENTITY
        }

        fn generator() -> Self {
            Self::GENERATOR
        }

        fn mul_scalar(&self, scalar: &Self::Scalar) -> Self {
            self * scalar
        }

        fn add(&self, other: &Self) -> Self {
            *self + *other
        }

        fn multiscalar_mul(scalars: &[Self::Scalar], points: &[Self]) -> Self {
            scalars
                .iter()
                .zip(points.iter())
                .fold(Self::IDENTITY, |acc, (s, p)| acc + p.mul_scalar(s))
        }

        // for secp256k1, compress returns first 32 bytes (x-coord)
        // use compress_vec for full 33-byte representation
        fn compress(&self) -> [u8; 32] {
            let affine = self.to_affine();
            let encoded = affine.to_encoded_point(true);
            let bytes = encoded.as_bytes();
            // return x-coordinate (skip the 0x02/0x03 prefix)
            let mut result = [0u8; 32];
            if bytes.len() >= 33 {
                result.copy_from_slice(&bytes[1..33]);
            }
            result
        }

        fn decompress(bytes: &[u8; 32]) -> Option<Self> {
            // try to decompress assuming even y (0x02 prefix)
            let mut compressed = [0u8; 33];
            compressed[0] = 0x02;
            compressed[1..].copy_from_slice(bytes);
            Self::decompress_slice(&compressed)
        }

        fn compress_vec(&self) -> Vec<u8> {
            let affine = self.to_affine();
            let encoded = affine.to_encoded_point(true);
            encoded.as_bytes().to_vec()
        }

        fn decompress_slice(bytes: &[u8]) -> Option<Self> {
            use k256::EncodedPoint;
            if bytes.len() == 33 {
                let encoded = EncodedPoint::from_bytes(bytes).ok()?;
                let affine = k256::AffinePoint::from_encoded_point(&encoded);
                if affine.is_some().into() {
                    Some(ProjectivePoint::from(affine.unwrap()))
                } else {
                    None
                }
            } else if bytes.len() == 32 {
                // assume even y
                let mut compressed = [0u8; 33];
                compressed[0] = 0x02;
                compressed[1..].copy_from_slice(bytes);
                Self::decompress_slice(&compressed)
            } else {
                None
            }
        }
    }

    /// secp256k1 curve backend (Bitcoin)
    #[derive(Clone, Debug, Default)]
    pub struct Secp256k1Curve;

    impl OsstCurve for Secp256k1Curve {
        type Scalar = Scalar;
        type Point = ProjectivePoint;
    }
}

// ============================================================================
// decaf377 implementation (Penumbra)
// ============================================================================

#[cfg(feature = "decaf377")]
pub mod decaf377 {
    use super::*;
    use ::decaf377::{Element, Fr};

    impl OsstScalar for Fr {
        fn zero() -> Self {
            Fr::ZERO
        }

        fn one() -> Self {
            Fr::ONE
        }

        fn from_u32(v: u32) -> Self {
            Fr::from(v as u64)
        }

        fn add(&self, other: &Self) -> Self {
            *self + *other
        }

        fn sub(&self, other: &Self) -> Self {
            *self - *other
        }

        fn mul(&self, other: &Self) -> Self {
            *self * *other
        }

        fn neg(&self) -> Self {
            -(*self)
        }

        fn invert(&self) -> Self {
            self.inverse().unwrap_or(Fr::ZERO)
        }

        fn random<R: rand_core::RngCore + rand_core::CryptoRng>(rng: &mut R) -> Self {
            let mut bytes = [0u8; 32];
            rng.fill_bytes(&mut bytes);
            Fr::from_le_bytes_mod_order(&bytes)
        }

        fn from_bytes_wide(bytes: &[u8; 64]) -> Self {
            // reduce 512-bit value mod field order
            // decaf377 Fr is ~253 bits, so we use the lower 32 bytes
            let mut arr = [0u8; 32];
            arr.copy_from_slice(&bytes[32..64]);
            Fr::from_le_bytes_mod_order(&arr)
        }

        fn to_bytes(&self) -> [u8; 32] {
            Fr::to_bytes(self)
        }

        fn from_canonical_bytes(bytes: &[u8; 32]) -> Option<Self> {
            Fr::from_bytes_checked(bytes).ok()
        }
    }

    impl OsstPoint for Element {
        type Scalar = Fr;

        const COMPRESSED_SIZE: usize = 32;

        fn identity() -> Self {
            Element::IDENTITY
        }

        fn generator() -> Self {
            Element::GENERATOR
        }

        fn mul_scalar(&self, scalar: &Self::Scalar) -> Self {
            *self * *scalar
        }

        fn add(&self, other: &Self) -> Self {
            *self + *other
        }

        fn multiscalar_mul(scalars: &[Self::Scalar], points: &[Self]) -> Self {
            scalars
                .iter()
                .zip(points.iter())
                .fold(Element::IDENTITY, |acc, (s, p)| acc + (*p * *s))
        }

        fn compress(&self) -> [u8; 32] {
            self.vartime_compress().0
        }

        fn decompress(bytes: &[u8; 32]) -> Option<Self> {
            ::decaf377::Encoding(*bytes).vartime_decompress().ok()
        }
    }

    /// decaf377 curve backend (Penumbra)
    #[derive(Clone, Debug, Default)]
    pub struct Decaf377Curve;

    impl OsstCurve for Decaf377Curve {
        type Scalar = Fr;
        type Point = Element;
    }
}
