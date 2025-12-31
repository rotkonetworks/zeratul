//! Escrow encryption using X25519 + ChaCha20Poly1305
//!
//! Following Penumbra's encryption patterns:
//! - X25519 for key exchange (ECDH)
//! - Blake2b with personalization for KDF
//! - ChaCha20Poly1305 for AEAD
//! - Zero nonce (safe because each key is unique per message)
//!
//! Ciphertext format: EPK (32) || Tag (16) || Encrypted data
//! The EPK is included as AAD (authenticated associated data).

#[cfg(feature = "std")]
use {
    blake2::{Blake2b, Digest},
    chacha20poly1305::{
        aead::{Aead, KeyInit, Payload},
        ChaCha20Poly1305, Nonce,
    },
    x25519_dalek::{EphemeralSecret, PublicKey, SharedSecret, StaticSecret},
    zeroize::Zeroize,
};

/// Size constants
pub const PUBLIC_KEY_SIZE: usize = 32;
pub const TAG_SIZE: usize = 16;
pub const NONCE_SIZE: usize = 12;
pub const SHARE_SIZE: usize = 32;

/// Minimum ciphertext size: EPK + Tag + at least 1 byte
pub const MIN_CIPHERTEXT_SIZE: usize = PUBLIC_KEY_SIZE + TAG_SIZE + 1;

/// Expected ciphertext size for a 32-byte share
pub const SHARE_CIPHERTEXT_SIZE: usize = PUBLIC_KEY_SIZE + TAG_SIZE + SHARE_SIZE;

/// Personalization string for escrow share encryption KDF
const KDF_PERSONALIZATION: &[u8; 16] = b"Zanchor_EscShare";

/// Error types for encryption operations
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EncryptionError {
    /// Ciphertext too short to contain EPK + tag
    CiphertextTooShort,
    /// AEAD decryption failed (invalid tag or corrupted data)
    DecryptionFailed,
    /// Invalid public key encoding
    InvalidPublicKey,
    /// Plaintext size mismatch
    PlaintextSizeMismatch,
}

impl core::fmt::Display for EncryptionError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::CiphertextTooShort => write!(f, "ciphertext too short"),
            Self::DecryptionFailed => write!(f, "decryption failed"),
            Self::InvalidPublicKey => write!(f, "invalid public key"),
            Self::PlaintextSizeMismatch => write!(f, "plaintext size mismatch"),
        }
    }
}

/// Symmetric key derived from X25519 shared secret
#[cfg(feature = "std")]
#[derive(Clone, Zeroize)]
#[zeroize(drop)]
pub struct SymmetricKey([u8; 32]);

#[cfg(feature = "std")]
impl SymmetricKey {
    /// Derive symmetric key from shared secret and ephemeral public key
    ///
    /// KDF: Blake2b-256(personalization || recipient_pk || epk || shared_secret)
    ///
    /// Following Penumbra's pattern of including both public keys in the KDF
    /// to bind the key to the specific key exchange instance.
    pub fn derive(
        recipient_pk: &PublicKey,
        epk: &PublicKey,
        shared_secret: &SharedSecret,
    ) -> Self {
        use blake2::digest::typenum::U32;

        let mut hasher = Blake2b::<U32>::new();

        // Personalization (like Penumbra's personal() but inline)
        hasher.update(KDF_PERSONALIZATION);

        // Include both public keys to bind the key to this exchange
        hasher.update(recipient_pk.as_bytes());
        hasher.update(epk.as_bytes());

        // The shared secret from X25519
        hasher.update(shared_secret.as_bytes());

        let result = hasher.finalize();
        let mut key = [0u8; 32];
        key.copy_from_slice(&result);

        Self(key)
    }

    /// Get the key bytes for ChaCha20Poly1305
    fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }
}

