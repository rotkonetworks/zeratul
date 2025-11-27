//! Automatic configuration selection for polynomial sizes 2^20 to 2^30
//!
//! Provides optimal Ligerito configurations for any polynomial size in the supported range.
//! The autosizer selects the smallest configuration that fits the input polynomial,
//! ensuring efficient proof generation while minimizing proof size.
//!
//! # Example
//!
//! ```rust,ignore
//! use ligerito::autosizer::{prover_config_for_size, verifier_config_for_size};
//! use ligerito_binary_fields::{BinaryElem32, BinaryElem128};
//!
//! // For a polynomial of 500,000 elements
//! let poly_len = 500_000;
//! let (prover_config, padded_size) = prover_config_for_size::<BinaryElem32, BinaryElem128>(poly_len);
//! // padded_size will be 2^20 = 1,048,576
//!
//! let verifier_config = verifier_config_for_size(poly_len);
//! ```

use crate::data_structures::VerifierConfig;

#[cfg(feature = "prover")]
use crate::data_structures::ProverConfig;

#[cfg(feature = "prover")]
use binary_fields::BinaryFieldElement;

#[cfg(feature = "prover")]
use reed_solomon::reed_solomon;

/// Supported polynomial sizes (log2)
pub const MIN_LOG_SIZE: u32 = 20;
pub const MAX_LOG_SIZE: u32 = 30;

/// Optimal configuration parameters for each polynomial size
/// Format: (log_size, recursive_steps, initial_dim_log, initial_k, [(dim_log, k), ...])
const OPTIMAL_CONFIGS: [(u32, usize, u32, usize, &[(u32, usize)]); 11] = [
    // 2^20: 1 recursive step, 2^14 × 2^6 initial, then 2^10 × 2^4
    (20, 1, 14, 6, &[(10, 4)]),
    // 2^21: 1 recursive step, 2^15 × 2^6 initial, then 2^11 × 2^4
    (21, 1, 15, 6, &[(11, 4)]),
    // 2^22: 2 recursive steps for better balance
    (22, 2, 16, 6, &[(12, 4), (8, 4)]),
    // 2^23: 2 recursive steps
    (23, 2, 17, 6, &[(13, 4), (9, 4)]),
    // 2^24: 2 recursive steps (existing config)
    (24, 2, 18, 6, &[(14, 4), (10, 4)]),
    // 2^25: 2 recursive steps
    (25, 2, 19, 6, &[(15, 4), (11, 4)]),
    // 2^26: 3 recursive steps for larger polynomials
    (26, 3, 20, 6, &[(16, 4), (12, 4), (8, 4)]),
    // 2^27: 3 recursive steps
    (27, 3, 21, 6, &[(17, 4), (13, 4), (9, 4)]),
    // 2^28: 4 recursive steps (existing config uses k=3, but k=4 is more balanced)
    (28, 3, 22, 6, &[(18, 4), (14, 4), (10, 4)]),
    // 2^29: 3 recursive steps
    (29, 3, 23, 6, &[(19, 4), (15, 4), (11, 4)]),
    // 2^30: 3 recursive steps (existing config)
    (30, 3, 23, 7, &[(19, 4), (15, 4), (11, 4)]),
];

/// Get the log2 of the required padded size for a polynomial
#[inline]
pub fn log_size_for_len(len: usize) -> u32 {
    if len == 0 {
        return MIN_LOG_SIZE;
    }
    let log = (len as f64).log2().ceil() as u32;
    log.clamp(MIN_LOG_SIZE, MAX_LOG_SIZE)
}

/// Select optimal prover configuration for a polynomial of given length
///
/// Returns (config, padded_size) where padded_size is the power-of-2 size
/// the polynomial should be padded to.
#[cfg(feature = "prover")]
pub fn prover_config_for_size<T, U>(len: usize) -> (ProverConfig<T, U>, usize)
where
    T: BinaryFieldElement,
    U: BinaryFieldElement,
{
    let log_size = log_size_for_len(len);
    let config = prover_config_for_log_size::<T, U>(log_size);
    (config, 1 << log_size)
}

/// Select optimal verifier configuration for a polynomial of given length
pub fn verifier_config_for_size(len: usize) -> VerifierConfig {
    let log_size = log_size_for_len(len);
    verifier_config_for_log_size(log_size)
}

