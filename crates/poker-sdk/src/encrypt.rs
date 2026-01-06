//! operator encryption using x25519 + chacha20poly1305
//!
//! encrypts shares for each operator using their registered x25519 pubkey
//! compatible with ghettobox-primitives::EncryptedShare

use chacha20poly1305::{
    aead::{Aead, KeyInit},
    ChaCha20Poly1305, Nonce,
};
use x25519_dalek::{EphemeralSecret, PublicKey, SharedSecret};
use rand::RngCore;

/// encrypted share for a single operator
#[derive(Clone, Debug)]
pub struct EncryptedShare {
    /// target operator id
    pub operator_id: u64,
    /// x25519 ephemeral pubkey (32 bytes)
    pub ephemeral_pubkey: [u8; 32],
    /// chacha20-poly1305 ciphertext (share + 16 byte auth tag)
    pub ciphertext: Vec<u8>,
    /// nonce (12 bytes)
    pub nonce: [u8; 12],
}

impl EncryptedShare {
    /// encrypt a share for an operator
    pub fn encrypt(
        operator_id: u64,
        operator_pubkey: &[u8; 32],
        share_data: &[u8],
    ) -> Result<Self, EncryptError> {
        let mut rng = rand::thread_rng();

        // generate ephemeral keypair
        let ephemeral_secret = EphemeralSecret::random_from_rng(&mut rng);
        let ephemeral_public = PublicKey::from(&ephemeral_secret);

        // derive shared secret via x25519
        let operator_pk = PublicKey::from(*operator_pubkey);
        let shared_secret: SharedSecret = ephemeral_secret.diffie_hellman(&operator_pk);

        // derive encryption key from shared secret
        let encryption_key = derive_encryption_key(shared_secret.as_bytes(), ephemeral_public.as_bytes());

        // generate random nonce
        let mut nonce = [0u8; 12];
        rng.fill_bytes(&mut nonce);

        // encrypt with chacha20-poly1305
        let cipher = ChaCha20Poly1305::new_from_slice(&encryption_key)
            .map_err(|_| EncryptError::InvalidKey)?;

        let ciphertext = cipher
            .encrypt(Nonce::from_slice(&nonce), share_data)
            .map_err(|_| EncryptError::EncryptionFailed)?;

        Ok(Self {
            operator_id,
            ephemeral_pubkey: *ephemeral_public.as_bytes(),
            ciphertext,
            nonce,
        })
    }

    /// decrypt a share (operator side)
    pub fn decrypt(&self, operator_secret: &[u8; 32]) -> Result<Vec<u8>, EncryptError> {
        // compute shared secret
        let ephemeral_pk = PublicKey::from(self.ephemeral_pubkey);

        // x25519 requires StaticSecret for decryption
        let secret = x25519_dalek::StaticSecret::from(*operator_secret);
        let shared_secret = secret.diffie_hellman(&ephemeral_pk);

        // derive encryption key
        let encryption_key = derive_encryption_key(shared_secret.as_bytes(), &self.ephemeral_pubkey);

        // decrypt
        let cipher = ChaCha20Poly1305::new_from_slice(&encryption_key)
            .map_err(|_| EncryptError::InvalidKey)?;

        cipher
            .decrypt(Nonce::from_slice(&self.nonce), self.ciphertext.as_slice())
            .map_err(|_| EncryptError::DecryptionFailed)
    }
}

/// derive symmetric key from shared secret and ephemeral pubkey
fn derive_encryption_key(shared_secret: &[u8], epk: &[u8]) -> [u8; 32] {
    let mut hasher = blake3::Hasher::new();
    hasher.update(b"poker.share.encryption.v1");
    hasher.update(shared_secret);
    hasher.update(epk);
    *hasher.finalize().as_bytes()
}

/// encrypt shares for multiple operators
pub fn encrypt_shares_for_operators(
    share_data: &[u8],
    operators: &[(u64, [u8; 32])], // (operator_id, pubkey)
) -> Result<Vec<EncryptedShare>, EncryptError> {
    operators
        .iter()
        .map(|(id, pubkey)| EncryptedShare::encrypt(*id, pubkey, share_data))
        .collect()
}

/// encryption errors
#[derive(Clone, Debug, thiserror::Error)]
pub enum EncryptError {
    #[error("invalid encryption key")]
    InvalidKey,
    #[error("encryption failed")]
    EncryptionFailed,
    #[error("decryption failed - invalid ciphertext or wrong key")]
    DecryptionFailed,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_encrypt_decrypt_share() {
        // generate operator keypair
        let mut rng = rand::thread_rng();
        let operator_secret = x25519_dalek::StaticSecret::random_from_rng(&mut rng);
        let operator_public = x25519_dalek::PublicKey::from(&operator_secret);

        let share_data = b"secret share data for operator";

        // encrypt
        let encrypted = EncryptedShare::encrypt(
            1, // operator_id
            operator_public.as_bytes(),
            share_data,
        ).unwrap();

        // decrypt
        let decrypted = encrypted.decrypt(operator_secret.as_bytes()).unwrap();

        assert_eq!(decrypted, share_data);
    }

    #[test]
    fn test_wrong_key_fails() {
        let mut rng = rand::thread_rng();
        let operator_secret = x25519_dalek::StaticSecret::random_from_rng(&mut rng);
        let operator_public = x25519_dalek::PublicKey::from(&operator_secret);

        let wrong_secret = x25519_dalek::StaticSecret::random_from_rng(&mut rng);

        let share_data = b"secret";

        let encrypted = EncryptedShare::encrypt(
            1,
            operator_public.as_bytes(),
            share_data,
        ).unwrap();

        // decrypt with wrong key should fail
        assert!(encrypted.decrypt(wrong_secret.as_bytes()).is_err());
    }

    #[test]
    fn test_encrypt_for_multiple_operators() {
        let mut rng = rand::thread_rng();

        let secrets: Vec<_> = (0..3)
            .map(|_| x25519_dalek::StaticSecret::random_from_rng(&mut rng))
            .collect();

        let operators: Vec<_> = secrets
            .iter()
            .enumerate()
            .map(|(i, s)| {
                let pk = x25519_dalek::PublicKey::from(s);
                (i as u64, *pk.as_bytes())
            })
            .collect();

        let share_data = b"shared secret";

        let encrypted = encrypt_shares_for_operators(share_data, &operators).unwrap();

        assert_eq!(encrypted.len(), 3);

        // each operator can decrypt
        for (i, es) in encrypted.iter().enumerate() {
            let decrypted = es.decrypt(secrets[i].as_bytes()).unwrap();
            assert_eq!(decrypted, share_data);
        }
    }
}