/// Encrypt a share to a recipient's X25519 public key
///
/// Returns: EPK (32 bytes) || Ciphertext (plaintext.len() + 16 bytes tag)
///
/// The ciphertext format follows Penumbra's DKG encryption pattern:
/// - Generate ephemeral keypair
/// - Perform X25519 key agreement
/// - Derive symmetric key via Blake2b KDF
/// - Encrypt with ChaCha20Poly1305 using zero nonce and EPK as AAD
#[cfg(feature = "std")]
pub fn encrypt_share(
    recipient_pk: &[u8; 32],
    plaintext: &[u8; SHARE_SIZE],
) -> Result<[u8; SHARE_CIPHERTEXT_SIZE], EncryptionError> {
    // Generate ephemeral keypair
    let esk = EphemeralSecret::random_from_rng(rand_core::OsRng);
    let epk = PublicKey::from(&esk);

    // Parse recipient public key
    let recipient = PublicKey::from(*recipient_pk);

    // X25519 key agreement: shared_secret = esk * recipient_pk
    let shared_secret = esk.diffie_hellman(&recipient);

    // Derive symmetric key
    let sym_key = SymmetricKey::derive(&recipient, &epk, &shared_secret);

    // Create cipher
    let cipher = ChaCha20Poly1305::new_from_slice(sym_key.as_bytes())
        .expect("key size is correct");

    // Zero nonce - safe because each EPK produces a unique key
    let nonce = Nonce::default();

    // Encrypt with EPK as AAD (authenticated associated data)
    let ciphertext = cipher
        .encrypt(
            &nonce,
            Payload {
                msg: plaintext,
                aad: epk.as_bytes(),
            },
        )
        .expect("encryption should not fail");

    // Format: EPK || ciphertext (includes tag)
    let mut result = [0u8; SHARE_CIPHERTEXT_SIZE];
    result[..PUBLIC_KEY_SIZE].copy_from_slice(epk.as_bytes());
    result[PUBLIC_KEY_SIZE..].copy_from_slice(&ciphertext);

    Ok(result)
}

/// Decrypt a share using the recipient's X25519 secret key
///
/// Expects format: EPK (32 bytes) || Ciphertext (plaintext + 16 bytes tag)
#[cfg(feature = "std")]
pub fn decrypt_share(
    secret_key: &[u8; 32],
    ciphertext: &[u8],
) -> Result<[u8; SHARE_SIZE], EncryptionError> {
    if ciphertext.len() < MIN_CIPHERTEXT_SIZE {
        return Err(EncryptionError::CiphertextTooShort);
    }

    // Extract EPK from ciphertext
    let epk_bytes: [u8; 32] = ciphertext[..PUBLIC_KEY_SIZE]
        .try_into()
        .map_err(|_| EncryptionError::CiphertextTooShort)?;
    let epk = PublicKey::from(epk_bytes);

    // Parse secret key
    let sk = StaticSecret::from(*secret_key);
    let recipient_pk = PublicKey::from(&sk);

    // X25519 key agreement: shared_secret = sk * epk
    let shared_secret = sk.diffie_hellman(&epk);

    // Derive symmetric key (same as encryption)
    let sym_key = SymmetricKey::derive(&recipient_pk, &epk, &shared_secret);

    // Create cipher
    let cipher = ChaCha20Poly1305::new_from_slice(sym_key.as_bytes())
        .expect("key size is correct");

    // Zero nonce (must match encryption)
    let nonce = Nonce::default();

    // Decrypt with EPK as AAD
    let encrypted_part = &ciphertext[PUBLIC_KEY_SIZE..];
    let plaintext = cipher
        .decrypt(
            &nonce,
            Payload {
                msg: encrypted_part,
                aad: epk.as_bytes(),
            },
        )
        .map_err(|_| EncryptionError::DecryptionFailed)?;

    // Verify size and convert to fixed array
    if plaintext.len() != SHARE_SIZE {
        return Err(EncryptionError::PlaintextSizeMismatch);
    }

    let mut result = [0u8; SHARE_SIZE];
    result.copy_from_slice(&plaintext);
    Ok(result)
}

/// Generate a new X25519 keypair
///
/// Returns (secret_key, public_key)
#[cfg(feature = "std")]
pub fn generate_keypair() -> ([u8; 32], [u8; 32]) {
    let sk = StaticSecret::random_from_rng(rand_core::OsRng);
    let pk = PublicKey::from(&sk);
    (sk.to_bytes(), pk.to_bytes())
}

