use thiserror::Error;

#[derive(Error, Debug)]
pub enum ZidecarError {
    #[error("zebrad RPC error: {0}")]
    ZebradRpc(String),

    #[error("storage error: {0}")]
    Storage(String),

    #[error("proof generation failed: {0}")]
    ProofGeneration(String),

    #[error("invalid block range: {0}")]
    InvalidRange(String),

    #[error("block not found: {0}")]
    BlockNotFound(u32),

    #[error("serialization error: {0}")]
    Serialization(String),

    #[error("validation error: {0}")]
    Validation(String),

    #[error("network error: {0}")]
    Network(String),

    #[error("zync core error: {0}")]
    ZyncCore(#[from] zync_core::ZyncError),
}

pub type Result<T> = std::result::Result<T, ZidecarError>;

impl From<ZidecarError> for tonic::Status {
    fn from(err: ZidecarError) -> Self {
        match err {
            ZidecarError::BlockNotFound(h) => {
                tonic::Status::not_found(format!("block not found: {}", h))
            }
            ZidecarError::InvalidRange(msg) => {
                tonic::Status::invalid_argument(msg)
            }
            _ => tonic::Status::internal(err.to_string()),
        }
    }
}
