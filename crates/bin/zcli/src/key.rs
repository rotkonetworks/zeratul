use blake2::{Blake2b512, Digest};
use zeroize::Zeroize;

use crate::error::Error;

/// 64-byte wallet seed derived from ssh key or mnemonic
pub struct WalletSeed {
    bytes: [u8; 64],
}

impl WalletSeed {
    pub fn as_bytes(&self) -> &[u8; 64] {
        &self.bytes
    }

    pub fn from_bytes(bytes: [u8; 64]) -> Self {
        Self { bytes }
    }
}

impl Drop for WalletSeed {
    fn drop(&mut self) {
        self.bytes.zeroize();
    }
}

/// load wallet seed from ed25519 ssh private key
///
/// derivation: BLAKE2b-512("ZcliWalletSeed" || ed25519_seed_32bytes)
pub fn load_ssh_seed(path: &str) -> Result<WalletSeed, Error> {
    let key_data = std::fs::read_to_string(path)
        .map_err(|e| Error::Key(format!("cannot read {}: {}", path, e)))?;
    let passphrase = ssh_passphrase(&key_data)?;

    let private_key = if let Some(pw) = &passphrase {
        ssh_key::PrivateKey::from_openssh(&key_data)
            .and_then(|k| k.decrypt(pw))
            .map_err(|e| Error::Key(format!("cannot decrypt ssh key: {}", e)))?
    } else {
        ssh_key::PrivateKey::from_openssh(&key_data)
            .map_err(|e| Error::Key(format!("cannot parse ssh key: {}", e)))?
    };

    let ed25519_keypair = match private_key.key_data() {
        ssh_key::private::KeypairData::Ed25519(kp) => kp,
        _ => return Err(Error::Key("not an ed25519 key".into())),
    };

    // ed25519 private key seed is the first 32 bytes
    let seed_bytes = ed25519_keypair.private.as_ref();
    if seed_bytes.len() < 32 {
        return Err(Error::Key("ed25519 seed too short".into()));
    }

    let mut hasher = Blake2b512::new();
    hasher.update(b"ZcliWalletSeed");
    hasher.update(&seed_bytes[..32]);
    let hash = hasher.finalize();

    let mut bytes = [0u8; 64];
    bytes.copy_from_slice(&hash);
    Ok(WalletSeed { bytes })
}

/// load wallet seed from bip39 mnemonic
///
/// derivation: mnemonic.to_seed("") → 64-byte seed (standard bip39)
pub fn load_mnemonic_seed(phrase: &str) -> Result<WalletSeed, Error> {
    let mnemonic = bip39::Mnemonic::parse(phrase)
        .map_err(|e| Error::Key(format!("invalid mnemonic: {}", e)))?;

    let seed = mnemonic.to_seed("");
    let mut bytes = [0u8; 64];
    bytes.copy_from_slice(&seed);
    Ok(WalletSeed { bytes })
}

/// prompt for ssh key passphrase if needed
fn ssh_passphrase(key_data: &str) -> Result<Option<String>, Error> {
    // unencrypted keys don't have ENCRYPTED in the PEM header
    if !key_data.contains("ENCRYPTED") {
        return Ok(None);
    }

    // try env var first (non-interactive)
    if let Ok(pw) = std::env::var("ZCLI_PASSPHRASE") {
        return Ok(Some(pw));
    }

    // if stdin is not a tty, we can't prompt
    if !is_terminal::is_terminal(std::io::stdin()) {
        return Err(Error::Key(
            "encrypted key requires passphrase (set ZCLI_PASSPHRASE or use a terminal)".into(),
        ));
    }

    let pw = rpassword::prompt_password("ssh key passphrase: ")
        .map_err(|e| Error::Key(format!("cannot read passphrase: {}", e)))?;
    Ok(Some(pw))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mnemonic_seed_deterministic() {
        let seed1 = load_mnemonic_seed(
            "abandon abandon abandon abandon abandon abandon abandon abandon \
             abandon abandon abandon abandon abandon abandon abandon abandon \
             abandon abandon abandon abandon abandon abandon abandon art"
        ).unwrap();
        let seed2 = load_mnemonic_seed(
            "abandon abandon abandon abandon abandon abandon abandon abandon \
             abandon abandon abandon abandon abandon abandon abandon abandon \
             abandon abandon abandon abandon abandon abandon abandon art"
        ).unwrap();
        assert_eq!(seed1.as_bytes(), seed2.as_bytes());
    }

    #[test]
    fn mnemonic_seed_is_standard_bip39() {
        // mnemonic.to_seed("") should match standard bip39 derivation
        let phrase = "abandon abandon abandon abandon abandon abandon abandon abandon \
                      abandon abandon abandon abandon abandon abandon abandon abandon \
                      abandon abandon abandon abandon abandon abandon abandon art";
        let seed = load_mnemonic_seed(phrase).unwrap();
        let mnemonic = bip39::Mnemonic::parse(phrase).unwrap();
        let expected = mnemonic.to_seed("");
        assert_eq!(seed.as_bytes(), &expected);
    }
}
