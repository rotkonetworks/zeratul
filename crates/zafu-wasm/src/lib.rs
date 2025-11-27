//! WASM bindings for Zafu Zcash wallet
//!
//! Provides trial decryption and key derivation for browser-based Zcash wallets.
//! This implementation avoids the heavy `orchard` crate's circuit dependencies
//! by implementing decryption directly using pasta_curves.
//!
//! Build with:
//! ```bash
//! wasm-pack build --target web --out-dir ../bin/zidecar/www/pkg
//! ```

use wasm_bindgen::prelude::*;
use pasta_curves::pallas;
use ff::PrimeField;
use group::GroupEncoding;
use blake2::{Blake2b512, Digest};
use chacha20poly1305::{ChaCha20Poly1305, KeyInit, aead::Aead};
use serde::{Deserialize, Serialize};
use subtle::ConstantTimeEq;

/// Initialize panic hook for better error messages
#[wasm_bindgen(start)]
pub fn init() {
    console_error_panic_hook::set_once();
}

/// Wallet keys derived from seed phrase
#[wasm_bindgen]
pub struct WalletKeys {
    /// Incoming viewing key (scalar)
    ivk: pallas::Scalar,
    /// Prepared ivk for faster decryption
    prepared_ivk_bytes: [u8; 32],
}

#[wasm_bindgen]
impl WalletKeys {
    /// Derive wallet keys from a 24-word BIP39 seed phrase
    #[wasm_bindgen(constructor)]
    pub fn from_seed_phrase(seed_phrase: &str) -> Result<WalletKeys, JsError> {
        let mnemonic = bip39::Mnemonic::parse(seed_phrase)
            .map_err(|e| JsError::new(&format!("Invalid seed phrase: {}", e)))?;

        // Derive seed bytes
        let seed = mnemonic.to_seed("");

        // Derive Orchard spending key using BLAKE2b
        let sk_bytes = derive_orchard_sk_bytes(&seed);

        // Derive full viewing key components
        // sk -> ask, nk, rivk (spending authority, nullifier, randomness)
        // For trial decryption we only need ivk = Commit^ivk_rivk(ask, nk)
        let (ivk, ivk_bytes) = derive_ivk(&sk_bytes);

        Ok(WalletKeys {
            ivk,
            prepared_ivk_bytes: ivk_bytes,
        })
    }

    /// Get the wallet's receiving address (simplified - returns ivk hash as identifier)
    #[wasm_bindgen]
    pub fn get_address(&self) -> String {
        // For demo purposes, return the ivk hash as address identifier
        // Real implementation would derive diversified address
        hex_encode(&self.prepared_ivk_bytes)
    }

    /// Scan a batch of compact actions and return found notes as JSON
    #[wasm_bindgen]
    pub fn scan_actions(&self, actions_json: JsValue) -> Result<JsValue, JsError> {
        let actions: Vec<CompactActionJs> = serde_wasm_bindgen::from_value(actions_json)
            .map_err(|e| JsError::new(&format!("Invalid actions JSON: {}", e)))?;

        let mut found_notes = Vec::new();

        for (idx, action) in actions.iter().enumerate() {
            if let Some(note) = self.try_decrypt_action(action) {
                found_notes.push(FoundNote {
                    index: idx as u32,
                    value: note.value,
                    nullifier: action.nullifier.clone(),
                    cmx: action.cmx.clone(),
                });
            }
        }

        serde_wasm_bindgen::to_value(&found_notes)
            .map_err(|e| JsError::new(&format!("Failed to serialize results: {}", e)))
    }

    /// Get total balance from found notes
    #[wasm_bindgen]
    pub fn calculate_balance(&self, notes_json: JsValue, spent_nullifiers_json: JsValue) -> Result<u64, JsError> {
        let notes: Vec<FoundNote> = serde_wasm_bindgen::from_value(notes_json)
            .map_err(|e| JsError::new(&format!("Invalid notes JSON: {}", e)))?;
        let spent: Vec<String> = serde_wasm_bindgen::from_value(spent_nullifiers_json)
            .map_err(|e| JsError::new(&format!("Invalid nullifiers JSON: {}", e)))?;

        let balance: u64 = notes.iter()
            .filter(|n| !spent.contains(&n.nullifier))
            .map(|n| n.value)
            .sum();

        Ok(balance)
    }

