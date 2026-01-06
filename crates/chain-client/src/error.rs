//! error types for chain client

use thiserror::Error;

#[derive(Error, Debug)]
pub enum ChainError {
    #[error("connection failed: {0}")]
    ConnectionFailed(String),

    #[error("not connected to chain")]
    NotConnected,

    #[error("query failed: {0}")]
    QueryFailed(String),

    #[error("transaction failed: {0}")]
    TransactionFailed(String),

    #[error("signing failed: {0}")]
    SigningFailed(String),

    #[error("encoding error: {0}")]
    EncodingError(String),

    #[error("decoding error: {0}")]
    DecodingError(String),

    #[error("xcm error: {0}")]
    XcmError(String),

    #[error("ibc error: {0}")]
    IbcError(String),

    #[error("timeout waiting for {0}")]
    Timeout(String),

    #[error("invalid address: {0}")]
    InvalidAddress(String),

    #[error("insufficient balance: have {have}, need {need}")]
    InsufficientBalance { have: u128, need: u128 },

    #[error("chain error: {0}")]
    ChainError(String),

    #[error("io error: {0}")]
    IoError(String),
}

pub type Result<T> = std::result::Result<T, ChainError>;

impl From<std::io::Error> for ChainError {
    fn from(e: std::io::Error) -> Self {
        ChainError::IoError(e.to_string())
    }
}
