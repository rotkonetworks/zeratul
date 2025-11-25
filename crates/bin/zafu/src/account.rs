//! encrypted wallet account storage
//! stores wallet data at ~/.config/zafu/wallet.dat

use anyhow::{anyhow, Result};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use zeroize::{Zeroize, ZeroizeOnDrop};

use crate::crypto::{self, SessionKey};

/// HD derived account (electrum-style)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HdAccount {
    /// account index (0, 1, 2...)
    pub index: u32,
    /// user-defined label
    pub label: String,
    /// balance in zatoshis (cached)
    pub balance: u64,
    /// last known address (for display)
    pub address: Option<String>,
}

impl HdAccount {
    pub fn new(index: u32, label: &str) -> Self {
        Self {
            index,
            label: label.to_string(),
            balance: 0,
            address: None,
        }
    }

    /// default account (account 0)
    pub fn default_account() -> Self {
        Self::new(0, "main")
    }
}

/// wallet account data (stored encrypted)
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct WalletData {
    /// BIP-39 seed phrase (stored encrypted)
    pub seed_phrase: Option<String>,
    /// unified spending key bytes
    pub spending_key: Option<Vec<u8>>,
    /// unified viewing key (for balance checking)
    pub viewing_key: Option<String>,
    /// birthday height (for efficient scanning)
    pub birthday_height: u32,
    /// last synced height
    pub last_sync_height: u32,
    /// zidecar server URL
    pub server_url: String,
    /// zcash node URL (zebrad/zcashd)
    pub node_url: Option<String>,
    /// HD accounts (electrum-style)
    #[serde(default)]
    pub accounts: Vec<HdAccount>,
    /// currently active account index
    #[serde(default)]
    pub active_account: u32,
}

impl WalletData {
    /// get active account or create default
    pub fn get_active_account(&self) -> HdAccount {
        self.accounts.iter()
            .find(|a| a.index == self.active_account)
            .cloned()
            .unwrap_or_else(HdAccount::default_account)
    }

    /// ensure at least one account exists
    pub fn ensure_default_account(&mut self) {
        if self.accounts.is_empty() {
            self.accounts.push(HdAccount::default_account());
        }
    }
}

/// encrypted wallet file format
#[derive(Debug, Serialize, Deserialize)]
pub struct WalletFile {
    pub version: u32,
    pub encrypted_data: String, // hex-encoded encrypted WalletData
}

impl WalletFile {
    const VERSION: u32 = 1;

    /// get wallet file path
    pub fn wallet_path() -> Result<PathBuf> {
        let config_dir = dirs::config_dir()
            .ok_or_else(|| anyhow!("could not find config directory"))?;
        Ok(config_dir.join("zafu").join("wallet.dat"))
    }

    /// check if wallet exists
    pub fn exists() -> bool {
        Self::wallet_path()
            .map(|p| p.exists())
            .unwrap_or(false)
    }

    /// save wallet data with password
    pub fn save(password: &str, data: &WalletData) -> Result<()> {
        let json = serde_json::to_vec(data)?;
        let encrypted = crypto::encrypt(password, &json)?;

        let wallet_file = WalletFile {
            version: Self::VERSION,
            encrypted_data: hex::encode(&encrypted),
        };

        let path = Self::wallet_path()?;
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }

        let toml = toml::to_string_pretty(&wallet_file)?;
        std::fs::write(&path, toml)?;