    fn try_decrypt_action(&self, action: &CompactActionJs) -> Option<DecryptedNote> {
        // Parse ephemeral key (32 bytes -> Pallas point)
        let epk_bytes: [u8; 32] = hex_decode(&action.ephemeral_key)?.try_into().ok()?;
        let epk = pallas::Affine::from_bytes(&epk_bytes);
        if epk.is_none().into() {
            return None;
        }
        let epk = epk.unwrap();

        // Compute shared secret: [ivk] * epk
        let shared_secret = (epk * self.ivk).to_bytes();

        // Derive symmetric key using BLAKE2b
        let sym_key = derive_symmetric_key(&shared_secret, &epk_bytes);

        // Parse and decrypt ciphertext
        let ciphertext = hex_decode(&action.ciphertext)?;
        if ciphertext.len() < 52 {
            return None;
        }

        // Try to decrypt compact ciphertext (52 bytes)
        // Orchard compact ciphertext: [diversifier(11) || value(8) || rseed(32)] encrypted
        // Total plaintext: 51 bytes, ciphertext: 52 bytes (with 1-byte tag padding for alignment)
        let cipher = ChaCha20Poly1305::new_from_slice(&sym_key).ok()?;

        // For compact decryption, we need to handle the compact format
        // The ciphertext is 52 bytes = 36 bytes encrypted + 16 byte tag
        if ciphertext.len() < 52 {
            return None;
        }

        // Use nullifier as nonce (first 12 bytes)
        let nullifier_bytes: [u8; 32] = hex_decode(&action.nullifier)?.try_into().ok()?;
        let nonce: [u8; 12] = nullifier_bytes[..12].try_into().ok()?;

        // For compact actions, we only decrypt the first part to get value
        // Full decryption would require more ciphertext
        // For demo: extract value assuming decryption succeeds
        let plaintext = cipher.decrypt((&nonce).into(), &ciphertext[..52]).ok()?;

        if plaintext.len() < 19 {
            return None;
        }

        // Extract value (8 bytes little-endian at offset 11)
        let value = u64::from_le_bytes(plaintext[11..19].try_into().ok()?);

        Some(DecryptedNote {
            value,
            nullifier: action.nullifier.clone(),
        })
    }
}

/// Compact action from JavaScript
#[derive(Debug, Deserialize)]
struct CompactActionJs {
    nullifier: String,      // hex (32 bytes)
    cmx: String,            // hex (32 bytes)
    ephemeral_key: String,  // hex (32 bytes)
    ciphertext: String,     // hex (52 bytes for compact)
}

/// Decrypted note (internal)
struct DecryptedNote {
    value: u64,
    nullifier: String,
}

/// Found note to return to JavaScript
#[derive(Debug, Serialize, Deserialize)]
pub struct FoundNote {
    pub index: u32,
    pub value: u64,
    pub nullifier: String,
    pub cmx: String,
}

/// Derive Orchard spending key bytes from BIP39 seed
fn derive_orchard_sk_bytes(seed: &[u8]) -> [u8; 32] {
    let mut hasher = Blake2b512::new();
    hasher.update(b"Zcash_OrchardZIP32");
    hasher.update(seed);
    let hash = hasher.finalize();

    let mut result = [0u8; 32];
    result.copy_from_slice(&hash[..32]);
    result
}

/// Derive incoming viewing key from spending key
fn derive_ivk(sk: &[u8; 32]) -> (pallas::Scalar, [u8; 32]) {
    // Simplified IVK derivation for demo
    // Real Orchard: ivk = Commit^ivk_rivk(ak, nk) mod q
    let mut hasher = Blake2b512::new();
    hasher.update(b"Zcash_OrchardIVK_");
    hasher.update(sk);
    let hash = hasher.finalize();

    let mut ivk_bytes = [0u8; 32];
    ivk_bytes.copy_from_slice(&hash[..32]);

    // Reduce to scalar field
    let ivk = pallas::Scalar::from_repr(ivk_bytes).unwrap_or(pallas::Scalar::zero());

    (ivk, ivk_bytes)
}

/// Derive symmetric key for note decryption
fn derive_symmetric_key(shared_secret: &[u8; 32], epk: &[u8; 32]) -> [u8; 32] {
    let mut hasher = Blake2b512::new();
    hasher.update(b"Zcash_OrchardKDF_");
    hasher.update(shared_secret);
    hasher.update(epk);
    let hash = hasher.finalize();

    let mut key = [0u8; 32];
    key.copy_from_slice(&hash[..32]);
    key
}

/// Generate a new 24-word seed phrase
#[wasm_bindgen]
pub fn generate_seed_phrase() -> Result<String, JsError> {
    let mnemonic = bip39::Mnemonic::generate(24)
        .map_err(|e| JsError::new(&format!("Failed to generate mnemonic: {}", e)))?;
    Ok(mnemonic.to_string())
}

/// Validate a seed phrase without creating keys
#[wasm_bindgen]
pub fn validate_seed_phrase(seed_phrase: &str) -> bool {
    bip39::Mnemonic::parse(seed_phrase).is_ok()
}

/// Get the current library version
#[wasm_bindgen]
pub fn version() -> String {
    env!("CARGO_PKG_VERSION").to_string()
}

// Hex helpers
fn hex_encode(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{:02x}", b)).collect()
}

fn hex_decode(s: &str) -> Option<Vec<u8>> {
    if s.len() % 2 != 0 {
        return None;
    }
    (0..s.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&s[i..i + 2], 16).ok())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_seed_validation() {
        assert!(validate_seed_phrase("abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon art"));
        assert!(!validate_seed_phrase("invalid seed phrase"));
    }

    #[test]
    fn test_key_derivation() {
        let keys = WalletKeys::from_seed_phrase(
            "abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon art"
        ).unwrap();

        let address = keys.get_address();
        assert_eq!(address.len(), 64); // 32 bytes = 64 hex chars
    }

    #[test]
    fn test_generate_seed() {
        let seed = generate_seed_phrase().unwrap();
        let words: Vec<&str> = seed.split_whitespace().collect();
        assert_eq!(words.len(), 24);
        assert!(validate_seed_phrase(&seed));
    }
}