/// Derive public key from secret key
#[cfg(feature = "std")]
pub fn public_key_from_secret(secret_key: &[u8; 32]) -> [u8; 32] {
    let sk = StaticSecret::from(*secret_key);
    let pk = PublicKey::from(&sk);
    pk.to_bytes()
}

#[cfg(all(test, feature = "std"))]
mod tests {
    use super::*;

    #[test]
    fn test_encrypt_decrypt_roundtrip() {
        // Generate recipient keypair
        let (sk, pk) = generate_keypair();

        // Test share
        let share: [u8; 32] = [
            0x01, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07, 0x08,
            0x09, 0x0a, 0x0b, 0x0c, 0x0d, 0x0e, 0x0f, 0x10,
            0x11, 0x12, 0x13, 0x14, 0x15, 0x16, 0x17, 0x18,
            0x19, 0x1a, 0x1b, 0x1c, 0x1d, 0x1e, 0x1f, 0x20,
        ];

        // Encrypt
        let ciphertext = encrypt_share(&pk, &share).expect("encryption should succeed");

        // Verify ciphertext size
        assert_eq!(ciphertext.len(), SHARE_CIPHERTEXT_SIZE);

        // Decrypt
        let decrypted = decrypt_share(&sk, &ciphertext).expect("decryption should succeed");

        // Verify roundtrip
        assert_eq!(share, decrypted);
    }

    #[test]
    fn test_wrong_key_fails() {
        let (_sk1, pk1) = generate_keypair();
        let (sk2, _pk2) = generate_keypair();

        let share = [0xab; 32];

        // Encrypt to pk1
        let ciphertext = encrypt_share(&pk1, &share).expect("encryption should succeed");

        // Try to decrypt with sk2 - should fail
        let result = decrypt_share(&sk2, &ciphertext);
        assert!(matches!(result, Err(EncryptionError::DecryptionFailed)));
    }

    #[test]
    fn test_tampered_ciphertext_fails() {
        let (sk, pk) = generate_keypair();
        let share = [0xcd; 32];

        let mut ciphertext = encrypt_share(&pk, &share).expect("encryption should succeed");

        // Tamper with the encrypted data (after EPK)
        ciphertext[PUBLIC_KEY_SIZE + 5] ^= 0xff;

        // Decryption should fail due to authentication
        let result = decrypt_share(&sk, &ciphertext);
        assert!(matches!(result, Err(EncryptionError::DecryptionFailed)));
    }

    #[test]
    fn test_tampered_epk_fails() {
        let (sk, pk) = generate_keypair();
        let share = [0xef; 32];

        let mut ciphertext = encrypt_share(&pk, &share).expect("encryption should succeed");

        // Tamper with the EPK
        ciphertext[5] ^= 0xff;

        // Decryption should fail because EPK is authenticated via AAD
        let result = decrypt_share(&sk, &ciphertext);
        assert!(matches!(result, Err(EncryptionError::DecryptionFailed)));
    }

    #[test]
    fn test_ciphertext_too_short() {
        let (sk, _pk) = generate_keypair();

        // Too short to contain EPK + tag
        let short_ciphertext = [0u8; PUBLIC_KEY_SIZE + TAG_SIZE - 1];

        let result = decrypt_share(&sk, &short_ciphertext);
        assert!(matches!(result, Err(EncryptionError::CiphertextTooShort)));
    }

    #[test]
    fn test_public_key_derivation() {
        let (sk, pk) = generate_keypair();
        let derived_pk = public_key_from_secret(&sk);
        assert_eq!(pk, derived_pk);
    }

    #[test]
    fn test_different_encryptions_have_different_epks() {
        let (_sk, pk) = generate_keypair();
        let share = [0x42; 32];

        let ct1 = encrypt_share(&pk, &share).expect("encryption should succeed");
        let ct2 = encrypt_share(&pk, &share).expect("encryption should succeed");

        // EPKs should be different (random)
        assert_ne!(&ct1[..PUBLIC_KEY_SIZE], &ct2[..PUBLIC_KEY_SIZE]);
    }
}