        Ok(())
    }

    /// save wallet data with session key (no password re-entry)
    pub fn save_with_session(session: &SessionKey, data: &WalletData) -> Result<()> {
        let json = serde_json::to_vec(data)?;
        let encrypted = session.encrypt(&json)?;

        let wallet_file = WalletFile {
            version: Self::VERSION,
            encrypted_data: hex::encode(&encrypted),
        };

        let path = Self::wallet_path()?;
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }

        let toml = toml::to_string_pretty(&wallet_file)?;
        std::fs::write(&path, toml)?;

        Ok(())
    }

    /// load wallet data with password
    pub fn load(password: &str) -> Result<(WalletData, SessionKey)> {
        let path = Self::wallet_path()?;
        let content = std::fs::read_to_string(&path)?;
        let wallet_file: WalletFile = toml::from_str(&content)?;

        if wallet_file.version != Self::VERSION {
            return Err(anyhow!("unsupported wallet version: {}", wallet_file.version));
        }

        let encrypted = hex::decode(&wallet_file.encrypted_data)?;
        let session = SessionKey::from_encrypted(password, &encrypted)?;
        let json = session.decrypt(&encrypted)?;
        let data: WalletData = serde_json::from_slice(&json)?;

        Ok((data, session))
    }

    /// delete wallet file
    pub fn delete() -> Result<()> {
        let path = Self::wallet_path()?;
        if path.exists() {
            std::fs::remove_file(&path)?;
        }
        Ok(())
    }
}

/// active wallet session (in-memory after login)
pub struct WalletSession {
    pub data: WalletData,
    pub session_key: SessionKey,
}

impl WalletSession {
    /// create new wallet session from password
    pub fn new(password: &str, data: WalletData) -> Self {
        let session_key = SessionKey::from_password(password);
        Self { data, session_key }
    }

    /// load existing wallet
    pub fn load(password: &str) -> Result<Self> {
        let (data, session_key) = WalletFile::load(password)?;
        Ok(Self { data, session_key })
    }

    /// save wallet data
    pub fn save(&self) -> Result<()> {
        WalletFile::save_with_session(&self.session_key, &self.data)
    }

    /// update viewing key
    pub fn set_viewing_key(&mut self, key: &str) -> Result<()> {
        self.data.viewing_key = Some(key.to_string());
        self.save()
    }

    /// update sync height
    pub fn set_sync_height(&mut self, height: u32) -> Result<()> {
        self.data.last_sync_height = height;
        self.save()
    }
}

/// secure seed phrase holder (zeroized on drop)
#[derive(Zeroize, ZeroizeOnDrop)]
pub struct SeedPhrase {
    words: String,
}

impl SeedPhrase {
    /// generate new 24-word BIP-39 seed phrase
    pub fn generate() -> Self {
        use bip39::Mnemonic;

        // generate 256 bits of entropy for 24 words
        let mnemonic = Mnemonic::generate(24)
            .expect("failed to generate mnemonic");

        Self { words: mnemonic.to_string() }
    }

    /// create from existing phrase (validates BIP-39)
    pub fn from_str(phrase: &str) -> Result<Self> {
        use bip39::Mnemonic;

        let word_count = phrase.split_whitespace().count();
        if word_count != 12 && word_count != 24 {
            return Err(anyhow!("seed phrase must be 12 or 24 words"));
        }

        // validate against BIP-39 wordlist
        Mnemonic::parse(phrase)
            .map_err(|e| anyhow!("invalid seed phrase: {}", e))?;

        Ok(Self { words: phrase.to_string() })
    }

    /// get words as string slice
    pub fn as_str(&self) -> &str {
        &self.words
    }

    /// get words as vector
    pub fn words(&self) -> Vec<&str> {
        self.words.split_whitespace().collect()
    }

    /// derive seed bytes for key derivation
    pub fn to_seed(&self, passphrase: &str) -> [u8; 64] {
        use bip39::Mnemonic;

        let mnemonic = Mnemonic::parse(&self.words)
            .expect("already validated");
        mnemonic.to_seed(passphrase)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn test_wallet_save_load() {
        // temporarily override config dir
        let data = WalletData {
            viewing_key: Some("uview1test...".into()),
            birthday_height: 1000000,
            last_sync_height: 1000500,
            server_url: "http://localhost:50051".into(),
            ..Default::default()
        };

        let password = "test_password";
        let session = WalletSession::new(password, data.clone());

        // can't easily test file save without mocking dirs
        // but the crypto roundtrip is tested in crypto.rs
    }
}
