//! ghettobox protocol implementation
//!
//! simplified juicebox-inspired protocol:
//! - pin stretched with argon2id
//! - secret split with xor into realm_share + user_share
//! - realm_share sealed to tpm
//! - user_share returned to caller (for email/matrix/paper/etc)
//!
//! recovery:
//! 1. client provides pin → derive access key
//! 2. realm verifies access key tag → returns sealed share
//! 3. client combines with user_share → recover secret

use crate::crypto::{
    combine_shares, decrypt, derive_encryption_key, encrypt, random_bytes,
    split_secret, split_stretched_pin, stretch_pin, unlock_key_tag, mac,
};
use crate::realm::{Realm, Registration};
use crate::share::Share;
use crate::{Error, Result};

/// ghettobox protocol client
pub struct Ghettobox<R: Realm> {
    realm: R,
    /// default allowed guesses before secret is destroyed
    default_allowed_guesses: u32,
}

impl<R: Realm> Ghettobox<R> {
    /// create a new ghettobox instance
    pub fn new(realm: R) -> Self {
        Self {
            realm,
            default_allowed_guesses: 5,
        }
    }

    /// set default allowed guesses
    pub fn with_allowed_guesses(mut self, guesses: u32) -> Self {
        self.default_allowed_guesses = guesses;
        self
    }

    /// register a secret protected by pin
    ///
    /// # arguments
    /// * `user_id` - unique identifier for this user
    /// * `pin` - user's pin (can be low entropy, e.g. 4 digits)
    /// * `secret` - the secret to protect
    /// * `user_info` - additional info to mix into kdf (e.g. email)
    ///
    /// # returns
    /// user share that must be stored separately (email, paper, etc)
    pub fn register(
        &self,
        user_id: &str,
        pin: &[u8],
        secret: &[u8],
        user_info: &[u8],
    ) -> Result<Share> {
        self.register_with_guesses(user_id, pin, secret, user_info, self.default_allowed_guesses)
    }

    /// register with custom guess limit
    pub fn register_with_guesses(
        &self,
        user_id: &str,
        pin: &[u8],
        secret: &[u8],
        user_info: &[u8],
        allowed_guesses: u32,
    ) -> Result<Share> {
        // generate random version/salt
        let version: [u8; 16] = random_bytes();

        // stretch pin
        let stretched = stretch_pin(pin, &version, user_info)?;
        let (access_key, encryption_seed) = split_stretched_pin(&stretched);

        // derive unlock key (simulating oprf result)
        // in full juicebox this would be a t-oprf across realms
        let unlock_key = mac(&access_key, &[b"ghettobox:unlock_key:v1", self.realm.id()]);
        let unlock_key_commitment = mac(&unlock_key, &[b"ghettobox:commitment:v1"]);

        // split secret
        let (realm_share, user_share_bytes) = split_secret(secret);

        // seal realm share
        let sealed_share = self.realm.seal(&realm_share)?;

        // encrypt secret with key derived from encryption seed
        let encryption_scalar: [u8; 32] = random_bytes();
        let encryption_key = derive_encryption_key(&encryption_seed, &encryption_scalar);
        let nonce: [u8; 12] = [0u8; 12]; // ok since key is unique per registration
        let encrypted_secret = encrypt(&encryption_key, secret, &nonce)?;

        // compute commitments and tags
        let unlock_tag = unlock_key_tag(&unlock_key, self.realm.id());

        let secret_commitment = {
            let c = mac(&unlock_key, &[
                b"ghettobox:secret_commitment:v1",
                self.realm.id(),
                &encryption_scalar,
                &encrypted_secret,
            ]);
            let mut arr = [0u8; 16];
            arr.copy_from_slice(&c[..16]);
            arr
        };

        // store registration
        let registration = Registration {
            version,
            sealed_share,
            unlock_key_commitment,
            unlock_key_tag: unlock_tag,
            encrypted_secret,
            secret_commitment,
            allowed_guesses,
            attempted_guesses: 0,
        };

        self.realm.store(user_id, &registration)?;

        // combine user share with encryption scalar for recovery
        let mut user_share_data = Vec::with_capacity(user_share_bytes.len() + 32);
        user_share_data.extend_from_slice(&encryption_scalar);
        user_share_data.extend_from_slice(&user_share_bytes);

        Ok(Share::new(user_share_data))
    }

