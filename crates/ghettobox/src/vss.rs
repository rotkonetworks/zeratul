//! verifiable secret sharing using shamir's scheme over GF(256)
//!
//! 2-of-3 threshold for distributed TPM nodes

use crate::{Error, Result};
use rand::RngCore;

/// number of shares to create
pub const SHARE_COUNT: usize = 3;

/// threshold for reconstruction
pub const THRESHOLD: usize = 2;

/// a single share from VSS
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct Share {
    /// share index (1-indexed, must be non-zero)
    pub index: u8,
    /// share data (same length as secret)
    pub data: Vec<u8>,
}

/// GF(256) multiplication using AES polynomial (x^8 + x^4 + x^3 + x + 1)
fn gf256_mul(a: u8, b: u8) -> u8 {
    let mut result = 0u8;
    let mut a = a;
    let mut b = b;

    while b != 0 {
        if b & 1 != 0 {
            result ^= a;
        }
        let hi = a & 0x80;
        a <<= 1;
        if hi != 0 {
            a ^= 0x1b; // AES polynomial
        }
        b >>= 1;
    }
    result
}

/// GF(256) multiplicative inverse using extended euclidean
fn gf256_inv(a: u8) -> u8 {
    if a == 0 {
        return 0; // 0 has no inverse
    }
    // use exponentiation: a^254 = a^(-1) in GF(256)
    let mut result = a;
    for _ in 0..6 {
        result = gf256_mul(result, result);
        result = gf256_mul(result, a);
    }
    gf256_mul(result, result)
}

/// GF(256) division
fn gf256_div(a: u8, b: u8) -> u8 {
    gf256_mul(a, gf256_inv(b))
}

/// evaluate polynomial at point x
fn poly_eval(coeffs: &[u8], x: u8) -> u8 {
    let mut result = 0u8;
    let mut x_power = 1u8;

    for &coeff in coeffs {
        result ^= gf256_mul(coeff, x_power);
        x_power = gf256_mul(x_power, x);
    }
    result
}

/// lagrange interpolation at x=0 to recover secret
fn lagrange_interpolate(shares: &[(u8, u8)]) -> u8 {
    let mut result = 0u8;

    for (i, &(xi, yi)) in shares.iter().enumerate() {
        let mut num = 1u8;
        let mut den = 1u8;

        for (j, &(xj, _)) in shares.iter().enumerate() {
            if i != j {
                num = gf256_mul(num, xj);           // (0 - xj) = xj in GF(256)
                den = gf256_mul(den, xi ^ xj);      // (xi - xj)
            }
        }

        let lagrange = gf256_mul(yi, gf256_div(num, den));
        result ^= lagrange;
    }

    result
}

/// split a secret into 3 shares with 2-of-3 threshold
pub fn split_secret(secret: &[u8]) -> Result<[Share; SHARE_COUNT]> {
    if secret.len() != 32 {
        return Err(Error::InvalidSecretLength);
    }

    let mut rng = rand::thread_rng();
    let mut shares = [
        Share { index: 1, data: vec![0u8; 32] },
        Share { index: 2, data: vec![0u8; 32] },
        Share { index: 3, data: vec![0u8; 32] },
    ];

    // for each byte of the secret, create a polynomial and evaluate at x=1,2,3
    for i in 0..32 {
        // polynomial: f(x) = secret[i] + random * x
        let mut random_coeff = [0u8; 1];
        rng.fill_bytes(&mut random_coeff);

        let coeffs = [secret[i], random_coeff[0]];

        shares[0].data[i] = poly_eval(&coeffs, 1);
        shares[1].data[i] = poly_eval(&coeffs, 2);
        shares[2].data[i] = poly_eval(&coeffs, 3);
    }

    Ok(shares)
}

/// reconstruct secret from at least 2 shares
pub fn combine_shares(shares: &[Share]) -> Result<[u8; 32]> {
    if shares.len() < THRESHOLD {
        return Err(Error::NotEnoughShares {
            have: shares.len(),
            need: THRESHOLD,
        });
    }

    let mut secret = [0u8; 32];

    // use first THRESHOLD shares
    let used_shares: Vec<_> = shares.iter().take(THRESHOLD).collect();

    for i in 0..32 {
        let points: Vec<(u8, u8)> = used_shares
            .iter()
            .map(|s| (s.index, s.data[i]))
            .collect();

        secret[i] = lagrange_interpolate(&points);
    }

    Ok(secret)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_gf256_ops() {
        // test multiplication
        assert_eq!(gf256_mul(0, 0), 0);
        assert_eq!(gf256_mul(1, 1), 1);
        assert_eq!(gf256_mul(2, 2), 4);

        // test inverse
        for a in 1..=255u8 {
            let inv = gf256_inv(a);
            assert_eq!(gf256_mul(a, inv), 1, "inverse failed for {}", a);
        }
    }

    #[test]
    fn test_split_combine() {
        let secret = [42u8; 32];
        let shares = split_secret(&secret).unwrap();

        // any 2 shares should work
        let recovered = combine_shares(&[shares[0].clone(), shares[1].clone()]).unwrap();
        assert_eq!(secret, recovered);

        let recovered = combine_shares(&[shares[0].clone(), shares[2].clone()]).unwrap();
        assert_eq!(secret, recovered);

        let recovered = combine_shares(&[shares[1].clone(), shares[2].clone()]).unwrap();
        assert_eq!(secret, recovered);

        // all 3 also works
        let recovered = combine_shares(&shares).unwrap();
        assert_eq!(secret, recovered);
    }

    #[test]
    fn test_random_secret() {
        let mut secret = [0u8; 32];
        rand::thread_rng().fill_bytes(&mut secret);

        let shares = split_secret(&secret).unwrap();
        let recovered = combine_shares(&[shares[0].clone(), shares[2].clone()]).unwrap();
        assert_eq!(secret, recovered);
    }

    #[test]
    fn test_not_enough_shares() {
        let secret = [42u8; 32];
        let shares = split_secret(&secret).unwrap();

        // 1 share is not enough
        let result = combine_shares(&[shares[0].clone()]);
        assert!(result.is_err());
    }
}