/// Get prover configuration for exact log2 size
#[cfg(feature = "prover")]
pub fn prover_config_for_log_size<T, U>(log_size: u32) -> ProverConfig<T, U>
where
    T: BinaryFieldElement,
    U: BinaryFieldElement,
{
    let log_size = log_size.clamp(MIN_LOG_SIZE, MAX_LOG_SIZE);

    // Find the matching config
    let idx = (log_size - MIN_LOG_SIZE) as usize;
    let (_, recursive_steps, initial_dim_log, initial_k, dims_ks) = OPTIMAL_CONFIGS[idx];

    let inv_rate = 4;

    // Calculate initial dimensions: rows × cols where rows * cols = 2^log_size
    // initial_dim_log is the log of rows, so cols = 2^(log_size - initial_dim_log)
    let initial_n_log = initial_dim_log;
    let initial_m_log = log_size - initial_dim_log;
    let initial_dims = (1 << initial_n_log, 1 << initial_m_log);

    // Build recursive dimensions
    let dims: Vec<(usize, usize)> = dims_ks
        .iter()
        .map(|&(dim_log, k)| {
            // dim_log is total log size for this round
            // k determines the column dimension: cols = 2^k
            // rows = 2^(dim_log - k) but we need rows >= cols for RS
            let n_log = dim_log - k as u32;
            (1 << n_log, 1 << k)
        })
        .collect();

    let ks: Vec<usize> = dims_ks.iter().map(|&(_, k)| k).collect();

    // Build Reed-Solomon codes
    let initial_reed_solomon = reed_solomon::<T>(initial_dims.0, initial_dims.0 * inv_rate);
    let reed_solomon_codes = dims
        .iter()
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
        num_queries: 148, // 100-bit security
    }
}

/// Get verifier configuration for exact log2 size
pub fn verifier_config_for_log_size(log_size: u32) -> VerifierConfig {
    let log_size = log_size.clamp(MIN_LOG_SIZE, MAX_LOG_SIZE);

    // Find the matching config
    let idx = (log_size - MIN_LOG_SIZE) as usize;
    let (_, recursive_steps, initial_dim_log, initial_k, dims_ks) = OPTIMAL_CONFIGS[idx];

    // Calculate initial_dim (log of row count)
    let initial_dim = initial_dim_log as usize;

    // Build log_dims: each entry is log of row count for that round
    let log_dims: Vec<usize> = dims_ks
        .iter()
        .map(|&(dim_log, k)| (dim_log - k as u32) as usize)
        .collect();

    let ks: Vec<usize> = dims_ks.iter().map(|&(_, k)| k).collect();

    VerifierConfig {
        recursive_steps,
        initial_dim,
        log_dims,
        initial_k,
        ks,
        num_queries: 148,
    }
}

/// Information about a configuration
#[derive(Debug, Clone)]
pub struct ConfigInfo {
    /// Log2 of polynomial size
    pub log_size: u32,
    /// Polynomial size (2^log_size)
    pub poly_size: usize,
    /// Number of recursive compression steps
    pub recursive_steps: usize,
    /// Initial k value (columns = 2^k)
    pub initial_k: usize,
    /// k values for each recursive step
    pub ks: Vec<usize>,
    /// Estimated proof size in bytes (approximate)
    pub estimated_proof_bytes: usize,
}

/// Get information about the configuration for a given polynomial size
pub fn config_info(len: usize) -> ConfigInfo {
    let log_size = log_size_for_len(len);
    config_info_for_log_size(log_size)
}

/// Get information about a specific log2 configuration
pub fn config_info_for_log_size(log_size: u32) -> ConfigInfo {
    let log_size = log_size.clamp(MIN_LOG_SIZE, MAX_LOG_SIZE);
    let idx = (log_size - MIN_LOG_SIZE) as usize;
    let (_, recursive_steps, _, initial_k, dims_ks) = OPTIMAL_CONFIGS[idx];

    let ks: Vec<usize> = dims_ks.iter().map(|&(_, k)| k).collect();

    // Rough estimate of proof size:
    // - 148 queries × (row_size × element_size) per round
    // - Plus Merkle proofs (~32 bytes × depth × queries)
    // - Plus sumcheck transcripts
    let num_queries = 148;
    let base_field_bytes = 4;  // BinaryElem32
    let ext_field_bytes = 16;  // BinaryElem128

    // Initial round: 148 queries × 2^initial_k elements × 4 bytes
    let initial_row_size = 1 << initial_k;
    let initial_data = num_queries * initial_row_size * base_field_bytes;

    // Recursive rounds: 148 queries × 2^k elements × 16 bytes each
    let recursive_data: usize = dims_ks
        .iter()
        .map(|&(_, k)| num_queries * (1 << k) * ext_field_bytes)
        .sum();

    // Merkle proofs: ~32 bytes per hash × depth × queries
    // depth ≈ log_size + 2 (for inv_rate)
    let merkle_overhead = num_queries * 32 * (log_size as usize + 2) * (recursive_steps + 1);

    // Sumcheck transcript: ~48 bytes per round (3 coefficients × 16 bytes)
    let sumcheck_overhead = 48 * (initial_k + ks.iter().sum::<usize>());

    let estimated_proof_bytes = initial_data + recursive_data + merkle_overhead + sumcheck_overhead;

    ConfigInfo {
        log_size,
        poly_size: 1 << log_size,
        recursive_steps,
        initial_k,
        ks,
        estimated_proof_bytes,
    }
}

