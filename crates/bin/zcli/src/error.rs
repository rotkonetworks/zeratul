use thiserror::Error;

#[derive(Debug, Error)]
pub enum Error {
    #[error("key error: {0}")]
    Key(String),

    #[error("address derivation: {0}")]
    Address(String),

    #[error("network error: {0}")]
    Network(String),

    #[error("wallet error: {0}")]
    Wallet(String),

    #[error("transaction error: {0}")]
    Transaction(String),

    #[error("insufficient funds: have {have} zat, need {need} zat")]
    InsufficientFunds { have: u64, need: u64 },

    #[error("io: {0}")]
    Io(#[from] std::io::Error),

    #[error("{0}")]
    Other(String),
}

impl Error {
    pub fn exit_code(&self) -> i32 {
        match self {
            Self::InsufficientFunds { .. } => 2,
            _ => 1,
        }
    }
}
