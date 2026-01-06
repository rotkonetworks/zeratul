//! cryptographic primitives for ghettobox
//!
//! - argon2id for pin stretching
//! - chacha20poly1305 for authenticated encryption
//! - xor for secret sharing

use argon2::{Argon2, Params, Version};
use chacha20poly1305::{
    aead::{Aead, KeyInit as AeadKeyInit},
    ChaCha20Poly1305, Nonce,
};
use hmac::{Hmac, Mac, digest::KeyInit};
use rand::RngCore;
use sha2::Sha256;

type HmacSha256 = Hmac<Sha256>;

use crate::{Error, Result};

/// argon2id parameters - tuned for reasonable security on commodity hardware
/// similar to juicebox recommendations
const ARGON2_M_COST: u32 = 16 * 1024; // 16 MiB
const ARGON2_T_COST: u32 = 32;        // 32 iterations
const ARGON2_P_COST: u32 = 1;         // parallelism 1

/// output length for kdf
const KDF_OUTPUT_LEN: usize = 64;

/// stretch a pin using argon2id
/// returns 64 bytes: 32 for access key, 32 for encryption key seed
pub fn stretch_pin(pin: &[u8], salt: &[u8], user_info: &[u8]) -> Result<[u8; KDF_OUTPUT_LEN]> {
    let params = Params::new(ARGON2_M_COST, ARGON2_T_COST, ARGON2_P_COST, Some(KDF_OUTPUT_LEN))
        .map_err(|e| Error::KdfFailed(e.to_string()))?;

    let argon2 = Argon2::new(argon2::Algorithm::Argon2id, Version::V0x13, params);

    // combine salt with user_info
    let mut full_salt = Vec::with_capacity(salt.len() + user_info.len());
    full_salt.extend_from_slice(salt);
    full_salt.extend_from_slice(user_info);

    let mut output = [0u8; KDF_OUTPUT_LEN];
    argon2
        .hash_password_into(pin, &full_salt, &mut output)
        .map_err(|e| Error::KdfFailed(e.to_string()))?;

    Ok(output)
}

/// split stretched pin into access key and encryption key seed
pub fn split_stretched_pin(stretched: &[u8; KDF_OUTPUT_LEN]) -> ([u8; 32], [u8; 32]) {
    let mut access_key = [0u8; 32];
    let mut encryption_seed = [0u8; 32];
    access_key.copy_from_slice(&stretched[..32]);
    encryption_seed.copy_from_slice(&stretched[32..]);
    (access_key, encryption_seed)
}

/// xor two byte slices of equal length
pub fn xor(a: &[u8], b: &[u8]) -> Vec<u8> {
    assert_eq!(a.len(), b.len(), "xor inputs must be same length");
    a.iter().zip(b.iter()).map(|(x, y)| x ^ y).collect()
}

/// split a secret into two xor shares
/// returns (realm_share, user_share)
pub fn split_secret(secret: &[u8]) -> (Vec<u8>, Vec<u8>) {
    let mut realm_share = vec![0u8; secret.len()];
    rand::thread_rng().fill_bytes(&mut realm_share);
    let user_share = xor(secret, &realm_share);
    (realm_share, user_share)
}

/// combine two xor shares to recover secret
pub fn combine_shares(realm_share: &[u8], user_share: &[u8]) -> Vec<u8> {
    xor(realm_share, user_share)
}

/// generate random bytes
pub fn random_bytes<const N: usize>() -> [u8; N] {
    let mut bytes = [0u8; N];
    rand::thread_rng().fill_bytes(&mut bytes);
    bytes
}

/// encrypt data using chacha20poly1305
pub fn encrypt(key: &[u8; 32], plaintext: &[u8], nonce: &[u8; 12]) -> Result<Vec<u8>> {
    let cipher: ChaCha20Poly1305 = AeadKeyInit::new_from_slice(key)
        .map_err(|e| Error::EncryptionFailed(e.to_string()))?;
    let n = Nonce::from_slice(nonce);
    cipher
        .encrypt(n, plaintext)
        .map_err(|e| Error::EncryptionFailed(e.to_string()))
}

/// decrypt data using chacha20poly1305
pub fn decrypt(key: &[u8; 32], ciphertext: &[u8], nonce: &[u8; 12]) -> Result<Vec<u8>> {
    let cipher: ChaCha20Poly1305 = AeadKeyInit::new_from_slice(key)
        .map_err(|e| Error::DecryptionFailed(e.to_string()))?;
    let n = Nonce::from_slice(nonce);
    cipher
        .decrypt(n, ciphertext)
        .map_err(|e| Error::DecryptionFailed(e.to_string()))
}

/// compute hmac-sha256 tag
pub fn mac(key: &[u8], data: &[&[u8]]) -> [u8; 32] {
    let mut h: HmacSha256 = KeyInit::new_from_slice(key).expect("hmac accepts any key length");
    for d in data {
        Mac::update(&mut h, d);
    }
    h.finalize().into_bytes().into()
}

/// derive unlock key tag for a specific realm
pub fn unlock_key_tag(unlock_key: &[u8], realm_id: &[u8]) -> [u8; 16] {
    let tag = mac(unlock_key, &[b"ghettobox:unlock_key_tag:v1", realm_id]);
    let mut result = [0u8; 16];
    result.copy_from_slice(&tag[..16]);
    result
}

/// derive encryption key from seed and scalar
pub fn derive_encryption_key(seed: &[u8; 32], scalar: &[u8; 32]) -> [u8; 32] {
    mac(seed, &[b"ghettobox:encryption_key:v1", scalar])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_xor_roundtrip() {
        let secret = b"my secret data here";
        let (realm_share, user_share) = split_secret(secret);
        let recovered = combine_shares(&realm_share, &user_share);
        assert_eq!(secret.as_slice(), recovered.as_slice());
    }

    #[test]
    fn test_encrypt_decrypt() {
        let key = random_bytes::<32>();
        let nonce = random_bytes::<12>();
        let plaintext = b"hello world";

        let ciphertext = encrypt(&key, plaintext, &nonce).unwrap();
        let decrypted = decrypt(&key, &ciphertext, &nonce).unwrap();

        assert_eq!(plaintext.as_slice(), decrypted.as_slice());
    }

    #[test]
    fn test_stretch_pin() {
        let pin = b"1234";
        let salt = random_bytes::<16>();
        let user_info = b"user@example.com";

        let stretched = stretch_pin(pin, &salt, user_info).unwrap();
        assert_eq!(stretched.len(), 64);

        // same inputs should produce same output
        let stretched2 = stretch_pin(pin, &salt, user_info).unwrap();
        assert_eq!(stretched, stretched2);

        // different pin should produce different output
        let stretched3 = stretch_pin(b"5678", &salt, user_info).unwrap();
        assert_ne!(stretched, stretched3);
    }
}
