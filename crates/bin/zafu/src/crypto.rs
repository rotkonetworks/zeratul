//! cryptographic utilities for wallet encryption
//! based on terminator's proven patterns: Argon2id + ChaCha20Poly1305

use anyhow::{anyhow, Result};
use argon2::{Argon2, Algorithm, Version, Params};
use chacha20poly1305::{ChaCha20Poly1305, KeyInit, AeadCore};
use chacha20poly1305::aead::{Aead, OsRng};
use rand_core::RngCore;
use zeroize::{Zeroize, ZeroizeOnDrop};

const SALT_LEN: usize = 32;
const NONCE_LEN: usize = 12;
const KEY_LEN: usize = 32;

/// derive encryption key from password using Argon2id
fn derive_key(salt: &[u8; SALT_LEN], password: &str) -> [u8; KEY_LEN] {
    let params = Params::new(
        2 * 1024,  // 2MB memory
        1,         // 1 iteration
        4,         // 4 parallelism
        Some(KEY_LEN),
    ).expect("valid argon2 params");

    let argon2 = Argon2::new(Algorithm::Argon2id, Version::V0x13, params);

    let mut key = [0u8; KEY_LEN];
    argon2
        .hash_password_into(password.as_bytes(), salt, &mut key)
        .expect("key derivation failed");
    key
}

/// encrypt data with password
/// format: [SALT (32)][NONCE (12)][CIPHERTEXT + TAG]
pub fn encrypt(password: &str, data: &[u8]) -> Result<Vec<u8>> {
    // generate random salt
    let mut salt = [0u8; SALT_LEN];
    OsRng.fill_bytes(&mut salt);

    // derive key
    let key = derive_key(&salt, password);

    // generate random nonce
    let nonce = ChaCha20Poly1305::generate_nonce(&mut OsRng);

    // encrypt
    let cipher = ChaCha20Poly1305::new(&key.into());
    let ciphertext = cipher
        .encrypt(&nonce, data)
        .map_err(|e| anyhow!("encryption failed: {}", e))?;

    // combine: salt + nonce + ciphertext
    let mut result = Vec::with_capacity(SALT_LEN + NONCE_LEN + ciphertext.len());
    result.extend_from_slice(&salt);
    result.extend_from_slice(&nonce);
    result.extend_from_slice(&ciphertext);

    Ok(result)
}

/// decrypt data with password
pub fn decrypt(password: &str, encrypted: &[u8]) -> Result<Vec<u8>> {
    if encrypted.len() < SALT_LEN + NONCE_LEN + 16 {
        return Err(anyhow!("encrypted data too short"));
    }

    // extract salt, nonce, ciphertext
    let salt: [u8; SALT_LEN] = encrypted[..SALT_LEN].try_into()?;
    let nonce: [u8; NONCE_LEN] = encrypted[SALT_LEN..SALT_LEN + NONCE_LEN].try_into()?;
    let ciphertext = &encrypted[SALT_LEN + NONCE_LEN..];

    // derive key
    let key = derive_key(&salt, password);

    // decrypt
    let cipher = ChaCha20Poly1305::new(&key.into());
    let plaintext = cipher
        .decrypt(&nonce.into(), ciphertext)
        .map_err(|_| anyhow!("decryption failed - wrong password?"))?;

    Ok(plaintext)
}

/// session key - keeps derived key in memory, zeroized on drop
/// allows re-encryption without re-entering password
#[derive(Zeroize, ZeroizeOnDrop)]
pub struct SessionKey {
    salt: [u8; SALT_LEN],
    key: [u8; KEY_LEN],
}

impl SessionKey {
    /// create new session key from password
    pub fn from_password(password: &str) -> Self {
        let mut salt = [0u8; SALT_LEN];
        OsRng.fill_bytes(&mut salt);
        let key = derive_key(&salt, password);
        Self { salt, key }
    }

    /// create session key from existing encrypted data
    pub fn from_encrypted(password: &str, encrypted: &[u8]) -> Result<Self> {
        if encrypted.len() < SALT_LEN {
            return Err(anyhow!("encrypted data too short"));
        }
        let salt: [u8; SALT_LEN] = encrypted[..SALT_LEN].try_into()?;
        let key = derive_key(&salt, password);
        Ok(Self { salt, key })
    }

    /// encrypt data using session key
    pub fn encrypt(&self, data: &[u8]) -> Result<Vec<u8>> {
        let nonce = ChaCha20Poly1305::generate_nonce(&mut OsRng);
        let cipher = ChaCha20Poly1305::new(&self.key.into());
        let ciphertext = cipher
            .encrypt(&nonce, data)
            .map_err(|e| anyhow!("encryption failed: {}", e))?;

        let mut result = Vec::with_capacity(SALT_LEN + NONCE_LEN + ciphertext.len());
        result.extend_from_slice(&self.salt);
        result.extend_from_slice(&nonce);
        result.extend_from_slice(&ciphertext);
        Ok(result)
    }

    /// decrypt data using session key
    pub fn decrypt(&self, encrypted: &[u8]) -> Result<Vec<u8>> {
        if encrypted.len() < SALT_LEN + NONCE_LEN + 16 {
            return Err(anyhow!("encrypted data too short"));
        }

        let nonce: [u8; NONCE_LEN] = encrypted[SALT_LEN..SALT_LEN + NONCE_LEN].try_into()?;
        let ciphertext = &encrypted[SALT_LEN + NONCE_LEN..];

        let cipher = ChaCha20Poly1305::new(&self.key.into());
        cipher
            .decrypt(&nonce.into(), ciphertext)
            .map_err(|_| anyhow!("decryption failed"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_encrypt_decrypt() {
        let password = "test_password_123";
        let data = b"sensitive wallet data";

        let encrypted = encrypt(password, data).unwrap();
        let decrypted = decrypt(password, &encrypted).unwrap();

        assert_eq!(data.as_slice(), decrypted.as_slice());
    }

    #[test]
    fn test_wrong_password() {
        let password = "correct_password";
        let wrong = "wrong_password";
        let data = b"sensitive data";

        let encrypted = encrypt(password, data).unwrap();
        let result = decrypt(wrong, &encrypted);

        assert!(result.is_err());
    }

    #[test]
    fn test_session_key() {
        let password = "session_test";
        let data = b"wallet state";

        let session = SessionKey::from_password(password);
        let encrypted = session.encrypt(data).unwrap();
        let decrypted = session.decrypt(&encrypted).unwrap();

        assert_eq!(data.as_slice(), decrypted.as_slice());
    }
}
