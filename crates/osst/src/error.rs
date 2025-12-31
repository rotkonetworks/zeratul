//! OSST error types

use core::fmt;

/// Errors that can occur during OSST operations
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OsstError {
    /// No contributions provided
    EmptyContributions,

    /// Not enough contributions for threshold
    InsufficientContributions { got: usize, need: usize },

    /// Duplicate custodian index
    DuplicateIndex(u32),

    /// Challenge hash resulted in zero (astronomically unlikely)
    ZeroChallenge,

    /// Invalid commitment point (not on curve)
    InvalidCommitment,

    /// Invalid response scalar (not canonical)
    InvalidResponse,

    /// Lagrange computation failed (duplicate indices)
    LagrangeError,

    /// Index out of valid range (must be > 0)
    InvalidIndex,
}

impl fmt::Display for OsstError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::EmptyContributions => write!(f, "no contributions provided"),
            Self::InsufficientContributions { got, need } => {
                write!(f, "insufficient contributions: got {}, need {}", got, need)
            }
            Self::DuplicateIndex(idx) => write!(f, "duplicate custodian index: {}", idx),
            Self::ZeroChallenge => write!(f, "challenge hash is zero"),
            Self::InvalidCommitment => write!(f, "invalid commitment point"),
            Self::InvalidResponse => write!(f, "invalid response scalar"),
            Self::LagrangeError => write!(f, "lagrange coefficient computation failed"),
            Self::InvalidIndex => write!(f, "index must be greater than 0"),
        }
    }
}

#[cfg(feature = "std")]
impl std::error::Error for OsstError {}
