//! software realm - in-memory implementation for testing
//!
//! no hardware security, just encryption with a realm key.
//! useful for development and testing, NOT for production.

use std::collections::HashMap;
use std::sync::{Arc, RwLock};
use std::time::{Duration, Instant};

use crate::crypto::{decrypt, encrypt, random_bytes};
use crate::realm::{Realm, Registration};
use crate::{Error, Result};

/// rate limit config
const MAX_ATTEMPTS: u32 = 5;
const LOCKOUT_DURATION: Duration = Duration::from_secs(60 * 60); // 1 hour

/// rate limit state
struct RateLimitState {
    attempts: u32,
    last_attempt: Instant,
    locked_until: Option<Instant>,
}

/// software realm for testing
/// NOT SECURE - use tpm realm for production
pub struct SoftwareRealm {
    id: [u8; 16],
    seal_key: [u8; 32],
    storage: Arc<RwLock<HashMap<String, Registration>>>,
    rate_limits: Arc<RwLock<HashMap<String, RateLimitState>>>,
}

impl SoftwareRealm {
    /// create a new software realm with random keys
    pub fn new() -> Self {
        Self {
            id: random_bytes(),
            seal_key: random_bytes(),
            storage: Arc::new(RwLock::new(HashMap::new())),
            rate_limits: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    /// create with specific id (for testing)
    pub fn with_id(id: [u8; 16]) -> Self {
        Self {
            id,
            seal_key: random_bytes(),
            storage: Arc::new(RwLock::new(HashMap::new())),
            rate_limits: Arc::new(RwLock::new(HashMap::new())),
        }
    }
}

impl Default for SoftwareRealm {
    fn default() -> Self {
        Self::new()
    }
}

impl Realm for SoftwareRealm {
    fn id(&self) -> &[u8] {
        &self.id
    }

    fn seal(&self, data: &[u8]) -> Result<Vec<u8>> {
        let nonce: [u8; 12] = random_bytes();
        let mut sealed = nonce.to_vec();
        sealed.extend(encrypt(&self.seal_key, data, &nonce)?);
        Ok(sealed)
    }

    fn unseal(&self, sealed: &[u8]) -> Result<Vec<u8>> {
        if sealed.len() < 12 {
            return Err(Error::UnsealFailed("sealed data too short".into()));
        }
        let nonce: [u8; 12] = sealed[..12].try_into().unwrap();
        let ciphertext = &sealed[12..];
        decrypt(&self.seal_key, ciphertext, &nonce)
    }

    fn store(&self, user_id: &str, registration: &Registration) -> Result<()> {
        let mut storage = self.storage.write().map_err(|e| Error::Storage(e.to_string()))?;
        storage.insert(user_id.to_string(), registration.clone());
        Ok(())
    }

    fn load(&self, user_id: &str) -> Result<Option<Registration>> {
        let storage = self.storage.read().map_err(|e| Error::Storage(e.to_string()))?;
        Ok(storage.get(user_id).cloned())
    }

    fn delete(&self, user_id: &str) -> Result<()> {
        let mut storage = self.storage.write().map_err(|e| Error::Storage(e.to_string()))?;
        storage.remove(user_id);

        let mut limits = self.rate_limits.write().map_err(|e| Error::Storage(e.to_string()))?;
        limits.remove(user_id);

        Ok(())
    }

    fn check_rate_limit(&self, user_id: &str) -> Result<()> {
        let limits = self.rate_limits.read().map_err(|e| Error::Storage(e.to_string()))?;

        if let Some(state) = limits.get(user_id) {
            if let Some(locked_until) = state.locked_until {
                if Instant::now() < locked_until {
                    let remaining = locked_until.duration_since(Instant::now());
                    return Err(Error::RateLimited {
                        attempts: state.attempts,
                        lockout_seconds: remaining.as_secs(),
                    });
                }
            }
        }

        Ok(())
    }

    fn increment_attempts(&self, user_id: &str) -> Result<u32> {
        self.check_rate_limit(user_id)?;

        let mut storage = self.storage.write().map_err(|e| Error::Storage(e.to_string()))?;

        if let Some(reg) = storage.get_mut(user_id) {
            reg.attempted_guesses += 1;

            if reg.attempted_guesses >= reg.allowed_guesses {
                // destroy the registration
                storage.remove(user_id);
                return Err(Error::NoGuessesRemaining);
            }

            let attempts = reg.attempted_guesses;

            // also track in rate limit state for extra protection
            drop(storage);
            let mut limits = self.rate_limits.write().map_err(|e| Error::Storage(e.to_string()))?;
            let state = limits.entry(user_id.to_string()).or_insert(RateLimitState {
                attempts: 0,
                last_attempt: Instant::now(),
                locked_until: None,
            });
            state.attempts += 1;
            state.last_attempt = Instant::now();
            if state.attempts >= MAX_ATTEMPTS {
                state.locked_until = Some(Instant::now() + LOCKOUT_DURATION);
            }

            Ok(attempts)
        } else {
            Err(Error::NotRegistered)
        }
    }

    fn reset_attempts(&self, user_id: &str) -> Result<()> {
        // reset registration attempts
        let mut storage = self.storage.write().map_err(|e| Error::Storage(e.to_string()))?;
        if let Some(reg) = storage.get_mut(user_id) {
            reg.attempted_guesses = 0;
        }
        drop(storage);

        // reset rate limit state
        let mut limits = self.rate_limits.write().map_err(|e| Error::Storage(e.to_string()))?;
        limits.remove(user_id);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_seal_unseal() {
        let realm = SoftwareRealm::new();
        let data = b"secret data";

        let sealed = realm.seal(data).unwrap();
        let unsealed = realm.unseal(&sealed).unwrap();

        assert_eq!(data.as_slice(), unsealed.as_slice());
    }

    #[test]
    fn test_store_load() {
        let realm = SoftwareRealm::new();
        let reg = Registration {
            version: [1u8; 16],
            sealed_share: vec![1, 2, 3],
            unlock_key_commitment: [2u8; 32],
            unlock_key_tag: [3u8; 16],
            encrypted_secret: vec![4, 5, 6],
            secret_commitment: [5u8; 16],
            allowed_guesses: 5,
            attempted_guesses: 0,
        };

        realm.store("user1", &reg).unwrap();
        let loaded = realm.load("user1").unwrap().unwrap();

        assert_eq!(reg.version, loaded.version);
        assert_eq!(reg.sealed_share, loaded.sealed_share);
    }

    #[test]
    fn test_rate_limiting() {
        let realm = SoftwareRealm::new();

        // need a registration first
        let reg = Registration {
            version: [1u8; 16],
            sealed_share: vec![1, 2, 3],
            unlock_key_commitment: [2u8; 32],
            unlock_key_tag: [3u8; 16],
            encrypted_secret: vec![4, 5, 6],
            secret_commitment: [5u8; 16],
            allowed_guesses: 10, // high limit to test rate limiting separately
            attempted_guesses: 0,
        };
        realm.store("user1", &reg).unwrap();

        // first 4 attempts should succeed
        for i in 1..=4 {
            let count = realm.increment_attempts("user1").unwrap();
            assert_eq!(count, i);
        }

        // 5th attempt should still work (our rate limit is 5 per window)
        let count = realm.increment_attempts("user1").unwrap();
        assert_eq!(count, 5);

        // 6th attempt should be rate limited (by rate limiter, not guess limit)
        let result = realm.increment_attempts("user1");
        assert!(matches!(result, Err(Error::RateLimited { .. })));

        // reset should clear
        realm.reset_attempts("user1").unwrap();
        let count = realm.increment_attempts("user1").unwrap();
        assert_eq!(count, 1);
    }

    #[test]
    fn test_guess_limit_destroys_secret() {
        let realm = SoftwareRealm::new();

        let reg = Registration {
            version: [1u8; 16],
            sealed_share: vec![1, 2, 3],
            unlock_key_commitment: [2u8; 32],
            unlock_key_tag: [3u8; 16],
            encrypted_secret: vec![4, 5, 6],
            secret_commitment: [5u8; 16],
            allowed_guesses: 3,
            attempted_guesses: 0,
        };
        realm.store("user1", &reg).unwrap();

        // 3 wrong attempts
        realm.increment_attempts("user1").unwrap();
        realm.increment_attempts("user1").unwrap();
        let result = realm.increment_attempts("user1");

        // 3rd attempt should destroy secret
        assert!(matches!(result, Err(Error::NoGuessesRemaining)));

        // registration should be gone
        assert!(realm.load("user1").unwrap().is_none());
    }
}
