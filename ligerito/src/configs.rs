//! Hardcoded configurations for different proof sizes

use crate::data_structures::{ProverConfig, VerifierConfig};
use binary_fields::BinaryFieldElement;
use reed_solomon::reed_solomon;
use std::marker::PhantomData;

/// Create minimal configuration for 2^12 polynomial (for testing/demos)
pub fn hardcoded_config_12<T, U>(
    _t: PhantomData<T>,
    _u: PhantomData<U>,
) -> ProverConfig<T, U>
where
    T: BinaryFieldElement,
    U: BinaryFieldElement,
{
    let recursive_steps = 1;
    let inv_rate = 4;

    let initial_dims = (1 << 8, 1 << 4);  // (256, 16)
    let dims = vec![(1 << 6, 1 << 2)];    // (64, 4)

    let initial_k = 4;
    let ks = vec![2];

    let initial_reed_solomon = reed_solomon::<T>(initial_dims.0, initial_dims.0 * inv_rate);
    let reed_solomon_codes = vec![
        reed_solomon::<U>(dims[0].0, dims[0].0 * inv_rate),
    ];

    ProverConfig {
        recursive_steps,
        initial_dims,
        dims,
        initial_k,
        ks,
        initial_reed_solomon,
        reed_solomon_codes,
    }
}

pub fn hardcoded_config_12_verifier() -> VerifierConfig {
    VerifierConfig {
        recursive_steps: 1,
        initial_dim: 8,
        log_dims: vec![6],
        initial_k: 4,
        ks: vec![2],
    }
}

/// Create configuration for 2^16 polynomial (still fast)
pub fn hardcoded_config_16<T, U>(
    _t: PhantomData<T>,
    _u: PhantomData<U>,
) -> ProverConfig<T, U>
where
    T: BinaryFieldElement,
    U: BinaryFieldElement,
{
    let recursive_steps = 1;
    let inv_rate = 4;

    let initial_dims = (1 << 12, 1 << 4);  // (4096, 16)
    let dims = vec![(1 << 8, 1 << 4)];     // (256, 16)

    let initial_k = 4;
    let ks = vec![4];

    let initial_reed_solomon = reed_solomon::<T>(initial_dims.0, initial_dims.0 * inv_rate);
    let reed_solomon_codes = vec![
        reed_solomon::<U>(dims[0].0, dims[0].0 * inv_rate),
    ];

    ProverConfig {
        recursive_steps,
        initial_dims,
        dims,
        initial_k,
        ks,
        initial_reed_solomon,
        reed_solomon_codes,
    }
}

pub fn hardcoded_config_16_verifier() -> VerifierConfig {
    VerifierConfig {
        recursive_steps: 1,
        initial_dim: 12,
        log_dims: vec![8],
        initial_k: 4,
        ks: vec![4],
    }
}

// Keep existing configurations below...

/// Create configuration for 2^20 polynomial
pub fn hardcoded_config_20<T, U>(
    _t: PhantomData<T>,
    _u: PhantomData<U>,
) -> ProverConfig<T, U>
where
    T: BinaryFieldElement,
    U: BinaryFieldElement,
{
    let recursive_steps = 1;
    let inv_rate = 4;

    let initial_dims = (1 << 14, 1 << 6);  // (2^14, 2^6)
    let dims = vec![(1 << 10, 1 << 4)];    // (2^10, 2^4)

    let initial_k = 6;
    let ks = vec![4];

    let initial_reed_solomon = reed_solomon::<T>(initial_dims.0, initial_dims.0 * inv_rate);
    let reed_solomon_codes = vec![
        reed_solomon::<U>(dims[0].0, dims[0].0 * inv_rate),
    ];

    ProverConfig {
        recursive_steps,
        initial_dims,
        dims,
        initial_k,
        ks,
        initial_reed_solomon,
        reed_solomon_codes,
    }
}

pub fn hardcoded_config_20_verifier() -> VerifierConfig {
    VerifierConfig {
        recursive_steps: 1,
        initial_dim: 14,
        log_dims: vec![10],
        initial_k: 6,
        ks: vec![4],
    }
}