    /// recover a secret using pin and user share
    ///
    /// # arguments
    /// * `user_id` - unique identifier for this user
    /// * `pin` - user's pin
    /// * `user_share` - the share returned during registration
    /// * `user_info` - same user_info used during registration
    ///
    /// # returns
    /// the recovered secret
    pub fn recover(
        &self,
        user_id: &str,
        pin: &[u8],
        user_share: &Share,
        user_info: &[u8],
    ) -> Result<Vec<u8>> {
        // phase 1: get registration state
        let registration = self.realm.load(user_id)?
            .ok_or(Error::NotRegistered)?;

        if registration.attempted_guesses >= registration.allowed_guesses {
            return Err(Error::NoGuessesRemaining);
        }

        // phase 2: derive access key and verify
        self.realm.check_rate_limit(user_id)?;

        let stretched = stretch_pin(pin, &registration.version, user_info)?;
        let (access_key, encryption_seed) = split_stretched_pin(&stretched);

        // derive unlock key
        let unlock_key = mac(&access_key, &[b"ghettobox:unlock_key:v1", self.realm.id()]);
        let unlock_key_commitment = mac(&unlock_key, &[b"ghettobox:commitment:v1"]);

        // verify commitment
        if unlock_key_commitment != registration.unlock_key_commitment {
            // wrong pin - increment attempts
            self.realm.increment_attempts(user_id)?;
            return Err(Error::InvalidPin);
        }

        // phase 3: verify unlock key tag and recover
        let unlock_tag = unlock_key_tag(&unlock_key, self.realm.id());

        if unlock_tag != registration.unlock_key_tag {
            self.realm.increment_attempts(user_id)?;
            return Err(Error::InvalidPin);
        }

        // success - reset attempts
        self.realm.reset_attempts(user_id)?;

        // unseal realm share
        let realm_share = self.realm.unseal(&registration.sealed_share)?;

        // parse user share
        user_share.verify()?;
        if user_share.data.len() < 32 {
            return Err(Error::InvalidShareFormat);
        }

        let encryption_scalar: [u8; 32] = user_share.data[..32].try_into().unwrap();
        let user_share_bytes = &user_share.data[32..];

        // verify secret commitment
        let expected_commitment = {
            let c = mac(&unlock_key, &[
                b"ghettobox:secret_commitment:v1",
                self.realm.id(),
                &encryption_scalar,
                &registration.encrypted_secret,
            ]);
            let mut arr = [0u8; 16];
            arr.copy_from_slice(&c[..16]);
            arr
        };

        if expected_commitment != registration.secret_commitment {
            return Err(Error::ShareVerificationFailed);
        }

        // recover secret via xor
        let secret_from_shares = combine_shares(&realm_share, user_share_bytes);

        // also decrypt and verify (belt and suspenders)
        let encryption_key = derive_encryption_key(&encryption_seed, &encryption_scalar);
        let nonce: [u8; 12] = [0u8; 12];
        let decrypted_secret = decrypt(&encryption_key, &registration.encrypted_secret, &nonce)?;

        // both methods should give same result
        if secret_from_shares != decrypted_secret {
            return Err(Error::ShareVerificationFailed);
        }

        Ok(decrypted_secret)
    }

    /// delete a registration
    pub fn delete(&self, user_id: &str) -> Result<()> {
        self.realm.delete(user_id)
    }

    /// check registration status
    pub fn status(&self, user_id: &str) -> Result<RegistrationStatus> {
        match self.realm.load(user_id)? {
            None => Ok(RegistrationStatus::NotRegistered),
            Some(reg) if reg.attempted_guesses >= reg.allowed_guesses => {
                Ok(RegistrationStatus::NoGuessesRemaining)
            }
            Some(reg) => Ok(RegistrationStatus::Registered {
                guesses_remaining: reg.allowed_guesses - reg.attempted_guesses,
            }),
        }
    }
}

/// registration status
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RegistrationStatus {
    NotRegistered,
    Registered { guesses_remaining: u32 },
    NoGuessesRemaining,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::realm::software::SoftwareRealm;

    #[test]
    fn test_register_recover() {
        let realm = SoftwareRealm::new();
        let gb = Ghettobox::new(realm);

        let user_id = "alice";
        let pin = b"1234";
        let secret = b"my super secret seed phrase";
        let user_info = b"alice@example.com";

        // register
        let user_share = gb.register(user_id, pin, secret, user_info).unwrap();

        // recover with correct pin
        let recovered = gb.recover(user_id, pin, &user_share, user_info).unwrap();
        assert_eq!(secret.as_slice(), recovered.as_slice());
    }

    #[test]
    fn test_wrong_pin() {
        let realm = SoftwareRealm::new();
        let gb = Ghettobox::new(realm);

        let user_id = "bob";
        let pin = b"1234";
        let wrong_pin = b"5678";
        let secret = b"secret";
        let user_info = b"bob@example.com";

        let user_share = gb.register(user_id, pin, secret, user_info).unwrap();

        let result = gb.recover(user_id, wrong_pin, &user_share, user_info);
        assert!(matches!(result, Err(Error::InvalidPin)));
    }

    #[test]
    fn test_guess_limit() {
        let realm = SoftwareRealm::new();
        let gb = Ghettobox::new(realm).with_allowed_guesses(3);

        let user_id = "charlie";
        let pin = b"1234";
        let wrong_pin = b"0000";
        let secret = b"secret";
        let user_info = b"";

        let user_share = gb.register(user_id, pin, secret, user_info).unwrap();

        // use up all guesses
        for _ in 0..3 {
            let _ = gb.recover(user_id, wrong_pin, &user_share, user_info);
        }

        // should now be locked out
        let result = gb.recover(user_id, pin, &user_share, user_info);
        assert!(matches!(result, Err(Error::NoGuessesRemaining) | Err(Error::NotRegistered)));
    }

    #[test]
    fn test_share_serialization() {
        let realm = SoftwareRealm::new();
        let gb = Ghettobox::new(realm);

        let user_id = "dave";
        let pin = b"4321";
        let secret = b"another secret";
        let user_info = b"";

        let user_share = gb.register(user_id, pin, secret, user_info).unwrap();

        // test various serialization formats
        let hex = user_share.to_hex();
        let recovered_share = Share::from_hex(&hex).unwrap();
        let recovered = gb.recover(user_id, pin, &recovered_share, user_info).unwrap();
        assert_eq!(secret.as_slice(), recovered.as_slice());

        // base64
        let b64 = user_share.to_base64();
        let recovered_share = Share::from_base64(&b64).unwrap();
        let recovered = gb.recover(user_id, pin, &recovered_share, user_info).unwrap();
        assert_eq!(secret.as_slice(), recovered.as_slice());

        // words
        let words = user_share.to_words();
        println!("recovery words: {}", words);
    }
}
