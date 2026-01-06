//! realm trait and implementations
//!
//! a realm is a pluggable backend that stores sealed shares.
//! the realm is responsible for:
//! - sealing/unsealing shares (ideally with hardware like tpm)
//! - rate limiting recovery attempts
//! - storing sealed data persistently
//!
//! implementations:
//! - software: in-memory for testing, no hardware security
//! - tpm: linux tpm 2.0 backed, hardware rate limiting

#[cfg(feature = "software")]
pub mod software;

#[cfg(feature = "tpm")]
pub mod tpm;

use crate::Result;
use serde::{Deserialize, Serialize};

/// registration state stored by realm
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Registration {
    /// random version/salt for this registration
    pub version: [u8; 16],
    /// sealed realm share (sealed by tpm or encrypted)
    pub sealed_share: Vec<u8>,
    /// commitment to verify correct pin
    pub unlock_key_commitment: [u8; 32],
    /// tag to prove knowledge of pin
    pub unlock_key_tag: [u8; 16],
    /// encrypted user secret
    pub encrypted_secret: Vec<u8>,
    /// commitment to verify secret integrity
    pub secret_commitment: [u8; 16],
    /// max allowed failed attempts
    pub allowed_guesses: u32,
    /// current failed attempts
    pub attempted_guesses: u32,
}

/// status of a user's registration
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RegistrationStatus {
    /// not registered
    NotRegistered,
    /// registered and can recover
    Registered { version: [u8; 16] },
    /// too many failed attempts, secret destroyed
    NoGuessesRemaining,
}

/// realm trait - pluggable backend for storing sealed shares
///
/// implementors must provide:
/// - seal: protect data (ideally with hardware tpm)
/// - unseal: recover data (with rate limiting)
/// - store/load: persistent storage
pub trait Realm: Send + Sync {
    /// unique identifier for this realm
    fn id(&self) -> &[u8];

    /// seal data - protect it from extraction
    /// for tpm: sealed to pcr state
    /// for software: just encrypt with realm key
    fn seal(&self, data: &[u8]) -> Result<Vec<u8>>;

    /// unseal data - recover protected data
    /// for tpm: enforces hardware rate limiting
    /// for software: just decrypt
    fn unseal(&self, sealed: &[u8]) -> Result<Vec<u8>>;

    /// store a registration for a user
    fn store(&self, user_id: &str, registration: &Registration) -> Result<()>;

    /// load a registration for a user
    fn load(&self, user_id: &str) -> Result<Option<Registration>>;

    /// delete a registration
    fn delete(&self, user_id: &str) -> Result<()>;

    /// increment failed attempts, return new count
    /// returns error if rate limited
    fn increment_attempts(&self, user_id: &str) -> Result<u32>;

    /// reset attempts to zero (on successful recovery)
    fn reset_attempts(&self, user_id: &str) -> Result<()>;

    /// check if user is rate limited
    fn check_rate_limit(&self, user_id: &str) -> Result<()>;
}

/// recover phase 1 response
#[derive(Debug, Clone)]
pub struct RecoverPhase1 {
    pub version: [u8; 16],
    pub allowed_guesses: u32,
    pub attempted_guesses: u32,
}

/// recover phase 2 response
#[derive(Debug, Clone)]
pub struct RecoverPhase2 {
    pub realm_share: Vec<u8>,
    pub unlock_key_commitment: [u8; 32],
}

/// recover phase 3 response
#[derive(Debug, Clone)]
pub struct RecoverPhase3 {
    pub encrypted_secret: Vec<u8>,
    pub secret_commitment: [u8; 16],
}