/// Create configuration for 2^24 polynomial
pub fn hardcoded_config_24<T, U>(
    _t: PhantomData<T>,
    _u: PhantomData<U>,
) -> ProverConfig<T, U>
where
    T: BinaryFieldElement,
    U: BinaryFieldElement,
{
    let recursive_steps = 2;
    let inv_rate = 4;

    let initial_dims = (1 << 18, 1 << 6);  // (2^18, 2^6)
    let dims = vec![
        (1 << 14, 1 << 4),  // (2^14, 2^4)
        (1 << 10, 1 << 4),  // (2^10, 2^4)
    ];

    let initial_k = 6;
    let ks = vec![4, 4];

    let initial_reed_solomon = reed_solomon::<T>(initial_dims.0, initial_dims.0 * inv_rate);
    let reed_solomon_codes = dims.iter()
        .map(|&(m, _)| reed_solomon::<U>(m, m * inv_rate))
        .collect();

    ProverConfig {
        recursive_steps,
        initial_dims,
        dims,
        initial_k,
        ks,
        initial_reed_solomon,
        reed_solomon_codes,
    }
}

pub fn hardcoded_config_24_verifier() -> VerifierConfig {
    VerifierConfig {
        recursive_steps: 2,
        initial_dim: 18,
        log_dims: vec![14, 10],
        initial_k: 6,
        ks: vec![4, 4],
    }
}

/// Create configuration for 2^28 polynomial
pub fn hardcoded_config_28<T, U>(
    _t: PhantomData<T>,
    _u: PhantomData<U>,
) -> ProverConfig<T, U>
where
    T: BinaryFieldElement,
    U: BinaryFieldElement,
{
    let recursive_steps = 4;
    let inv_rate = 4;

    let initial_dims = (1 << 22, 1 << 6);  // (2^22, 2^6)
    let dims = vec![
        (1 << 19, 1 << 3),  // (2^19, 2^3)
        (1 << 16, 1 << 3),  // (2^16, 2^3)
        (1 << 13, 1 << 3),  // (2^13, 2^3)
        (1 << 10, 1 << 3),  // (2^10, 2^3)
    ];

    let initial_k = 6;
    let ks = vec![3, 3, 3, 3];

    let initial_reed_solomon = reed_solomon::<T>(initial_dims.0, initial_dims.0 * inv_rate);
    let reed_solomon_codes = dims.iter()
        .map(|&(m, _)| reed_solomon::<U>(m, m * inv_rate))
        .collect();

    ProverConfig {
        recursive_steps,
        initial_dims,
        dims,
        initial_k,
        ks,
        initial_reed_solomon,
        reed_solomon_codes,
    }
}

pub fn hardcoded_config_28_verifier() -> VerifierConfig {
    VerifierConfig {
        recursive_steps: 4,
        initial_dim: 22,
        log_dims: vec![19, 16, 13, 10],
        initial_k: 6,
        ks: vec![3, 3, 3, 3],
    }
}

/// Create configuration for 2^30 polynomial
pub fn hardcoded_config_30<T, U>(
    _t: PhantomData<T>,
    _u: PhantomData<U>,
) -> ProverConfig<T, U>
where
    T: BinaryFieldElement,
    U: BinaryFieldElement,
{
    let recursive_steps = 3;
    let inv_rate = 4;

    let initial_dims = (1 << 23, 1 << 7);  // (2^23, 2^7)
    let dims = vec![
        (1 << 19, 1 << 4),  // (2^19, 2^4)
        (1 << 15, 1 << 4),  // (2^15, 2^4)
        (1 << 11, 1 << 4),  // (2^11, 2^4)
    ];

    let initial_k = 7;
    let ks = vec![4, 4, 4];

    let initial_reed_solomon = reed_solomon::<T>(initial_dims.0, initial_dims.0 * inv_rate);
    let reed_solomon_codes = dims.iter()
        .map(|&(m, _)| reed_solomon::<U>(m, m * inv_rate))
        .collect();

    ProverConfig {
        recursive_steps,
        initial_dims,
        dims,
        initial_k,
        ks,
        initial_reed_solomon,
        reed_solomon_codes,
    }
}

pub fn hardcoded_config_30_verifier() -> VerifierConfig {
    VerifierConfig {
        recursive_steps: 3,
        initial_dim: 23,
        log_dims: vec![19, 15, 11],
        initial_k: 7,
        ks: vec![4, 4, 4],
    }
}
