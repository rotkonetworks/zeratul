//! error types for ghettobox

use thiserror::Error;

pub type Result<T> = std::result::Result<T, Error>;

#[derive(Debug, Error)]
pub enum Error {
    #[error("user not registered")]
    NotRegistered,

    #[error("invalid pin")]
    InvalidPin,

    #[error("rate limited: {attempts} failed attempts, locked for {lockout_seconds}s")]
    RateLimited {
        attempts: u32,
        lockout_seconds: u64,
    },

    #[error("no guesses remaining, secret destroyed")]
    NoGuessesRemaining,

    #[error("seal operation failed: {0}")]
    SealFailed(String),

    #[error("unseal operation failed: {0}")]
    UnsealFailed(String),

    #[error("share verification failed")]
    ShareVerificationFailed,

    #[error("encryption failed: {0}")]
    EncryptionFailed(String),

    #[error("decryption failed: {0}")]
    DecryptionFailed(String),

    #[error("invalid share format")]
    InvalidShareFormat,

    #[error("kdf failed: {0}")]
    KdfFailed(String),

    #[error("storage error: {0}")]
    Storage(String),

    #[error("tpm error: {0}")]
    #[cfg(feature = "tpm")]
    Tpm(String),

    // === vss errors ===
    #[error("invalid secret length (must be 32 bytes)")]
    InvalidSecretLength,

    #[error("vss operation failed: {0}")]
    VssFailed(String),

    #[error("not enough shares: have {have}, need {need}")]
    NotEnoughShares { have: usize, need: usize },

    // === network errors ===
    #[error("network error: {0}")]
    NetworkError(String),

    #[error("not enough nodes: have {have}, need {need}")]
    NotEnoughNodes { have: usize, need: usize },

    #[error("registration failed: {0}")]
    RegistrationFailed(String),

    #[error("recovery failed: {0}")]
    RecoveryFailed(String),

    // === account errors ===
    #[error("key derivation failed")]
    KeyDerivationFailed,
}
