//! error types for zync

use thiserror::Error;

#[derive(Debug, Error)]
pub enum ZyncError {
    #[error("invalid proof: {0}")]
    InvalidProof(String),

    #[error("verification failed: {0}")]
    VerificationFailed(String),

    #[error("state mismatch: {0}")]
    StateMismatch(String),

    #[error("chain linkage error: {0}")]
    ChainLinkage(String),

    #[error("wrong viewing key")]
    WrongViewingKey,

    #[error("decryption failed: {0}")]
    DecryptionFailed(String),

    #[error("invalid data: {0}")]
    InvalidData(String),

    #[error("ligerito error: {0}")]
    Ligerito(String),

    #[error("serialization error: {0}")]
    Serialization(String),

    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
}

impl From<ligerito::LigeritoError> for ZyncError {
    fn from(e: ligerito::LigeritoError) -> Self {
        ZyncError::Ligerito(e.to_string())
    }
}

pub type Result<T> = std::result::Result<T, ZyncError>;
