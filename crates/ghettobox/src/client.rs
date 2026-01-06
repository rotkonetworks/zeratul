//! high-level client for account management
//!
//! ties together: PIN stretching, VSS, network, account derivation

use crate::account::{Account, hash_email};
use crate::crypto::{stretch_pin, random_bytes, unlock_key_tag};
use crate::vss;
use crate::{Error, Result};

/// ghettobox client for account management
pub struct Client {
    /// realm id for this network
    realm_id: [u8; 32],
    #[cfg(feature = "network")]
    network: crate::network::NetworkClient,
}

/// account creation result
pub struct CreateAccountResult {
    /// the account (with signing key)
    pub account: Account,
    /// user share (for backup, e.g. email or paper)
    pub user_share: UserShare,
    /// vss shares (distributed to TPM nodes)
    pub vss_shares: [vss::Share; 3],
}

/// user share for backup
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct UserShare {
    /// email hash (for lookup)
    pub email_hash: [u8; 32],
    /// version/salt for PIN stretching
    pub version: [u8; 16],
    /// encrypted seed (encrypted with key derived from stretched PIN)
    pub encrypted_seed: Vec<u8>,
}

impl Client {
    /// create client for rotko network
    #[cfg(feature = "network")]
    pub fn rotko() -> Result<Self> {
        Ok(Self {
            realm_id: *b"rotko.network:ghettobox:v1\0\0\0\0\0\0",
            network: crate::network::NetworkClient::rotko_mainnet()?,
        })
    }

    /// create client for local development
    #[cfg(feature = "network")]
    pub fn localhost() -> Result<Self> {
        Ok(Self {
            realm_id: *b"localhost:ghettobox:dev\0\0\0\0\0\0\0\0\0",
            network: crate::network::NetworkClient::localhost()?,
        })
    }

    /// create offline client (for testing)
    pub fn offline() -> Self {
        Self {
            realm_id: *b"offline:ghettobox:test\0\0\0\0\0\0\0\0\0\0",
            #[cfg(feature = "network")]
            network: crate::network::NetworkClient::localhost().unwrap(),
        }
    }

    /// create a new account
    ///
    /// # flow
    /// 1. generate random seed
    /// 2. split seed into 3 VSS shares
    /// 3. stretch PIN to get unlock key
    /// 4. encrypt seed with key derived from PIN
    /// 5. derive account (ed25519) from seed
    ///
    /// # returns
    /// - account with signing key
    /// - user share (for backup)
    /// - vss shares (to distribute to TPM nodes)
    pub fn create_account(
        &self,
        email: &str,
        pin: &[u8],
    ) -> Result<CreateAccountResult> {
        // generate random seed
        let seed: [u8; 32] = random_bytes();

        // split into VSS shares
        let vss_shares = vss::split_secret(&seed)?;

        // derive account from seed
        let account = Account::from_seed(&seed)?;

        // create user share for backup
        let email_hash = hash_email(email);
        let version: [u8; 16] = random_bytes();

        // stretch PIN
        let stretched = stretch_pin(pin, &version, email.as_bytes())?;
        let encryption_key: [u8; 32] = stretched[32..].try_into().unwrap();

        // encrypt seed
        let nonce = [0u8; 12]; // ok since key is unique per registration
        let encrypted_seed = crate::crypto::encrypt(&encryption_key, &seed, &nonce)?;

        let user_share = UserShare {
            email_hash,
            version,
            encrypted_seed,
        };

        Ok(CreateAccountResult {
            account,
            user_share,
            vss_shares,
        })
    }

    /// recover account from user share and vss shares
    pub fn recover_account(
        &self,
        email: &str,
        pin: &[u8],
        user_share: &UserShare,
        vss_shares: &[vss::Share],
    ) -> Result<Account> {
        // verify email matches
        let email_hash = hash_email(email);
        if email_hash != user_share.email_hash {
            return Err(Error::InvalidPin); // don't leak which part failed
        }

        // reconstruct seed from VSS shares
        let seed = vss::combine_shares(vss_shares)?;

        // also decrypt from user share to verify
        let stretched = stretch_pin(pin, &user_share.version, email.as_bytes())?;
        let encryption_key: [u8; 32] = stretched[32..].try_into().unwrap();

        let nonce = [0u8; 12];
        let decrypted_seed = crate::crypto::decrypt(&encryption_key, &user_share.encrypted_seed, &nonce)?;

        // verify both methods give same seed
        if decrypted_seed != seed {
            return Err(Error::ShareVerificationFailed);
        }

        // derive account from seed
        let seed_arr: [u8; 32] = seed.try_into().map_err(|_| Error::InvalidSecretLength)?;
        Account::from_seed(&seed_arr)
    }

    /// compute unlock key tag for a given email/pin (for network auth)
    pub fn compute_unlock_tag(&self, email: &str, pin: &[u8], version: &[u8; 16]) -> Result<[u8; 16]> {
        let stretched = stretch_pin(pin, version, email.as_bytes())?;
        let access_key: [u8; 32] = stretched[..32].try_into().unwrap();
        Ok(unlock_key_tag(&access_key, &self.realm_id))
    }

    /// register account with network (distributes shares to TPM nodes)
    #[cfg(feature = "network")]
    pub async fn register_network(
        &self,
        email: &str,
        pin: &[u8],
        result: &CreateAccountResult,
    ) -> Result<()> {
        let unlock_tag = self.compute_unlock_tag(email, pin, &result.user_share.version)?;

        self.network.register(
            result.user_share.email_hash,
            unlock_tag,
            &result.vss_shares,
            5, // 5 allowed guesses
        ).await?;

        Ok(())
    }

    /// recover account from network (fetches shares from TPM nodes)
    #[cfg(feature = "network")]
    pub async fn recover_network(
        &self,
        email: &str,
        pin: &[u8],
        user_share: &UserShare,
    ) -> Result<Account> {
        let unlock_tag = self.compute_unlock_tag(email, pin, &user_share.version)?;

        // fetch shares from network
        let vss_shares = self.network.recover(
            user_share.email_hash,
            unlock_tag,
        ).await?;

        // recover account
        self.recover_account(email, pin, user_share, &vss_shares)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_create_recover_offline() {
        let client = Client::offline();

        let email = "alice@example.com";
        let pin = b"1234";

        // create account
        let result = client.create_account(email, pin).unwrap();
        println!("account address: {}", result.account.address_hex());

        // recover with 2 of 3 shares
        let recovered = client.recover_account(
            email,
            pin,
            &result.user_share,
            &[result.vss_shares[0].clone(), result.vss_shares[2].clone()],
        ).unwrap();

        assert_eq!(result.account.address, recovered.address);
    }

    #[test]
    fn test_wrong_pin_fails() {
        let client = Client::offline();

        let email = "bob@example.com";
        let pin = b"1234";
        let wrong_pin = b"5678";

        let result = client.create_account(email, pin).unwrap();

        // try to recover with wrong PIN
        let recovered = client.recover_account(
            email,
            wrong_pin,
            &result.user_share,
            &[result.vss_shares[0].clone(), result.vss_shares[1].clone()],
        );

        assert!(recovered.is_err());
    }
}
