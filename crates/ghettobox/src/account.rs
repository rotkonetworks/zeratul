//! account and key management
//!
//! derives signing keys from recovered seed

use crate::{Error, Result};
use ed25519_dalek::{SigningKey, VerifyingKey, Signer, Signature};
use hkdf::Hkdf;
use sha2::Sha256;

/// account derived from seed
pub struct Account {
    /// signing key for transactions
    signing_key: SigningKey,
    /// public key / account address
    pub address: [u8; 32],
}

impl Account {
    /// derive account from seed
    pub fn from_seed(seed: &[u8; 32]) -> Result<Self> {
        // derive signing key using hkdf
        let hk = Hkdf::<Sha256>::new(None, seed);
        let mut signing_bytes = [0u8; 32];
        hk.expand(b"ghettobox:ed25519:v1", &mut signing_bytes)
            .map_err(|_| Error::KeyDerivationFailed)?;

        let signing_key = SigningKey::from_bytes(&signing_bytes);
        let verifying_key = signing_key.verifying_key();
        let address = verifying_key.to_bytes();

        Ok(Self {
            signing_key,
            address,
        })
    }

    /// get public key
    pub fn public_key(&self) -> VerifyingKey {
        self.signing_key.verifying_key()
    }

    /// sign a message
    pub fn sign(&self, message: &[u8]) -> Signature {
        self.signing_key.sign(message)
    }

    /// sign and return bytes
    pub fn sign_bytes(&self, message: &[u8]) -> [u8; 64] {
        self.sign(message).to_bytes()
    }

    /// get address as hex string
    pub fn address_hex(&self) -> String {
        hex::encode(self.address)
    }

    /// derive a sub-key for specific purpose
    pub fn derive_subkey(&self, purpose: &[u8]) -> Result<SigningKey> {
        let hk = Hkdf::<Sha256>::new(None, self.signing_key.as_bytes());
        let mut subkey_bytes = [0u8; 32];
        hk.expand(purpose, &mut subkey_bytes)
            .map_err(|_| Error::KeyDerivationFailed)?;
        Ok(SigningKey::from_bytes(&subkey_bytes))
    }
}

/// account registration for on-chain
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct AccountRegistration {
    /// public key / address
    pub address: [u8; 32],
    /// email hash (for recovery identification)
    pub email_hash: [u8; 32],
    /// registration timestamp
    pub registered_at: u64,
    /// account status
    pub status: AccountStatus,
}

/// account status on chain
#[derive(Clone, Debug, Default, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
pub enum AccountStatus {
    #[default]
    Active,
    Suspended,
    Deleted,
}

/// hash email for privacy
pub fn hash_email(email: &str) -> [u8; 32] {
    use sha2::{Sha256, Digest};
    let mut hasher = Sha256::new();
    hasher.update(b"ghettobox:email:v1:");
    hasher.update(email.to_lowercase().trim().as_bytes());
    hasher.finalize().into()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_account_derivation() {
        let seed = [42u8; 32];
        let account = Account::from_seed(&seed).unwrap();

        // same seed should give same account
        let account2 = Account::from_seed(&seed).unwrap();
        assert_eq!(account.address, account2.address);

        // different seed should give different account
        let seed2 = [43u8; 32];
        let account3 = Account::from_seed(&seed2).unwrap();
        assert_ne!(account.address, account3.address);
    }

    #[test]
    fn test_signing() {
        let seed = [42u8; 32];
        let account = Account::from_seed(&seed).unwrap();

        let message = b"hello world";
        let signature = account.sign(message);

        // verify signature
        use ed25519_dalek::Verifier;
        assert!(account.public_key().verify(message, &signature).is_ok());
    }

    #[test]
    fn test_email_hash() {
        let hash1 = hash_email("Alice@Example.com");
        let hash2 = hash_email("alice@example.com");
        let hash3 = hash_email("  alice@example.com  ");

        // should normalize
        assert_eq!(hash1, hash2);
        assert_eq!(hash2, hash3);
    }
}