/// Print a summary of all available configurations
#[cfg(feature = "std")]
pub fn print_config_summary() {
    println!("Ligerito Configuration Summary");
    println!("==============================");
    println!("{:>6} {:>12} {:>8} {:>10} {:>12}",
             "Log", "Poly Size", "Steps", "k values", "Est. Proof");
    println!("{:-<6} {:-<12} {:-<8} {:-<10} {:-<12}", "", "", "", "", "");

    for log_size in MIN_LOG_SIZE..=MAX_LOG_SIZE {
        let info = config_info_for_log_size(log_size);
        let ks_str = core::iter::once(info.initial_k)
            .chain(info.ks.iter().copied())
            .map(|k| k.to_string())
            .collect::<Vec<_>>()
            .join(",");

        println!("{:>6} {:>12} {:>8} {:>10} {:>10} KB",
                 log_size,
                 format_size(info.poly_size),
                 info.recursive_steps,
                 ks_str,
                 info.estimated_proof_bytes / 1024);
    }
}

#[cfg(feature = "std")]
fn format_size(n: usize) -> String {
    if n >= 1_000_000_000 {
        format!("{:.1}G", n as f64 / 1_000_000_000.0)
    } else if n >= 1_000_000 {
        format!("{:.1}M", n as f64 / 1_000_000.0)
    } else if n >= 1_000 {
        format!("{:.1}K", n as f64 / 1_000.0)
    } else {
        format!("{}", n)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_log_size_for_len() {
        assert_eq!(log_size_for_len(0), 20);
        assert_eq!(log_size_for_len(1), 20);
        assert_eq!(log_size_for_len(1 << 20), 20);
        assert_eq!(log_size_for_len((1 << 20) + 1), 21);
        assert_eq!(log_size_for_len(1 << 24), 24);
        assert_eq!(log_size_for_len(1 << 30), 30);
        // Clamp to max
        assert_eq!(log_size_for_len(usize::MAX), 30);
    }

    #[test]
    fn test_verifier_config_consistency() {
        for log_size in MIN_LOG_SIZE..=MAX_LOG_SIZE {
            let config = verifier_config_for_log_size(log_size);

            // Verify config is self-consistent
            assert_eq!(config.recursive_steps, config.log_dims.len());
            assert_eq!(config.recursive_steps, config.ks.len());
            assert!(config.initial_k > 0);
            assert!(config.initial_dim > 0);

            // Verify dimensions make sense
            let initial_total = config.initial_dim + config.initial_k;
            assert_eq!(initial_total as u32, log_size,
                "initial_dim + initial_k should equal log_size for config {}", log_size);
        }
    }

    #[test]
    #[cfg(feature = "prover")]
    fn test_prover_config_consistency() {
        use ligerito_binary_fields::{BinaryElem32, BinaryElem128};

        // Only test smaller sizes in debug builds (RS code generation is slow)
        #[cfg(debug_assertions)]
        let test_sizes = [20, 21, 22, 24];
        #[cfg(not(debug_assertions))]
        let test_sizes: Vec<u32> = (MIN_LOG_SIZE..=MAX_LOG_SIZE).collect();

        for &log_size in &test_sizes {
            let config = prover_config_for_log_size::<BinaryElem32, BinaryElem128>(log_size);

            // Verify dimensions multiply correctly
            let initial_size = config.initial_dims.0 * config.initial_dims.1;
            assert_eq!(initial_size, 1 << log_size,
                "initial dims should multiply to 2^{}", log_size);

            // Verify recursive steps match
            assert_eq!(config.recursive_steps, config.dims.len());
            assert_eq!(config.recursive_steps, config.ks.len());
            assert_eq!(config.recursive_steps, config.reed_solomon_codes.len());
        }
    }

    #[test]
    fn test_config_info() {
        let info = config_info_for_log_size(24);
        assert_eq!(info.log_size, 24);
        assert_eq!(info.poly_size, 1 << 24);
        assert!(info.estimated_proof_bytes > 0);
        assert!(info.estimated_proof_bytes < 1_000_000); // Should be under 1MB
    }

    #[test]
    #[cfg(feature = "std")]
    fn test_print_summary() {
        // Just ensure it doesn't panic
        print_config_summary();
    }
}
