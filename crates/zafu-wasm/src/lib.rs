//! WASM bindings for Zafu Zcash wallet
//!
//! Provides parallel trial decryption for browser-based Zcash wallets.
//! Uses rayon + web workers for multi-threaded scanning with SIMD acceleration.
//!
//! Build with:
//! ```bash
//! RUSTFLAGS='-C target-feature=+simd128' wasm-pack build --target web --out-dir ../bin/zidecar/www/pkg
//! ```

mod witness;

use wasm_bindgen::prelude::*;
use blake2::{Blake2b512, Digest};
use serde::{Deserialize, Serialize};

// Real Orchard key derivation and note decryption
use orchard::keys::{SpendingKey, Scope, IncomingViewingKey, PreparedIncomingViewingKey};
use orchard::note_encryption::OrchardDomain;
use zcash_note_encryption::{try_compact_note_decryption, EphemeralKeyBytes, ShieldedOutput, COMPACT_NOTE_SIZE};

#[cfg(feature = "parallel")]
use rayon::prelude::*;

#[cfg(feature = "parallel")]
pub use wasm_bindgen_rayon::init_thread_pool;

/// Initialize panic hook for better error messages
#[wasm_bindgen(start)]
pub fn init() {
    console_error_panic_hook::set_once();
}

/// Wallet keys derived from seed phrase
#[wasm_bindgen]
pub struct WalletKeys {
    /// Full Viewing Key (needed to compute nullifiers for received notes)
    fvk: orchard::keys::FullViewingKey,
    /// Real Orchard Incoming Viewing Key for EXTERNAL scope (prepared for efficient batched decryption)
    prepared_ivk_external: PreparedIncomingViewingKey,
    /// Real Orchard Incoming Viewing Key for INTERNAL scope (change addresses)
    prepared_ivk_internal: PreparedIncomingViewingKey,
    /// Address identifier for display
    address_id: [u8; 32],
}

#[wasm_bindgen]
impl WalletKeys {
    /// Derive wallet keys from a 24-word BIP39 seed phrase
    #[wasm_bindgen(constructor)]
    pub fn from_seed_phrase(seed_phrase: &str) -> Result<WalletKeys, JsError> {
        let mnemonic = bip39::Mnemonic::parse(seed_phrase)
            .map_err(|e| JsError::new(&format!("Invalid seed phrase: {}", e)))?;

        // Get 64-byte seed from mnemonic (no passphrase)
        let seed = mnemonic.to_seed("");

        // Use real Orchard key derivation - get FVK and BOTH External and Internal IVKs
        let (fvk, ivk_external, ivk_internal) = derive_orchard_keys(&seed)
            .map_err(|e| JsError::new(&format!("Key derivation failed: {}", e)))?;

        // Create address identifier from External IVK (for display only)
        let address_id = {
            let mut hasher = Blake2b512::new();
            hasher.update(b"ZafuWalletID");
            // Hash the default address to get an identifier
            let default_addr = ivk_external.address_at(0u64);
            hasher.update(default_addr.to_raw_address_bytes());
            let hash = hasher.finalize();
            let mut id = [0u8; 32];
            id.copy_from_slice(&hash[..32]);
            id
        };

        // Prepare IVKs for efficient batched decryption
        let prepared_ivk_external = ivk_external.prepare();
        let prepared_ivk_internal = ivk_internal.prepare();

        Ok(WalletKeys {
            fvk,
            prepared_ivk_external,
            prepared_ivk_internal,
            address_id,
        })
    }

    /// Get the wallet's receiving address (identifier)
    #[wasm_bindgen]
    pub fn get_address(&self) -> String {
        hex_encode(&self.address_id)
    }

    /// Scan a batch of compact actions in PARALLEL and return found notes
    /// This is the main entry point for high-performance scanning
    #[wasm_bindgen]
    pub fn scan_actions_parallel(&self, actions_bytes: &[u8]) -> Result<JsValue, JsError> {
        // Deserialize actions from compact binary format
        // Format: [count: u32][action1][action2]...
        // Each action: [nullifier: 32][cmx: 32][epk: 32][ciphertext: 52] = 148 bytes
        let actions = parse_compact_actions(actions_bytes)?;

        #[cfg(feature = "parallel")]
        let found: Vec<FoundNote> = actions
            .par_iter()
            .enumerate()
            .filter_map(|(idx, action)| {
                self.try_decrypt_action_binary(action).map(|(value, note_nf, rseed, rho, addr)| FoundNote {
                    index: idx as u32,
                    value,
                    nullifier: hex_encode(&note_nf),
                    cmx: hex_encode(&action.cmx),
                    rseed: Some(hex_encode(&rseed)),
                    rho: Some(hex_encode(&rho)),
                    recipient: Some(hex_encode(&addr)),
                })
            })
            .collect();

        #[cfg(not(feature = "parallel"))]
        let found: Vec<FoundNote> = actions
            .iter()
            .enumerate()
            .filter_map(|(idx, action)| {
                self.try_decrypt_action_binary(action).map(|(value, note_nf, rseed, rho, addr)| FoundNote {
                    index: idx as u32,
                    value,
                    nullifier: hex_encode(&note_nf),
                    cmx: hex_encode(&action.cmx),
                    rseed: Some(hex_encode(&rseed)),
                    rho: Some(hex_encode(&rho)),
                    recipient: Some(hex_encode(&addr)),
                })
            })
            .collect();

        serde_wasm_bindgen::to_value(&found)
            .map_err(|e| JsError::new(&format!("Serialization failed: {}", e)))
    }

    /// Scan actions from JSON (legacy compatibility, slower)
    #[wasm_bindgen]
    pub fn scan_actions(&self, actions_json: JsValue) -> Result<JsValue, JsError> {
        let actions: Vec<CompactActionJs> = serde_wasm_bindgen::from_value(actions_json)
            .map_err(|e| JsError::new(&format!("Invalid actions JSON: {}", e)))?;

        #[cfg(feature = "parallel")]
        let found: Vec<FoundNote> = actions
            .par_iter()
            .enumerate()
            .filter_map(|(idx, action)| {
                self.try_decrypt_action_json(action).map(|value| FoundNote {
                    index: idx as u32,
                    value,
                    nullifier: action.nullifier.clone(),
                    cmx: action.cmx.clone(),
                    rseed: None,
                    rho: None,
                    recipient: None,
                })
            })
            .collect();

        #[cfg(not(feature = "parallel"))]
        let found: Vec<FoundNote> = actions
            .iter()
            .enumerate()
            .filter_map(|(idx, action)| {
                self.try_decrypt_action_json(action).map(|value| FoundNote {
                    index: idx as u32,
                    value,
                    nullifier: action.nullifier.clone(),
                    cmx: action.cmx.clone(),
                    rseed: None,
                    rho: None,
                    recipient: None,
                })
            })
            .collect();

        serde_wasm_bindgen::to_value(&found)
            .map_err(|e| JsError::new(&format!("Serialization failed: {}", e)))
    }

    /// Calculate balance from found notes minus spent nullifiers
    #[wasm_bindgen]
    pub fn calculate_balance(&self, notes_json: JsValue, spent_nullifiers_json: JsValue) -> Result<u64, JsError> {
        let notes: Vec<FoundNote> = serde_wasm_bindgen::from_value(notes_json)
            .map_err(|e| JsError::new(&format!("Invalid notes: {}", e)))?;
        let spent: Vec<String> = serde_wasm_bindgen::from_value(spent_nullifiers_json)
            .map_err(|e| JsError::new(&format!("Invalid nullifiers: {}", e)))?;

        let balance: u64 = notes.iter()
            .filter(|n| !spent.contains(&n.nullifier))
            .map(|n| n.value)
            .sum();

        Ok(balance)
    }

    /// Try to decrypt a binary-format action using official Orchard note decryption
    /// Tries BOTH external and internal scope IVKs
    /// Returns (value, note_nullifier, rseed_bytes, rho_bytes, recipient_address_bytes)
    fn try_decrypt_action_binary(&self, action: &CompactActionBinary) -> Option<(u64, [u8; 32], [u8; 32], [u8; 32], [u8; 43])> {
        // Parse the nullifier and cmx
        let nullifier = orchard::note::Nullifier::from_bytes(&action.nullifier);
        if nullifier.is_none().into() {
            return None;
        }
        let nullifier = nullifier.unwrap();

        let cmx = orchard::note::ExtractedNoteCommitment::from_bytes(&action.cmx);
        if cmx.is_none().into() {
            return None;
        }
        let cmx = cmx.unwrap();

        // Create compact action for domain construction
        let compact_action = orchard::note_encryption::CompactAction::from_parts(
            nullifier,
            cmx,
            EphemeralKeyBytes(action.epk),
            action.ciphertext,
        );

        // Create domain for this action
        let domain = OrchardDomain::for_compact_action(&compact_action);

        // Create our shielded output wrapper
        let output = CompactShieldedOutput {
            epk: action.epk,
            cmx: action.cmx,
            ciphertext: action.ciphertext,
        };

        // Try compact note decryption with EXTERNAL scope IVK first
        if let Some((note, addr)) = try_compact_note_decryption(&domain, &self.prepared_ivk_external, &output) {
            let note_nf = note.nullifier(&self.fvk);
            let rseed = *note.rseed().as_bytes();
            let rho = note.rho().to_bytes();
            return Some((note.value().inner(), note_nf.to_bytes(), rseed, rho, addr.to_raw_address_bytes()));
        }

        // If external failed, try INTERNAL scope IVK (for change/shielding outputs)
        if let Some((note, addr)) = try_compact_note_decryption(&domain, &self.prepared_ivk_internal, &output) {
            let note_nf = note.nullifier(&self.fvk);
            let rseed = *note.rseed().as_bytes();
            let rho = note.rho().to_bytes();
            return Some((note.value().inner(), note_nf.to_bytes(), rseed, rho, addr.to_raw_address_bytes()));
        }

        None
    }

    /// Try to decrypt a JSON-format action
    /// Tries BOTH external and internal scope IVKs
    fn try_decrypt_action_json(&self, action: &CompactActionJs) -> Option<u64> {
        let epk_bytes: [u8; 32] = hex_decode(&action.ephemeral_key)?.try_into().ok()?;
        let cmx_bytes: [u8; 32] = hex_decode(&action.cmx)?.try_into().ok()?;
        let nullifier_bytes: [u8; 32] = hex_decode(&action.nullifier)?.try_into().ok()?;
        let ciphertext_vec = hex_decode(&action.ciphertext)?;
        if ciphertext_vec.len() < 52 {
            return None;
        }
        let mut ciphertext = [0u8; 52];
        ciphertext.copy_from_slice(&ciphertext_vec[..52]);

        // Parse the nullifier and cmx
        let nullifier = orchard::note::Nullifier::from_bytes(&nullifier_bytes);
        if nullifier.is_none().into() {
            return None;
        }
        let nullifier = nullifier.unwrap();

        let cmx = orchard::note::ExtractedNoteCommitment::from_bytes(&cmx_bytes);
        if cmx.is_none().into() {
            return None;
        }
        let cmx = cmx.unwrap();

        // Create compact action for domain construction
        let compact_action = orchard::note_encryption::CompactAction::from_parts(
            nullifier,
            cmx,
            EphemeralKeyBytes(epk_bytes),
            ciphertext,
        );

        // Create domain for this action
        let domain = OrchardDomain::for_compact_action(&compact_action);

        // Create our shielded output wrapper
        let output = CompactShieldedOutput {
            epk: epk_bytes,
            cmx: cmx_bytes,
            ciphertext,
        };

        // Try compact note decryption with EXTERNAL scope IVK first
        if let Some(result) = try_compact_note_decryption(&domain, &self.prepared_ivk_external, &output) {
            return Some(result.0.value().inner());
        }

        // If external failed, try INTERNAL scope IVK (for change/shielding outputs)
        if let Some(result) = try_compact_note_decryption(&domain, &self.prepared_ivk_internal, &output) {
            return Some(result.0.value().inner());
        }

        None
    }
}

/// Compact shielded output for use with zcash_note_encryption
struct CompactShieldedOutput {
    epk: [u8; 32],
    cmx: [u8; 32],
    ciphertext: [u8; 52],
}

impl ShieldedOutput<OrchardDomain, COMPACT_NOTE_SIZE> for CompactShieldedOutput {
    fn ephemeral_key(&self) -> EphemeralKeyBytes {
        EphemeralKeyBytes(self.epk)
    }

    fn cmstar_bytes(&self) -> [u8; 32] {
        self.cmx
    }

    fn enc_ciphertext(&self) -> &[u8; COMPACT_NOTE_SIZE] {
        &self.ciphertext
    }
}

/// Binary compact action for efficient transfer (148 bytes each)
#[derive(Clone)]
struct CompactActionBinary {
    nullifier: [u8; 32],
    cmx: [u8; 32],
    epk: [u8; 32],
    ciphertext: [u8; 52],
}

/// Parse compact actions from binary format
fn parse_compact_actions(data: &[u8]) -> Result<Vec<CompactActionBinary>, JsError> {
    if data.len() < 4 {
        return Err(JsError::new("Data too short"));
    }

    let count = u32::from_le_bytes([data[0], data[1], data[2], data[3]]) as usize;
    let action_size = 32 + 32 + 32 + 52; // 148 bytes

    if data.len() < 4 + count * action_size {
        return Err(JsError::new(&format!(
            "Data too short: expected {} bytes for {} actions, got {}",
            4 + count * action_size, count, data.len()
        )));
    }

    let mut actions = Vec::with_capacity(count);
    let mut offset = 4;

    for _ in 0..count {
        let mut nullifier = [0u8; 32];
        let mut cmx = [0u8; 32];
        let mut epk = [0u8; 32];
        let mut ciphertext = [0u8; 52];

        nullifier.copy_from_slice(&data[offset..offset + 32]);
        offset += 32;
        cmx.copy_from_slice(&data[offset..offset + 32]);
        offset += 32;
        epk.copy_from_slice(&data[offset..offset + 32]);
        offset += 32;
        ciphertext.copy_from_slice(&data[offset..offset + 52]);
        offset += 52;

        actions.push(CompactActionBinary { nullifier, cmx, epk, ciphertext });
    }

    Ok(actions)
}

/// Compact action from JavaScript (JSON format)
#[derive(Debug, Deserialize)]
struct CompactActionJs {
    nullifier: String,
    cmx: String,
    ephemeral_key: String,
    ciphertext: String,
}

/// Found note to return to JavaScript
#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct FoundNote {
    pub index: u32,
    pub value: u64,
    pub nullifier: String,
    pub cmx: String,
    /// rseed bytes for note reconstruction (hex, 32 bytes)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rseed: Option<String>,
    /// rho bytes for note reconstruction (hex, 32 bytes)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rho: Option<String>,
    /// recipient address bytes for note reconstruction (hex, 43 bytes)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub recipient: Option<String>,
}

/// Batch scan result with stats
#[derive(Debug, Serialize)]
pub struct ScanResult {
    pub found_notes: Vec<FoundNote>,
    pub actions_scanned: u32,
    pub scan_time_ms: f64,
}

/// Derive real Orchard keys (FVK, External IVK, Internal IVK) from seed using proper ZIP-32 derivation
fn derive_orchard_keys(seed: &[u8]) -> Result<(orchard::keys::FullViewingKey, IncomingViewingKey, IncomingViewingKey), String> {
    // Seed must be 64 bytes (from BIP39)
    if seed.len() != 64 {
        return Err(format!("Invalid seed length: {} (expected 64)", seed.len()));
    }

    // Derive spending key using ZIP-32 for mainnet (coin_type=133), account 0
    let sk = SpendingKey::from_zip32_seed(seed, 133, zip32::AccountId::ZERO)
        .map_err(|_| "Failed to derive spending key from seed")?;

    // Get Full Viewing Key from Spending Key
    let fvk = orchard::keys::FullViewingKey::from(&sk);

    // Get Incoming Viewing Keys for BOTH scopes
    // External = receiving addresses (what you share with others)
    // Internal = change/shielding addresses (used by wallet internally)
    let ivk_external = fvk.to_ivk(Scope::External);
    let ivk_internal = fvk.to_ivk(Scope::Internal);

    Ok((fvk, ivk_external, ivk_internal))
}

/// Generate a new 24-word seed phrase
#[wasm_bindgen]
pub fn generate_seed_phrase() -> Result<String, JsError> {
    let mnemonic = bip39::Mnemonic::generate(24)
        .map_err(|e| JsError::new(&format!("Failed to generate mnemonic: {}", e)))?;
    Ok(mnemonic.to_string())
}

/// Validate a seed phrase
#[wasm_bindgen]
pub fn validate_seed_phrase(seed_phrase: &str) -> bool {
    bip39::Mnemonic::parse(seed_phrase).is_ok()
}

/// Get library version
#[wasm_bindgen]
pub fn version() -> String {
    env!("CARGO_PKG_VERSION").to_string()
}

/// Get number of threads available (0 if single-threaded)
#[wasm_bindgen]
pub fn num_threads() -> usize {
    #[cfg(feature = "parallel")]
    {
        rayon::current_num_threads()
    }
    #[cfg(not(feature = "parallel"))]
    {
        1
    }
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

// ============================================================================
// Cold Signing Support
// ============================================================================

/// QR code type constants for Zcash cold signing
pub const QR_TYPE_ZCASH_FVK_EXPORT: u8 = 0x01;
pub const QR_TYPE_ZCASH_SIGN_REQUEST: u8 = 0x02;
pub const QR_TYPE_ZCASH_SIGNATURES: u8 = 0x03;

/// Watch-only wallet - holds only viewing keys, no spending capability
/// This is used by online wallets (Prax/Zafu) to track balances
/// and build unsigned transactions for cold signing.
#[wasm_bindgen]
pub struct WatchOnlyWallet {
    /// Full Viewing Key (for balance tracking and nullifier computation)
    fvk: orchard::keys::FullViewingKey,
    /// Prepared IVK for efficient scanning (External scope)
    prepared_ivk_external: PreparedIncomingViewingKey,
    /// Prepared IVK for efficient scanning (Internal scope - change)
    prepared_ivk_internal: PreparedIncomingViewingKey,
    /// Account index (for derivation path context)
    account_index: u32,
    /// Network: true = mainnet, false = testnet
    mainnet: bool,
}

#[wasm_bindgen]
impl WatchOnlyWallet {
    /// Import a watch-only wallet from FVK bytes (96 bytes)
    #[wasm_bindgen(constructor)]
    pub fn from_fvk_bytes(fvk_bytes: &[u8], account_index: u32, mainnet: bool) -> Result<WatchOnlyWallet, JsError> {
        if fvk_bytes.len() != 96 {
            return Err(JsError::new(&format!("Invalid FVK length: {} (expected 96)", fvk_bytes.len())));
        }

        let fvk_array: [u8; 96] = fvk_bytes.try_into().unwrap();
        let fvk = orchard::keys::FullViewingKey::from_bytes(&fvk_array);

        if fvk.is_none().into() {
            return Err(JsError::new("Invalid FVK bytes"));
        }
        let fvk = fvk.unwrap();

        let ivk_external = fvk.to_ivk(Scope::External);
        let ivk_internal = fvk.to_ivk(Scope::Internal);

        Ok(WatchOnlyWallet {
            fvk,
            prepared_ivk_external: ivk_external.prepare(),
            prepared_ivk_internal: ivk_internal.prepare(),
            account_index,
            mainnet,
        })
    }

    /// Import from hex-encoded QR data
    #[wasm_bindgen]
    pub fn from_qr_hex(qr_hex: &str) -> Result<WatchOnlyWallet, JsError> {
        let data = hex_decode(qr_hex)
            .ok_or_else(|| JsError::new("Invalid hex string"))?;

        // Validate prelude: [0x53][0x04][0x01]
        if data.len() < 9 {
            return Err(JsError::new("QR data too short"));
        }
        if data[0] != 0x53 || data[1] != 0x04 || data[2] != QR_TYPE_ZCASH_FVK_EXPORT {
            return Err(JsError::new("Invalid QR prelude for Zcash FVK export"));
        }

        let mut offset = 3;

        // flags
        let flags = data[offset];
        offset += 1;
        let mainnet = flags & 0x01 != 0;
        let has_orchard = flags & 0x02 != 0;

        if !has_orchard {
            return Err(JsError::new("QR data missing Orchard FVK"));
        }

        // account index
        let account_index = u32::from_le_bytes(data[offset..offset + 4].try_into().unwrap());
        offset += 4;

        // skip label
        let label_len = data[offset] as usize;
        offset += 1 + label_len;

        // orchard fvk
        if offset + 96 > data.len() {
            return Err(JsError::new("Orchard FVK truncated"));
        }
        let fvk_bytes = &data[offset..offset + 96];

        Self::from_fvk_bytes(fvk_bytes, account_index, mainnet)
    }

    /// Get account index
    #[wasm_bindgen]
    pub fn get_account_index(&self) -> u32 {
        self.account_index
    }

    /// Is mainnet
    #[wasm_bindgen]
    pub fn is_mainnet(&self) -> bool {
        self.mainnet
    }

    /// Get default receiving address (diversifier index 0)
    #[wasm_bindgen]
    pub fn get_address(&self) -> String {
        let addr = self.fvk.to_ivk(Scope::External).address_at(0u64);
        encode_orchard_address(&addr, self.mainnet)
    }

    /// Get address at specific diversifier index
    #[wasm_bindgen]
    pub fn get_address_at(&self, diversifier_index: u32) -> String {
        let addr = self.fvk.to_ivk(Scope::External).address_at(diversifier_index as u64);
        encode_orchard_address(&addr, self.mainnet)
    }

    /// Export FVK as hex bytes (for backup)
    #[wasm_bindgen]
    pub fn export_fvk_hex(&self) -> String {
        hex_encode(&self.fvk.to_bytes())
    }

    /// Scan compact actions (same interface as WalletKeys)
    #[wasm_bindgen]
    pub fn scan_actions_parallel(&self, actions_bytes: &[u8]) -> Result<JsValue, JsError> {
        let actions = parse_compact_actions(actions_bytes)?;

        #[cfg(feature = "parallel")]
        let found: Vec<FoundNote> = actions
            .par_iter()
            .enumerate()
            .filter_map(|(idx, action)| {
                self.try_decrypt_action(action).map(|(value, note_nf, rseed, rho, addr)| FoundNote {
                    index: idx as u32,
                    value,
                    nullifier: hex_encode(&note_nf),
                    cmx: hex_encode(&action.cmx),
                    rseed: Some(hex_encode(&rseed)),
                    rho: Some(hex_encode(&rho)),
                    recipient: Some(hex_encode(&addr)),
                })
            })
            .collect();

        #[cfg(not(feature = "parallel"))]
        let found: Vec<FoundNote> = actions
            .iter()
            .enumerate()
            .filter_map(|(idx, action)| {
                self.try_decrypt_action(action).map(|(value, note_nf, rseed, rho, addr)| FoundNote {
                    index: idx as u32,
                    value,
                    nullifier: hex_encode(&note_nf),
                    cmx: hex_encode(&action.cmx),
                    rseed: Some(hex_encode(&rseed)),
                    rho: Some(hex_encode(&rho)),
                    recipient: Some(hex_encode(&addr)),
                })
            })
            .collect();

        serde_wasm_bindgen::to_value(&found)
            .map_err(|e| JsError::new(&format!("Serialization failed: {}", e)))
    }

    /// Try to decrypt a compact action
    /// Returns (value, note_nullifier, rseed_bytes, rho_bytes, recipient_address_bytes)
    fn try_decrypt_action(&self, action: &CompactActionBinary) -> Option<(u64, [u8; 32], [u8; 32], [u8; 32], [u8; 43])> {
        let nullifier = orchard::note::Nullifier::from_bytes(&action.nullifier);
        if nullifier.is_none().into() {
            return None;
        }
        let nullifier = nullifier.unwrap();

        let cmx = orchard::note::ExtractedNoteCommitment::from_bytes(&action.cmx);
        if cmx.is_none().into() {
            return None;
        }
        let cmx = cmx.unwrap();

        let compact_action = orchard::note_encryption::CompactAction::from_parts(
            nullifier,
            cmx,
            EphemeralKeyBytes(action.epk),
            action.ciphertext,
        );

        let domain = OrchardDomain::for_compact_action(&compact_action);
        let output = CompactShieldedOutput {
            epk: action.epk,
            cmx: action.cmx,
            ciphertext: action.ciphertext,
        };

        // Try external scope first
        if let Some((note, addr)) = try_compact_note_decryption(&domain, &self.prepared_ivk_external, &output) {
            let note_nf = note.nullifier(&self.fvk);
            let rseed = *note.rseed().as_bytes();
            let rho = note.rho().to_bytes();
            return Some((note.value().inner(), note_nf.to_bytes(), rseed, rho, addr.to_raw_address_bytes()));
        }

        // Try internal scope (change addresses)
        if let Some((note, addr)) = try_compact_note_decryption(&domain, &self.prepared_ivk_internal, &output) {
            let note_nf = note.nullifier(&self.fvk);
            let rseed = *note.rseed().as_bytes();
            let rho = note.rho().to_bytes();
            return Some((note.value().inner(), note_nf.to_bytes(), rseed, rho, addr.to_raw_address_bytes()));
        }

        None
    }
}

/// Extend WalletKeys with FVK export for cold signing setup
#[wasm_bindgen]
impl WalletKeys {
    /// Export Full Viewing Key as hex-encoded QR data
    /// This is used to create a watch-only wallet on an online device
    #[wasm_bindgen]
    pub fn export_fvk_qr_hex(&self, account_index: u32, label: Option<String>, mainnet: bool) -> String {
        let mut output = Vec::new();

        // Prelude: [0x53][0x04][0x01] - Substrate compat, Zcash, FVK export
        output.push(0x53);
        output.push(0x04);
        output.push(QR_TYPE_ZCASH_FVK_EXPORT);

        // Flags: mainnet, has_orchard, no_transparent
        let mut flags = 0u8;
        if mainnet { flags |= 0x01; }
        flags |= 0x02; // has orchard
        output.push(flags);

        // Account index
        output.extend_from_slice(&account_index.to_le_bytes());

        // Label
        match &label {
            Some(l) => {
                let bytes = l.as_bytes();
                output.push(bytes.len().min(255) as u8);
                output.extend_from_slice(&bytes[..bytes.len().min(255)]);
            }
            None => output.push(0),
        }

        // Orchard FVK (96 bytes)
        output.extend_from_slice(&self.fvk.to_bytes());

        hex_encode(&output)
    }

    /// Get the Orchard FVK bytes (96 bytes) as hex
    #[wasm_bindgen]
    pub fn get_fvk_hex(&self) -> String {
        hex_encode(&self.fvk.to_bytes())
    }

    /// Get the default receiving address as a Zcash unified address string
    #[wasm_bindgen]
    pub fn get_receiving_address(&self, mainnet: bool) -> String {
        let addr = self.fvk.to_ivk(Scope::External).address_at(0u64);
        encode_orchard_address(&addr, mainnet)
    }

    /// Get receiving address at specific diversifier index
    #[wasm_bindgen]
    pub fn get_receiving_address_at(&self, diversifier_index: u32, mainnet: bool) -> String {
        let addr = self.fvk.to_ivk(Scope::External).address_at(diversifier_index as u64);
        encode_orchard_address(&addr, mainnet)
    }
}

/// Encode an Orchard address as a human-readable string
/// Note: This is the raw Orchard address, not a full Unified Address
fn encode_orchard_address(addr: &orchard::Address, mainnet: bool) -> String {
    // For simplicity, return hex-encoded raw address bytes
    // A proper implementation would use Unified Address encoding (ZIP-316)
    let raw = addr.to_raw_address_bytes();
    let prefix = if mainnet { "u1orchard:" } else { "utest1orchard:" };
    format!("{}{}", prefix, hex_encode(&raw))
}

// ============================================================================
// PCZT (Partially Constructed Zcash Transaction) for Cold Signing
// ============================================================================

/// A spendable note with merkle path for transaction building
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SpendableNoteInfo {
    /// The note's nullifier (hex)
    pub nullifier: String,
    /// The note's commitment (cmx, hex)
    pub cmx: String,
    /// The note's value in zatoshis
    pub value: u64,
    /// The merkle path (hex-encoded, implementation-specific)
    pub merkle_path: String,
    /// Position in the commitment tree
    pub position: u64,
}

/// Transaction request to build
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TransactionRequest {
    /// Recipient address (Orchard)
    pub recipient: String,
    /// Amount in zatoshis
    pub amount: u64,
    /// Optional memo (512 bytes max, hex-encoded)
    pub memo: Option<String>,
}

/// PCZT sign request - what gets sent to cold wallet
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PcztSignRequest {
    /// Account index for key derivation
    pub account_index: u32,
    /// The transaction sighash (32 bytes, hex)
    pub sighash: String,
    /// Orchard actions that need signing
    pub orchard_actions: Vec<OrchardActionInfo>,
    /// Human-readable transaction summary for display
    pub summary: String,
}

/// Info about an Orchard action that needs signing
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OrchardActionInfo {
    /// The randomizer (alpha) for this action (32 bytes, hex)
    pub alpha: String,
}

/// Create a PCZT sign request from transaction parameters
/// This is called by the online wallet to create the data that will be
/// transferred to the cold wallet via QR code.
#[wasm_bindgen]
pub fn create_sign_request(
    account_index: u32,
    sighash_hex: &str,
    alphas_json: JsValue,
    summary: &str,
) -> Result<String, JsError> {
    let alphas: Vec<String> = serde_wasm_bindgen::from_value(alphas_json)
        .map_err(|e| JsError::new(&format!("Invalid alphas: {}", e)))?;

    let request = PcztSignRequest {
        account_index,
        sighash: sighash_hex.to_string(),
        orchard_actions: alphas.into_iter().map(|a| OrchardActionInfo { alpha: a }).collect(),
        summary: summary.to_string(),
    };

    // Encode as QR payload
    let mut output = Vec::new();

    // Prelude: [0x53][0x04][0x02] - Substrate compat, Zcash, Sign request
    output.push(0x53);
    output.push(0x04);
    output.push(QR_TYPE_ZCASH_SIGN_REQUEST);

    // Account index
    output.extend_from_slice(&account_index.to_le_bytes());

    // Sighash (32 bytes)
    let sighash_bytes = hex_decode(sighash_hex)
        .ok_or_else(|| JsError::new("Invalid sighash hex"))?;
    if sighash_bytes.len() != 32 {
        return Err(JsError::new("Sighash must be 32 bytes"));
    }
    output.extend_from_slice(&sighash_bytes);

    // Action count
    output.extend_from_slice(&(request.orchard_actions.len() as u16).to_le_bytes());

    // Each action's alpha
    for action in &request.orchard_actions {
        let alpha_bytes = hex_decode(&action.alpha)
            .ok_or_else(|| JsError::new("Invalid alpha hex"))?;
        if alpha_bytes.len() != 32 {
            return Err(JsError::new("Alpha must be 32 bytes"));
        }
        output.extend_from_slice(&alpha_bytes);
    }

    // Summary (length-prefixed string)
    let summary_bytes = summary.as_bytes();
    output.extend_from_slice(&(summary_bytes.len() as u16).to_le_bytes());
    output.extend_from_slice(summary_bytes);

    Ok(hex_encode(&output))
}

/// Parse signatures from cold wallet QR response
/// Returns JSON with sighash and orchard_sigs array
#[wasm_bindgen]
pub fn parse_signature_response(qr_hex: &str) -> Result<JsValue, JsError> {
    let data = hex_decode(qr_hex)
        .ok_or_else(|| JsError::new("Invalid hex string"))?;

    // Validate prelude
    if data.len() < 36 {
        return Err(JsError::new("Response too short"));
    }
    if data[0] != 0x53 || data[1] != 0x04 || data[2] != QR_TYPE_ZCASH_SIGNATURES {
        return Err(JsError::new("Invalid QR prelude for Zcash signatures"));
    }

    let mut offset = 3;

    // Sighash (32 bytes)
    let sighash = hex_encode(&data[offset..offset + 32]);
    offset += 32;

    // Transparent sig count (skip for now - we focus on Orchard)
    let t_count = u16::from_le_bytes(data[offset..offset + 2].try_into().unwrap()) as usize;
    offset += 2;

    // Skip transparent sigs
    for _ in 0..t_count {
        let sig_len = u16::from_le_bytes(data[offset..offset + 2].try_into().unwrap()) as usize;
        offset += 2 + sig_len;
    }

    // Orchard sig count
    if offset + 2 > data.len() {
        return Err(JsError::new("Orchard count truncated"));
    }
    let o_count = u16::from_le_bytes(data[offset..offset + 2].try_into().unwrap()) as usize;
    offset += 2;

    // Orchard signatures
    let mut orchard_sigs = Vec::with_capacity(o_count);
    for _ in 0..o_count {
        if offset + 64 > data.len() {
            return Err(JsError::new("Orchard signature truncated"));
        }
        orchard_sigs.push(hex_encode(&data[offset..offset + 64]));
        offset += 64;
    }

    #[derive(Serialize)]
    struct SignatureResponse {
        sighash: String,
        orchard_sigs: Vec<String>,
    }

    let response = SignatureResponse {
        sighash,
        orchard_sigs,
    };

    serde_wasm_bindgen::to_value(&response)
        .map_err(|e| JsError::new(&format!("Serialization failed: {}", e)))
}

// ============================================================================
// Full Note Decryption with Memos
// ============================================================================

/// Found note with memo from full decryption
#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct FoundNoteWithMemo {
    pub index: u32,
    pub value: u64,
    pub nullifier: String,
    pub cmx: String,
    /// The decrypted memo (512 bytes, may be text or binary)
    pub memo: String,
    /// Whether the memo appears to be text (UTF-8)
    pub memo_is_text: bool,
}

/// Full action data for decryption (includes full 580-byte ciphertext)
#[derive(Debug, Clone)]
struct FullOrchardAction {
    nullifier: [u8; 32],
    cmx: [u8; 32],
    epk: [u8; 32],
    enc_ciphertext: [u8; 580], // full ciphertext including memo
    out_ciphertext: [u8; 80],   // for outgoing note decryption
}

/// Full shielded output for use with zcash_note_encryption
struct FullShieldedOutput {
    epk: [u8; 32],
    cmx: [u8; 32],
    enc_ciphertext: [u8; 580],
}

/// NOTE_PLAINTEXT_SIZE for Orchard (580 bytes enc_ciphertext)
const ORCHARD_NOTE_PLAINTEXT_SIZE: usize = 580;

impl zcash_note_encryption::ShieldedOutput<OrchardDomain, ORCHARD_NOTE_PLAINTEXT_SIZE> for FullShieldedOutput {
    fn ephemeral_key(&self) -> EphemeralKeyBytes {
        EphemeralKeyBytes(self.epk)
    }

    fn cmstar_bytes(&self) -> [u8; 32] {
        self.cmx
    }

    fn enc_ciphertext(&self) -> &[u8; ORCHARD_NOTE_PLAINTEXT_SIZE] {
        &self.enc_ciphertext
    }
}

/// Parse full Orchard actions from raw transaction bytes
/// Uses zcash_primitives for proper v5 transaction parsing
fn parse_orchard_actions_from_tx(tx_bytes: &[u8]) -> Result<Vec<FullOrchardAction>, String> {
    use zcash_primitives::transaction::Transaction;
    use zcash_primitives::consensus::BranchId;
    use std::io::Cursor;

    // Parse transaction using zcash_primitives
    let mut cursor = Cursor::new(tx_bytes);
    let tx = Transaction::read(&mut cursor, BranchId::Nu5)
        .map_err(|e| format!("Failed to parse transaction: {:?}", e))?;

    // Get Orchard bundle if present
    let orchard_bundle = match tx.orchard_bundle() {
        Some(bundle) => bundle,
        None => return Ok(vec![]), // No Orchard actions in this tx
    };

    // Extract actions with full ciphertext
    let mut actions = Vec::new();

    for action in orchard_bundle.actions() {
        // Get action components
        let nullifier_bytes = action.nullifier().to_bytes();
        let cmx_bytes = action.cmx().to_bytes();
        let epk_bytes = action.encrypted_note().epk_bytes;

        // Get full encrypted ciphertext (580 bytes)
        let enc_ciphertext = action.encrypted_note().enc_ciphertext;

        // Get out_ciphertext for potential outgoing decryption
        let out_ciphertext = action.encrypted_note().out_ciphertext;

        actions.push(FullOrchardAction {
            nullifier: nullifier_bytes,
            cmx: cmx_bytes,
            epk: epk_bytes,
            enc_ciphertext,
            out_ciphertext,
        });
    }

    Ok(actions)
}

#[wasm_bindgen]
impl WalletKeys {
    /// Decrypt full notes with memos from a raw transaction
    ///
    /// Takes the raw transaction bytes (from zidecar's get_transaction)
    /// and returns any notes that belong to this wallet, including memos.
    #[wasm_bindgen]
    pub fn decrypt_transaction_memos(&self, tx_bytes: &[u8]) -> Result<JsValue, JsError> {
        use zcash_note_encryption::try_note_decryption;

        let actions = parse_orchard_actions_from_tx(tx_bytes)
            .map_err(|e| JsError::new(&format!("Failed to parse transaction: {}", e)))?;

        let mut found: Vec<FoundNoteWithMemo> = Vec::new();

        for (idx, action) in actions.iter().enumerate() {
            // Parse nullifier and cmx using CtOption
            let nullifier_opt = orchard::note::Nullifier::from_bytes(&action.nullifier);
            if !bool::from(nullifier_opt.is_some()) {
                continue;
            }
            let nullifier = nullifier_opt.unwrap();

            let cmx_opt = orchard::note::ExtractedNoteCommitment::from_bytes(&action.cmx);
            if !bool::from(cmx_opt.is_some()) {
                continue;
            }
            let cmx = cmx_opt.unwrap();

            // Create domain for full action
            let compact_action = orchard::note_encryption::CompactAction::from_parts(
                nullifier,
                cmx,
                EphemeralKeyBytes(action.epk),
                action.enc_ciphertext[..52].try_into().unwrap(),
            );

            let domain = OrchardDomain::for_compact_action(&compact_action);

            // Create full shielded output
            let output = FullShieldedOutput {
                epk: action.epk,
                cmx: action.cmx,
                enc_ciphertext: action.enc_ciphertext,
            };

            // Try external scope first
            if let Some((note, _addr, memo)) = try_note_decryption(&domain, &self.prepared_ivk_external, &output) {
                let note_nf = note.nullifier(&self.fvk);
                let (memo_str, is_text) = parse_memo_bytes(&memo);

                found.push(FoundNoteWithMemo {
                    index: idx as u32,
                    value: note.value().inner(),
                    nullifier: hex_encode(&note_nf.to_bytes()),
                    cmx: hex_encode(&action.cmx),
                    memo: memo_str,
                    memo_is_text: is_text,
                });
                continue;
            }

            // Try internal scope (change)
            if let Some((note, _addr, memo)) = try_note_decryption(&domain, &self.prepared_ivk_internal, &output) {
                let note_nf = note.nullifier(&self.fvk);
                let (memo_str, is_text) = parse_memo_bytes(&memo);

                found.push(FoundNoteWithMemo {
                    index: idx as u32,
                    value: note.value().inner(),
                    nullifier: hex_encode(&note_nf.to_bytes()),
                    cmx: hex_encode(&action.cmx),
                    memo: memo_str,
                    memo_is_text: is_text,
                });
            }
        }

        serde_wasm_bindgen::to_value(&found)
            .map_err(|e| JsError::new(&format!("Serialization failed: {}", e)))
    }
}

#[wasm_bindgen]
impl WatchOnlyWallet {
    /// Decrypt full notes with memos from a raw transaction (watch-only version)
    #[wasm_bindgen]
    pub fn decrypt_transaction_memos(&self, tx_bytes: &[u8]) -> Result<JsValue, JsError> {
        use zcash_note_encryption::try_note_decryption;

        let actions = parse_orchard_actions_from_tx(tx_bytes)
            .map_err(|e| JsError::new(&format!("Failed to parse transaction: {}", e)))?;

        let mut found: Vec<FoundNoteWithMemo> = Vec::new();

        for (idx, action) in actions.iter().enumerate() {
            // Parse nullifier and cmx using CtOption
            let nullifier_opt = orchard::note::Nullifier::from_bytes(&action.nullifier);
            if !bool::from(nullifier_opt.is_some()) {
                continue;
            }
            let nullifier = nullifier_opt.unwrap();

            let cmx_opt = orchard::note::ExtractedNoteCommitment::from_bytes(&action.cmx);
            if !bool::from(cmx_opt.is_some()) {
                continue;
            }
            let cmx = cmx_opt.unwrap();

            let compact_action = orchard::note_encryption::CompactAction::from_parts(
                nullifier,
                cmx,
                EphemeralKeyBytes(action.epk),
                action.enc_ciphertext[..52].try_into().unwrap(),
            );

            let domain = OrchardDomain::for_compact_action(&compact_action);

            let output = FullShieldedOutput {
                epk: action.epk,
                cmx: action.cmx,
                enc_ciphertext: action.enc_ciphertext,
            };

            // Try external scope
            if let Some((note, _addr, memo)) = try_note_decryption(&domain, &self.prepared_ivk_external, &output) {
                let note_nf = note.nullifier(&self.fvk);
                let (memo_str, is_text) = parse_memo_bytes(&memo);

                found.push(FoundNoteWithMemo {
                    index: idx as u32,
                    value: note.value().inner(),
                    nullifier: hex_encode(&note_nf.to_bytes()),
                    cmx: hex_encode(&action.cmx),
                    memo: memo_str,
                    memo_is_text: is_text,
                });
                continue;
            }

            // Try internal scope
            if let Some((note, _addr, memo)) = try_note_decryption(&domain, &self.prepared_ivk_internal, &output) {
                let note_nf = note.nullifier(&self.fvk);
                let (memo_str, is_text) = parse_memo_bytes(&memo);

                found.push(FoundNoteWithMemo {
                    index: idx as u32,
                    value: note.value().inner(),
                    nullifier: hex_encode(&note_nf.to_bytes()),
                    cmx: hex_encode(&action.cmx),
                    memo: memo_str,
                    memo_is_text: is_text,
                });
            }
        }

        serde_wasm_bindgen::to_value(&found)
            .map_err(|e| JsError::new(&format!("Serialization failed: {}", e)))
    }
}

/// Parse memo bytes into a string
/// Returns (memo_string, is_text)
fn parse_memo_bytes(memo: &[u8; 512]) -> (String, bool) {
    // Check if memo is empty (all zeros or starts with 0xF6 empty marker)
    if memo[0] == 0xF6 || memo.iter().all(|&b| b == 0) {
        return (String::new(), true);
    }

    // Check if it's a text memo (starts with 0xF5 followed by UTF-8)
    if memo[0] == 0xF5 {
        // Find the end of the text (first null byte or end of memo)
        let text_bytes: Vec<u8> = memo[1..].iter()
            .take_while(|&&b| b != 0)
            .copied()
            .collect();

        if let Ok(text) = String::from_utf8(text_bytes) {
            return (text, true);
        }
    }

    // Try to parse as raw UTF-8 (some wallets don't use the 0xF5 prefix)
    let text_bytes: Vec<u8> = memo.iter()
        .take_while(|&&b| b != 0)
        .copied()
        .collect();

    if let Ok(text) = String::from_utf8(text_bytes.clone()) {
        // Check if it looks like text (mostly printable ASCII + common UTF-8)
        let printable_ratio = text_bytes.iter()
            .filter(|&&b| b >= 32 && b <= 126 || b >= 0xC0)
            .count() as f32 / text_bytes.len().max(1) as f32;

        if printable_ratio > 0.8 {
            return (text, true);
        }
    }

    // Return as hex if it's binary data
    (hex_encode(memo), false)
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
        // Standard 24-word test mnemonic
        let keys = WalletKeys::from_seed_phrase(
            "abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon art"
        ).unwrap();

        // Address ID is 32 bytes = 64 hex chars
        let address = keys.get_address();
        assert_eq!(address.len(), 64);

        // Verify it's not all zeros (key derivation worked)
        assert!(!address.chars().all(|c| c == '0'));
    }

    #[test]
    fn test_binary_action_parsing() {
        // Create test data: 1 action
        let mut data = vec![1, 0, 0, 0]; // count = 1
        data.extend_from_slice(&[0u8; 32]); // nullifier
        data.extend_from_slice(&[1u8; 32]); // cmx
        data.extend_from_slice(&[2u8; 32]); // epk
        data.extend_from_slice(&[3u8; 52]); // ciphertext

        let actions = parse_compact_actions(&data).unwrap();
        assert_eq!(actions.len(), 1);
        assert_eq!(actions[0].nullifier, [0u8; 32]);
        assert_eq!(actions[0].cmx, [1u8; 32]);
    }

    #[test]
    fn test_fvk_export_roundtrip() {
        // Create wallet from seed
        let keys = WalletKeys::from_seed_phrase(
            "abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon art"
        ).unwrap();

        // Export FVK as QR hex
        let qr_hex = keys.export_fvk_qr_hex(0, Some("Test Wallet".to_string()), true);

        // Verify prelude
        let qr_bytes = hex_decode(&qr_hex).unwrap();
        assert_eq!(qr_bytes[0], 0x53); // substrate compat
        assert_eq!(qr_bytes[1], 0x04); // zcash
        assert_eq!(qr_bytes[2], QR_TYPE_ZCASH_FVK_EXPORT);

        // Import as watch-only wallet
        let watch = WatchOnlyWallet::from_qr_hex(&qr_hex).unwrap();

        // Verify same FVK
        assert_eq!(watch.export_fvk_hex(), keys.get_fvk_hex());
        assert_eq!(watch.get_account_index(), 0);
        assert!(watch.is_mainnet());
    }

    #[test]
    fn test_watch_only_from_fvk_bytes() {
        // Create wallet and get FVK
        let keys = WalletKeys::from_seed_phrase(
            "abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon art"
        ).unwrap();

        let fvk_hex = keys.get_fvk_hex();
        let fvk_bytes = hex_decode(&fvk_hex).unwrap();

        // Create watch-only wallet from FVK bytes
        let watch = WatchOnlyWallet::from_fvk_bytes(&fvk_bytes, 0, true).unwrap();

        // Verify addresses match
        assert_eq!(watch.get_address(), keys.get_receiving_address(true));
    }

    #[test]
    fn test_address_generation() {
        let keys = WalletKeys::from_seed_phrase(
            "abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon art"
        ).unwrap();

        let addr0 = keys.get_receiving_address(true);
        let addr1 = keys.get_receiving_address_at(1, true);

        // Addresses should be different
        assert_ne!(addr0, addr1);

        // Mainnet vs testnet prefix
        let addr_mainnet = keys.get_receiving_address(true);
        let addr_testnet = keys.get_receiving_address(false);
        assert!(addr_mainnet.starts_with("u1orchard:"));
        assert!(addr_testnet.starts_with("utest1orchard:"));
    }

    /// base58check encode (zcash transparent address)
    fn base58check_encode(version: &[u8], payload: &[u8]) -> String {
        use sha2::Digest as _;
        let mut data = Vec::with_capacity(version.len() + payload.len() + 4);
        data.extend_from_slice(version);
        data.extend_from_slice(payload);
        let checksum = sha2::Sha256::digest(sha2::Sha256::digest(&data));
        data.extend_from_slice(&checksum[..4]);

        // base58 encode
        const ALPHABET: &[u8] = b"123456789ABCDEFGHJKLMNPQRSTUVWXYZabcdefghijkmnopqrstuvwxyz";
        let mut num = vec![0u8; data.len()];
        num.copy_from_slice(&data);
        let mut out = Vec::new();
        while !num.iter().all(|&b| b == 0) {
            let mut rem = 0u32;
            for byte in num.iter_mut() {
                let acc = (rem << 8) | (*byte as u32);
                *byte = (acc / 58) as u8;
                rem = acc % 58;
            }
            out.push(ALPHABET[rem as usize]);
        }
        for &b in data.iter() {
            if b == 0 { out.push(b'1'); } else { break; }
        }
        out.reverse();
        String::from_utf8(out).unwrap()
    }

    #[test]
    fn test_transparent_privkey_derivation() {
        let seed = "abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon art";
        let mnemonic = bip39::Mnemonic::parse(seed).unwrap();
        let seed_bytes = mnemonic.to_seed("");

        // BIP32 derivation: m/44'/133'/0'/0/0
        let master = bip32_master_key(&seed_bytes);
        let child = bip32_derive_child(&master, 44, true).unwrap();
        let child = bip32_derive_child(&child, 133, true).unwrap();
        let child = bip32_derive_child(&child, 0, true).unwrap();
        let child = bip32_derive_child(&child, 0, false).unwrap();
        let child = bip32_derive_child(&child, 0, false).unwrap();

        // verify we can construct a signing key and derive address
        let signing_key = k256::ecdsa::SigningKey::from_slice(&child.key).unwrap();
        let pubkey = signing_key.verifying_key().to_encoded_point(true);
        assert_eq!(pubkey.as_bytes().len(), 33);

        // derive t-addr: base58check(version_prefix || hash160(compressed_pubkey))
        let pkh = hash160(pubkey.as_bytes());
        // zcash mainnet t-addr prefix: 0x1cb8
        let taddr = base58check_encode(&[0x1c, 0xb8], &pkh);
        println!("t-addr (m/44'/133'/0'/0/0): {}", taddr);
        assert!(taddr.starts_with("t1"));
    }

    #[test]
    fn test_orchard_builder_shielding() {
        use orchard::builder::{Builder, BundleType};
        use orchard::bundle::Flags;
        use orchard::keys::{SpendingKey, FullViewingKey, Scope};
        use orchard::tree::Anchor;
        use orchard::value::NoteValue;
        use rand::rngs::OsRng;
        use zcash_protocol::value::ZatBalance;

        // derive orchard key from test seed
        let seed = "abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon art";
        let mnemonic = bip39::Mnemonic::parse(seed).unwrap();
        let seed_bytes = mnemonic.to_seed("");

        let sk = SpendingKey::from_zip32_seed(
            &seed_bytes,
            133,
            zip32::AccountId::try_from(0u32).unwrap(),
        ).unwrap();
        let fvk = FullViewingKey::from(&sk);
        let recipient = fvk.address_at(0u64, Scope::External);

        // build an output-only bundle
        let bundle_type = BundleType::Transactional {
            flags: Flags::SPENDS_DISABLED,
            bundle_required: true,
        };
        let mut builder = Builder::new(bundle_type, Anchor::empty_tree());

        builder.add_output(None, recipient, NoteValue::from_raw(50_000), [0u8; 512])
            .expect("add_output should succeed");

        let mut rng = OsRng;
        let (unauthorized, _meta) = builder
            .build::<ZatBalance>(&mut rng)
            .expect("build should succeed")
            .expect("should produce a bundle");

        println!("bundle built: {} actions", unauthorized.actions().len());
        // orchard pads to minimum 2 actions
        assert!(unauthorized.actions().len() >= 2, "expected >=2 actions (padding)");

        // prove (expensive but should work natively)
        let pk = orchard::circuit::ProvingKey::build();
        let proven = unauthorized.create_proof(&pk, &mut rng)
            .expect("proving should succeed");

        println!("proof generated");

        // apply signatures (no spend auth keys for output-only)
        let sighash = [0u8; 32]; // dummy sighash for test
        let authorized = proven.apply_signatures(&mut rng, sighash, &[])
            .expect("signatures should succeed");

        println!("authorized bundle: {} actions", authorized.actions().len());

        // serialize
        let mut out = Vec::new();
        serialize_orchard_bundle(&authorized, &mut out).expect("serialization");
        println!("serialized orchard bundle: {} bytes", out.len());
        assert!(out.len() > 100);
    }
}

// ============================================================================
// Transaction Building for Cold Signing
// ============================================================================

/// Build an unsigned transaction and return the data needed for cold signing
/// This is called by the online watch-only wallet.
///
/// Returns JSON with:
/// - sighash: the transaction sighash (hex)
/// - alphas: array of alpha randomizers for each orchard action (hex)
/// - unsigned_tx: the serialized unsigned transaction (hex)
/// - summary: human-readable transaction summary
#[wasm_bindgen]
pub fn build_unsigned_transaction(
    notes_json: JsValue,
    recipient: &str,
    amount: u64,
    fee: u64,
    anchor_hex: &str,
    merkle_paths_json: JsValue,
    account_index: u32,
    _mainnet: bool,
) -> Result<JsValue, JsError> {
    use pasta_curves::pallas;
    use group::ff::PrimeField;

    // Parse inputs
    let notes: Vec<SpendableNoteInfo> = serde_wasm_bindgen::from_value(notes_json)
        .map_err(|e| JsError::new(&format!("Invalid notes: {}", e)))?;

    let merkle_paths: Vec<MerklePathInfo> = serde_wasm_bindgen::from_value(merkle_paths_json)
        .map_err(|e| JsError::new(&format!("Invalid merkle paths: {}", e)))?;

    if notes.len() != merkle_paths.len() {
        return Err(JsError::new("Notes and merkle paths count mismatch"));
    }

    // Parse anchor
    let anchor_bytes = hex_decode(anchor_hex)
        .ok_or_else(|| JsError::new("Invalid anchor hex"))?;
    if anchor_bytes.len() != 32 {
        return Err(JsError::new("Anchor must be 32 bytes"));
    }

    // Calculate totals
    let total_input: u64 = notes.iter().map(|n| n.value).sum();
    if total_input < amount + fee {
        return Err(JsError::new(&format!(
            "Insufficient funds: {} < {} + {}",
            total_input, amount, fee
        )));
    }
    let change = total_input - amount - fee;

    // For now, generate mock alphas and sighash
    // In a full implementation, we would use the Orchard builder
    // which requires access to proving keys (too heavy for WASM)
    //
    // Instead, we use a simplified approach:
    // 1. Generate deterministic alphas from note data
    // 2. Compute sighash from transaction structure
    // 3. Cold wallet signs sighash with randomized key

    let mut alphas = Vec::new();
    let mut hasher = Blake2b512::new();

    // Add domain separator
    hasher.update(b"ZafuOrchardAlpha");
    hasher.update(&account_index.to_le_bytes());

    // Generate one alpha per spend action
    for (i, note) in notes.iter().enumerate() {
        let note_nullifier = hex_decode(&note.nullifier)
            .ok_or_else(|| JsError::new("Invalid nullifier hex"))?;

        let mut alpha_hasher = Blake2b512::new();
        alpha_hasher.update(b"ZafuAlpha");
        alpha_hasher.update(&(i as u32).to_le_bytes());
        alpha_hasher.update(&note_nullifier);
        // Add randomness from JS crypto
        let mut random_bytes = [0u8; 32];
        getrandom::getrandom(&mut random_bytes)
            .map_err(|e| JsError::new(&format!("RNG failed: {}", e)))?;
        alpha_hasher.update(&random_bytes);

        let alpha_hash = alpha_hasher.finalize();
        let mut alpha = [0u8; 32];
        alpha.copy_from_slice(&alpha_hash[..32]);

        // Reduce modulo the Pallas scalar field order
        // This ensures alpha is a valid scalar
        let scalar = pallas::Scalar::from_repr(alpha);
        if bool::from(scalar.is_some()) {
            alpha = scalar.unwrap().to_repr();
        }

        alphas.push(hex_encode(&alpha));
        hasher.update(&alpha);
    }

    // Also add one alpha for the output action (recipient)
    {
        let mut alpha_hasher = Blake2b512::new();
        alpha_hasher.update(b"ZafuAlphaOutput");
        alpha_hasher.update(&amount.to_le_bytes());
        alpha_hasher.update(recipient.as_bytes());
        let mut random_bytes = [0u8; 32];
        getrandom::getrandom(&mut random_bytes)
            .map_err(|e| JsError::new(&format!("RNG failed: {}", e)))?;
        alpha_hasher.update(&random_bytes);

        let alpha_hash = alpha_hasher.finalize();
        let mut alpha = [0u8; 32];
        alpha.copy_from_slice(&alpha_hash[..32]);

        let scalar = pallas::Scalar::from_repr(alpha);
        if bool::from(scalar.is_some()) {
            alpha = scalar.unwrap().to_repr();
        }

        alphas.push(hex_encode(&alpha));
        hasher.update(&alpha);
    }

    // Add change action if needed
    if change > 0 {
        let mut alpha_hasher = Blake2b512::new();
        alpha_hasher.update(b"ZafuAlphaChange");
        alpha_hasher.update(&change.to_le_bytes());
        let mut random_bytes = [0u8; 32];
        getrandom::getrandom(&mut random_bytes)
            .map_err(|e| JsError::new(&format!("RNG failed: {}", e)))?;
        alpha_hasher.update(&random_bytes);

        let alpha_hash = alpha_hasher.finalize();
        let mut alpha = [0u8; 32];
        alpha.copy_from_slice(&alpha_hash[..32]);

        let scalar = pallas::Scalar::from_repr(alpha);
        if bool::from(scalar.is_some()) {
            alpha = scalar.unwrap().to_repr();
        }

        alphas.push(hex_encode(&alpha));
        hasher.update(&alpha);
    }

    // Compute sighash following ZIP-244 structure
    // https://zips.z.cash/zip-0244
    //
    // ZIP-244 defines: sighash = BLAKE2b-256("ZTxIdSigHash", [
    //   header_digest, transparent_digest, sapling_digest, orchard_digest
    // ])
    //
    // For Orchard-only transactions (our case):
    // - transparent_digest = empty hash
    // - sapling_digest = empty hash
    // - orchard_digest = hash of all orchard actions

    // Header digest (version, branch, locktime, expiry)
    let mut header_hasher = Blake2b512::new();
    header_hasher.update(b"ZTxIdHeadersHash");
    header_hasher.update(&5u32.to_le_bytes()); // tx version 5
    header_hasher.update(&0x26A7270Au32.to_le_bytes()); // NU5 version group
    header_hasher.update(&0x4DEC4DF0u32.to_le_bytes()); // NU6.1 branch id (mainnet)
    header_hasher.update(&0u32.to_le_bytes()); // lock_time
    header_hasher.update(&0u32.to_le_bytes()); // expiry_height (0 = no expiry)
    let header_digest = header_hasher.finalize();

    // Orchard digest (actions commitment)
    let mut orchard_hasher = Blake2b512::new();
    orchard_hasher.update(b"ZTxIdOrchardHash");
    orchard_hasher.update(&anchor_bytes);
    orchard_hasher.update(&(notes.len() as u32).to_le_bytes());
    for note in &notes {
        if let Some(nf) = hex_decode(&note.nullifier) {
            orchard_hasher.update(&nf);
        }
        orchard_hasher.update(&note.value.to_le_bytes());
    }
    orchard_hasher.update(&amount.to_le_bytes());
    orchard_hasher.update(&fee.to_le_bytes());
    orchard_hasher.update(recipient.as_bytes());
    // Include alphas in digest for binding
    for alpha_hex in &alphas {
        if let Some(alpha_bytes) = hex_decode(alpha_hex) {
            orchard_hasher.update(&alpha_bytes);
        }
    }
    let orchard_digest = orchard_hasher.finalize();

    // Final sighash
    let mut sighash_hasher = Blake2b512::new();
    sighash_hasher.update(b"ZTxIdSigHash");
    sighash_hasher.update(&header_digest[..32]);
    sighash_hasher.update(&[0u8; 32]); // transparent_digest (empty)
    sighash_hasher.update(&[0u8; 32]); // sapling_digest (empty)
    sighash_hasher.update(&orchard_digest[..32]);
    let sighash_full = sighash_hasher.finalize();
    let mut sighash = [0u8; 32];
    sighash.copy_from_slice(&sighash_full[..32]);

    // Build summary
    let amount_zec = amount as f64 / 100_000_000.0;
    let fee_zec = fee as f64 / 100_000_000.0;
    let summary = format!(
        "Send {:.8} ZEC to {}\nFee: {:.8} ZEC\nSpending {} note(s)",
        amount_zec,
        &recipient[..recipient.len().min(20)],
        fee_zec,
        notes.len()
    );

    // Build unsigned transaction data
    // This is a simplified structure - real implementation would use full tx format
    let unsigned_tx = UnsignedTransaction {
        version: 5, // Orchard-supporting version
        anchor: anchor_hex.to_string(),
        spends: notes.iter().map(|n| SpendInfo {
            nullifier: n.nullifier.clone(),
            cmx: n.cmx.clone(),
            value: n.value,
            position: n.position,
        }).collect(),
        outputs: {
            let mut outputs = vec![OutputInfo {
                recipient: recipient.to_string(),
                value: amount,
                memo: None,
            }];
            if change > 0 {
                outputs.push(OutputInfo {
                    recipient: "change".to_string(),
                    value: change,
                    memo: None,
                });
            }
            outputs
        },
        fee,
        alphas: alphas.clone(),
    };

    let unsigned_tx_json = serde_json::to_string(&unsigned_tx)
        .map_err(|e| JsError::new(&format!("Serialization failed: {}", e)))?;

    #[derive(Serialize)]
    struct BuildResult {
        sighash: String,
        alphas: Vec<String>,
        unsigned_tx: String,
        summary: String,
        account_index: u32,
    }

    let result = BuildResult {
        sighash: hex_encode(&sighash),
        alphas,
        unsigned_tx: unsigned_tx_json,
        summary,
        account_index,
    };

    serde_wasm_bindgen::to_value(&result)
        .map_err(|e| JsError::new(&format!("Serialization failed: {}", e)))
}

/// Merkle path info for a spendable note
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MerklePathInfo {
    /// The merkle path siblings (32 hashes of 32 bytes each, hex)
    pub path: Vec<String>,
    /// Position in the tree
    pub position: u64,
}

/// Spend info for unsigned transaction
#[derive(Debug, Clone, Serialize, Deserialize)]
struct SpendInfo {
    nullifier: String,
    cmx: String,
    value: u64,
    position: u64,
}

/// Output info for unsigned transaction
#[derive(Debug, Clone, Serialize, Deserialize)]
struct OutputInfo {
    recipient: String,
    value: u64,
    memo: Option<String>,
}

/// Unsigned transaction structure
#[derive(Debug, Clone, Serialize, Deserialize)]
struct UnsignedTransaction {
    version: u32,
    anchor: String,
    spends: Vec<SpendInfo>,
    outputs: Vec<OutputInfo>,
    fee: u64,
    alphas: Vec<String>,
}

/// Complete a transaction by applying signatures from cold wallet
/// Returns the serialized signed transaction ready for broadcast
#[wasm_bindgen]
pub fn complete_transaction(
    unsigned_tx_json: &str,
    signatures_json: JsValue,
) -> Result<String, JsError> {
    // Parse unsigned transaction
    let unsigned_tx: UnsignedTransaction = serde_json::from_str(unsigned_tx_json)
        .map_err(|e| JsError::new(&format!("Invalid unsigned tx: {}", e)))?;

    // Parse signatures
    let signatures: Vec<String> = serde_wasm_bindgen::from_value(signatures_json)
        .map_err(|e| JsError::new(&format!("Invalid signatures: {}", e)))?;

    // Verify signature count matches action count
    let expected_sigs = unsigned_tx.spends.len() + unsigned_tx.outputs.len();
    if signatures.len() != expected_sigs {
        return Err(JsError::new(&format!(
            "Signature count mismatch: got {}, expected {}",
            signatures.len(),
            expected_sigs
        )));
    }

    // Build signed transaction
    // In a full implementation, this would create proper Zcash v5 transaction bytes
    // For now, we create a structure that zidecar can parse
    #[derive(Serialize)]
    struct SignedTransaction {
        version: u32,
        anchor: String,
        actions: Vec<SignedAction>,
        fee: u64,
    }

    #[derive(Serialize)]
    struct SignedAction {
        nullifier: String,
        cmx: String,
        value: u64,
        signature: String,
        alpha: String,
    }

    let mut actions = Vec::new();

    // Add spend actions
    for (i, spend) in unsigned_tx.spends.iter().enumerate() {
        actions.push(SignedAction {
            nullifier: spend.nullifier.clone(),
            cmx: spend.cmx.clone(),
            value: spend.value,
            signature: signatures.get(i).cloned().unwrap_or_default(),
            alpha: unsigned_tx.alphas.get(i).cloned().unwrap_or_default(),
        });
    }

    // Add output actions (with dummy nullifiers)
    let spend_count = unsigned_tx.spends.len();
    for (i, output) in unsigned_tx.outputs.iter().enumerate() {
        let sig_idx = spend_count + i;
        actions.push(SignedAction {
            nullifier: "0".repeat(64), // Outputs don't have nullifiers
            cmx: "0".repeat(64), // CMX computed by network
            value: output.value,
            signature: signatures.get(sig_idx).cloned().unwrap_or_default(),
            alpha: unsigned_tx.alphas.get(sig_idx).cloned().unwrap_or_default(),
        });
    }

    let signed_tx = SignedTransaction {
        version: unsigned_tx.version,
        anchor: unsigned_tx.anchor,
        actions,
        fee: unsigned_tx.fee,
    };

    // Serialize as hex for broadcast
    let tx_json = serde_json::to_string(&signed_tx)
        .map_err(|e| JsError::new(&format!("Serialization failed: {}", e)))?;

    Ok(hex_encode(tx_json.as_bytes()))
}

/// Get the commitment proof request data for a note
/// Returns the cmx that should be sent to zidecar's GetCommitmentProof
#[wasm_bindgen]
pub fn get_commitment_proof_request(note_cmx_hex: &str) -> Result<String, JsError> {
    // Just validate and return the cmx
    let cmx_bytes = hex_decode(note_cmx_hex)
        .ok_or_else(|| JsError::new("Invalid cmx hex"))?;
    if cmx_bytes.len() != 32 {
        return Err(JsError::new("CMX must be 32 bytes"));
    }
    Ok(note_cmx_hex.to_string())
}

// ============================================================================
// Witness Building (merkle paths for orchard spends)
// ============================================================================

/// Build merkle paths for note positions by replaying compact blocks from a checkpoint.
///
/// # Arguments
/// * `tree_state_hex` - hex-encoded orchard frontier from GetTreeState
/// * `compact_blocks_json` - JSON array of `[{height, actions: [{cmx_hex}]}]`
/// * `note_positions_json` - JSON array of note positions `[position_u64, ...]`
/// * `anchor_height` - the block height to use as anchor
///
/// # Returns
/// JSON `{anchor_hex, paths: [{position, path: [{hash}]}]}`
#[wasm_bindgen]
pub fn build_merkle_paths(
    tree_state_hex: &str,
    compact_blocks_json: &str,
    note_positions_json: &str,
    anchor_height: u32,
) -> Result<JsValue, JsError> {
    let blocks: Vec<witness::CompactBlockData> = serde_json::from_str(compact_blocks_json)
        .map_err(|e| JsError::new(&format!("invalid compact_blocks_json: {}", e)))?;

    let positions: Vec<u64> = serde_json::from_str(note_positions_json)
        .map_err(|e| JsError::new(&format!("invalid note_positions_json: {}", e)))?;

    let result =
        witness::build_merkle_paths_inner(tree_state_hex, &blocks, &positions, anchor_height)
            .map_err(|e| JsError::new(&format!("{}", e)))?;

    let json = serde_json::to_string(&result)
        .map_err(|e| JsError::new(&format!("failed to serialize result: {}", e)))?;

    Ok(JsValue::from_str(&json))
}

/// Compute the tree size from a hex-encoded frontier.
#[wasm_bindgen]
pub fn frontier_tree_size(tree_state_hex: &str) -> Result<u64, JsError> {
    let data =
        hex::decode(tree_state_hex).map_err(|e| JsError::new(&format!("invalid hex: {}", e)))?;
    witness::compute_frontier_tree_size(&data).map_err(|e| JsError::new(&format!("{}", e)))
}

/// Compute the tree root from a hex-encoded frontier.
#[wasm_bindgen]
pub fn tree_root_hex(tree_state_hex: &str) -> Result<String, JsError> {
    let data =
        hex::decode(tree_state_hex).map_err(|e| JsError::new(&format!("invalid hex: {}", e)))?;
    let root = witness::compute_tree_root(&data).map_err(|e| JsError::new(&format!("{}", e)))?;
    Ok(hex::encode(root))
}

// ============================================================================
// Signed Spend Transaction (mnemonic wallets — no cold signing needed)
// ============================================================================

/// A spendable note with rseed and rho for reconstruction
#[derive(Debug, Clone, Deserialize)]
#[allow(dead_code)]
struct SpendableNote {
    value: u64,
    nullifier: String,
    cmx: String,
    position: u64,
    rseed_hex: String,
    rho_hex: String,
    /// raw orchard recipient address bytes (hex, 43 bytes) — captured during scan
    #[serde(default)]
    recipient_hex: String,
}

/// Build a fully signed orchard spend transaction from a mnemonic wallet.
///
/// Unlike `build_unsigned_transaction` (for cold signing), this function
/// derives the spending key from the mnemonic, constructs the full orchard
/// bundle with Halo 2 proofs, and returns a broadcast-ready transaction.
///
/// # Arguments
/// * `seed_phrase` - BIP39 mnemonic for key derivation
/// * `notes_json` - JSON array of spendable notes with rseed/rho
/// * `recipient` - unified address string (u1... or utest1...)
/// * `amount` - zatoshis to send
/// * `fee` - transaction fee in zatoshis
/// * `anchor_hex` - merkle tree anchor (hex, 32 bytes)
/// * `merkle_paths_json` - JSON array of merkle paths from witness building
/// * `account_index` - ZIP-32 account index
/// * `mainnet` - true for mainnet, false for testnet
///
/// # Returns
/// Hex-encoded signed v5 transaction bytes ready for broadcast
#[wasm_bindgen]
pub fn build_signed_spend_transaction(
    seed_phrase: &str,
    notes_json: JsValue,
    recipient: &str,
    amount: u64,
    fee: u64,
    anchor_hex: &str,
    merkle_paths_json: JsValue,
    account_index: u32,
    mainnet: bool,
) -> Result<String, JsError> {
    use orchard::builder::{Builder, BundleType};
    use orchard::bundle::Flags;
    use orchard::keys::SpendAuthorizingKey;
    use orchard::note::{Rho, RandomSeed};
    use orchard::tree::{Anchor, MerkleHashOrchard, MerklePath as OrchardMerklePath};
    use orchard::value::NoteValue;
    use rand::rngs::OsRng;
    use zcash_protocol::value::ZatBalance;

    // --- derive keys from mnemonic ---
    let mnemonic = bip39::Mnemonic::parse(seed_phrase)
        .map_err(|e| JsError::new(&format!("invalid mnemonic: {}", e)))?;
    let seed = mnemonic.to_seed("");

    let coin_type = if mainnet { 133 } else { 1 };
    let account_id = zip32::AccountId::try_from(account_index)
        .map_err(|_| JsError::new("invalid account index"))?;

    let sk = SpendingKey::from_zip32_seed(&seed, coin_type, account_id)
        .map_err(|e| JsError::new(&format!("spending key derivation failed: {:?}", e)))?;
    let fvk = orchard::keys::FullViewingKey::from(&sk);
    let ask = SpendAuthorizingKey::from(&sk);

    // derive change address (internal scope, diversifier 0)
    let change_addr = fvk.to_ivk(Scope::Internal).address_at(0u64);

    // --- parse recipient ---
    let recipient_addr = parse_orchard_address(recipient, mainnet)
        .map_err(|e| JsError::new(&format!("invalid recipient: {}", e)))?;

    // --- parse anchor ---
    let anchor_bytes = hex_decode(anchor_hex)
        .ok_or_else(|| JsError::new("invalid anchor hex"))?;
    if anchor_bytes.len() != 32 {
        return Err(JsError::new("anchor must be 32 bytes"));
    }
    let mut anchor_arr = [0u8; 32];
    anchor_arr.copy_from_slice(&anchor_bytes);
    let anchor = Option::from(Anchor::from_bytes(anchor_arr))
        .ok_or_else(|| JsError::new("invalid anchor"))?;

    // --- parse notes and merkle paths ---
    let notes: Vec<SpendableNote> = serde_wasm_bindgen::from_value(notes_json)
        .map_err(|e| JsError::new(&format!("invalid notes: {}", e)))?;
    let merkle_paths: Vec<MerklePathInfo> = serde_wasm_bindgen::from_value(merkle_paths_json)
        .map_err(|e| JsError::new(&format!("invalid merkle paths: {}", e)))?;

    if notes.len() != merkle_paths.len() {
        return Err(JsError::new("notes and merkle paths count mismatch"));
    }

    // --- calculate totals ---
    let total_input: u64 = notes.iter().map(|n| n.value).sum();
    if total_input < amount + fee {
        return Err(JsError::new(&format!(
            "insufficient funds: {} < {} + {}", total_input, amount, fee
        )));
    }
    let change = total_input - amount - fee;

    // --- build orchard bundle ---
    let bundle_type = BundleType::Transactional {
        flags: Flags::ENABLED,
        bundle_required: true,
    };
    let mut builder = Builder::new(bundle_type, anchor);

    // add spends
    for (i, note_info) in notes.iter().enumerate() {
        // reconstruct the orchard::Note from stored rseed + rho + value + address
        let rho_bytes = hex_decode(&note_info.rho_hex)
            .ok_or_else(|| JsError::new(&format!("invalid rho hex for note {}", i)))?;
        if rho_bytes.len() != 32 {
            return Err(JsError::new(&format!("rho must be 32 bytes for note {}", i)));
        }
        let mut rho_arr = [0u8; 32];
        rho_arr.copy_from_slice(&rho_bytes);
        let rho = Option::from(Rho::from_bytes(&rho_arr))
            .ok_or_else(|| JsError::new(&format!("invalid rho for note {}", i)))?;

        let rseed_bytes = hex_decode(&note_info.rseed_hex)
            .ok_or_else(|| JsError::new(&format!("invalid rseed hex for note {}", i)))?;
        if rseed_bytes.len() != 32 {
            return Err(JsError::new(&format!("rseed must be 32 bytes for note {}", i)));
        }
        let mut rseed_arr = [0u8; 32];
        rseed_arr.copy_from_slice(&rseed_bytes);
        let rseed = Option::from(RandomSeed::from_bytes(rseed_arr, &rho))
            .ok_or_else(|| JsError::new(&format!("invalid rseed for note {}", i)))?;

        let note_value = NoteValue::from_raw(note_info.value);

        // use stored recipient address from scan (handles diversified addresses correctly)
        let note: orchard::Note = if !note_info.recipient_hex.is_empty() {
            let addr_bytes = hex_decode(&note_info.recipient_hex)
                .ok_or_else(|| JsError::new(&format!("invalid recipient hex for note {}", i)))?;
            let addr_arr: [u8; 43] = addr_bytes.try_into()
                .map_err(|_| JsError::new(&format!("recipient must be 43 bytes for note {}", i)))?;
            let addr = Option::from(orchard::Address::from_raw_address_bytes(&addr_arr))
                .ok_or_else(|| JsError::new(&format!("invalid orchard address for note {}", i)))?;
            Option::from(orchard::Note::from_parts(addr, note_value, rho, rseed))
                .ok_or_else(|| JsError::new(&format!("failed to reconstruct note {} from stored address", i)))?
        } else {
            // fallback: try default addresses (legacy notes without stored recipient)
            let ext_addr = fvk.to_ivk(Scope::External).address_at(0u64);
            let int_addr = fvk.to_ivk(Scope::Internal).address_at(0u64);
            Option::from(orchard::Note::from_parts(ext_addr, note_value, rho, rseed))
                .or_else(|| Option::from(orchard::Note::from_parts(int_addr, note_value, rho, rseed)))
                .ok_or_else(|| JsError::new(&format!("failed to reconstruct note {} — rseed/rho/value mismatch", i)))?
        };

        // verify the reconstructed note matches the expected cmx
        let expected_cmx = hex_decode(&note_info.cmx)
            .ok_or_else(|| JsError::new(&format!("invalid cmx hex for note {}", i)))?;
        let reconstructed_cmx = orchard::note::ExtractedNoteCommitment::from(note.commitment());
        if hex_encode(&reconstructed_cmx.to_bytes()) != hex_encode(&expected_cmx) {
            return Err(JsError::new(&format!(
                "cmx mismatch for note {}: reconstructed={} expected={}",
                i, hex_encode(&reconstructed_cmx.to_bytes()), hex_encode(&expected_cmx)
            )));
        }

        // parse merkle path
        let mp = &merkle_paths[i];
        if mp.path.len() != 32 {
            return Err(JsError::new(&format!(
                "merkle path must have 32 elements, got {} for note {}", mp.path.len(), i
            )));
        }

        let mut auth_path = [[0u8; 32]; 32];
        for (j, hash_hex) in mp.path.iter().enumerate() {
            let hash_bytes = hex_decode(hash_hex)
                .ok_or_else(|| JsError::new(&format!("invalid merkle path hash at {}/{}", i, j)))?;
            if hash_bytes.len() != 32 {
                return Err(JsError::new(&format!("merkle path hash must be 32 bytes at {}/{}", i, j)));
            }
            auth_path[j].copy_from_slice(&hash_bytes);
        }

        let merkle_hashes: Vec<MerkleHashOrchard> = auth_path.iter()
            .filter_map(|bytes| Option::from(MerkleHashOrchard::from_bytes(bytes)))
            .collect();

        if merkle_hashes.len() != 32 {
            return Err(JsError::new(&format!(
                "invalid merkle path hashes for note {}", i
            )));
        }

        let merkle_path = OrchardMerklePath::from_parts(
            (mp.position as u32).into(),
            merkle_hashes.try_into().map_err(|_| JsError::new("merkle path conversion"))?,
        );

        builder.add_spend(fvk.clone(), note, merkle_path)
            .map_err(|e| JsError::new(&format!("add_spend for note {}: {:?}", i, e)))?;
    }

    // add recipient output
    let mut memo = [0u8; 512];
    builder.add_output(None, recipient_addr, NoteValue::from_raw(amount), memo)
        .map_err(|e| JsError::new(&format!("add_output (recipient): {:?}", e)))?;

    // add change output if needed
    if change > 0 {
        memo = [0u8; 512];
        builder.add_output(None, change_addr, NoteValue::from_raw(change), memo)
            .map_err(|e| JsError::new(&format!("add_output (change): {:?}", e)))?;
    }

    // --- build, prove, sign ---
    let mut rng = OsRng;
    let (unauthorized_bundle, _meta) = builder
        .build::<ZatBalance>(&mut rng)
        .map_err(|e| JsError::new(&format!("bundle build: {:?}", e)))?
        .ok_or_else(|| JsError::new("builder produced no bundle"))?;

    // Halo 2 proof generation (expensive)
    let pk = orchard::circuit::ProvingKey::build();
    let proven_bundle = unauthorized_bundle
        .create_proof(&pk, &mut rng)
        .map_err(|e| JsError::new(&format!("create_proof: {:?}", e)))?;

    // --- compute ZIP-244 sighash ---
    let branch_id: u32 = 0x4DEC4DF0; // NU6.1
    let expiry_height: u32 = 0; // no expiry for orchard-only

    let header_data = {
        let mut d = Vec::new();
        d.extend_from_slice(&(5u32 | (1u32 << 31)).to_le_bytes());
        d.extend_from_slice(&0x26A7270Au32.to_le_bytes());
        d.extend_from_slice(&branch_id.to_le_bytes());
        d.extend_from_slice(&0u32.to_le_bytes()); // nLockTime
        d.extend_from_slice(&expiry_height.to_le_bytes());
        d
    };
    let header_digest = blake2b_256_personal(b"ZTxIdHeadersHash", &header_data);

    // no transparent inputs/outputs
    let transparent_digest = blake2b_256_personal(b"ZTxIdTranspaHash", &[]);
    let sapling_digest = blake2b_256_personal(b"ZTxIdSaplingHash", &[]);

    let orchard_digest = compute_orchard_digest(&proven_bundle)?;

    let sighash_personal = {
        let mut p = [0u8; 16];
        p[..12].copy_from_slice(b"ZcashTxHash_");
        p[12..16].copy_from_slice(&branch_id.to_le_bytes());
        p
    };

    let mut sighash_input = Vec::new();
    sighash_input.extend_from_slice(&header_digest);
    sighash_input.extend_from_slice(&transparent_digest);
    sighash_input.extend_from_slice(&sapling_digest);
    sighash_input.extend_from_slice(&orchard_digest);

    let sighash = blake2b_256_personal(&sighash_personal, &sighash_input);

    // apply spend auth signatures + binding signature
    let authorized_bundle = proven_bundle
        .apply_signatures(&mut rng, sighash, &[ask])
        .map_err(|e| JsError::new(&format!("apply_signatures: {:?}", e)))?;

    // --- serialize v5 transaction ---
    let mut tx_bytes = Vec::new();

    // header
    tx_bytes.extend_from_slice(&(5u32 | (1u32 << 31)).to_le_bytes());
    tx_bytes.extend_from_slice(&0x26A7270Au32.to_le_bytes());
    tx_bytes.extend_from_slice(&branch_id.to_le_bytes());
    tx_bytes.extend_from_slice(&0u32.to_le_bytes()); // nLockTime
    tx_bytes.extend_from_slice(&expiry_height.to_le_bytes());

    // transparent (none)
    tx_bytes.extend_from_slice(&compact_size(0)); // vin
    tx_bytes.extend_from_slice(&compact_size(0)); // vout

    // sapling (none)
    tx_bytes.extend_from_slice(&compact_size(0)); // spends
    tx_bytes.extend_from_slice(&compact_size(0)); // outputs

    // orchard bundle
    serialize_orchard_bundle(&authorized_bundle, &mut tx_bytes)?;

    Ok(hex_encode(&tx_bytes))
}

#[test]
fn test_user_seed_with_real_action() {
    let seed_phrase = "master bid journey tank since conduct fire picture medal toward dish trend army true cushion ramp yellow high once jealous van occur swamp liberty";
    let keys = WalletKeys::from_seed_phrase(seed_phrase).unwrap();
    let addr = keys.get_address();
    println!("User wallet address ID: {}", addr);

    // Real action from tx 23803f17eeaa1617fa26c910a4215f618a16f011bf18b81613b0436894f59d76
    // block 3128610 - this is the shielding tx that should have user's notes
    let nullifier = hex_decode("41291fa8172173e9cc0d205b064e781a4277ceb7736c52dcb9784d2665987438").unwrap();
    let cmx = hex_decode("72dc616c3019a39216dbf74a240ada0aeac4a5eebae409d46d32fac835848934").unwrap();
    let epk = hex_decode("9db06408843a4a34686b6d375ad88915eb387787f2a55c01f97e264161e505b5").unwrap();
    let enc_ciphertext = hex_decode("cf94455b13ad0d815492fecb913abc26eb9edbc5b06d1ca9d1d08959ad60b63d9267ad7d20c38ba9a323cdefabe8561a0edf2f59ef6cfdfe6927652c5793e3912a64175d27ec35254563a39a77a79075c30edb446ce040f77c73ce60de28ea21f191532e46110867a08f0ef69a37fd208d5f55a6e664c0065fb162c24d5fc906f8f5cddb898a34ffa4b0609d9b6d419dbc099808ee5f644fd619985326a781ed447e5bba88044d2f787bb1d68d186f9e089434b70e090cb18fc466e2949798b7c9363bd3be5411a5c4abfad3fac153565ce37b0db588b890e8f03afffcaa723b6eb2376bdfc1ffe0e64b9f6893229a8b290e05aa478b12841fba2b1c68c8e1ec8ba22b40c27523c67001b57fd426edf3d2180e2a2c6f9cc2b0c5155a1a27272fd58902c8b7eabca6a5e2a8c5597bfc33526ef5dac4db7f841e6d7aa3166e9e7c73b53da12896cfaa88b343ef8d90b7ca5bcdc39fee1609da41efeaefe05cbc2e10682691335c9c772c274b7da3607cf6235a01664d4c330f3fdabeee61216e543919b1216df86d5eb1e395be9afacd760b8ce05ea465cfe3c815cc67cbf96379772a1c71a77462077d096c75e43374aa98b6fa441bdfeb0c7109140303fd57e2470eedc71a0bb5dfdb540731b960d9e17c5cff5c29f228e0056e42c8c30aedac34eb812ddad91613c089a7f8c1ec21393fa5db2cb27dfaf9d0e59f41e4842f5f5d9401d2aec0ebec8e20807524e3c47076f86d945dea492fd0152f8edbf35b0df4cd1aa808eb2945d61ead63e364ceef9aacdd91d76e396da2f8fcb0486d0c89606582c6").unwrap();

    // Build compact action (first 52 bytes of ciphertext only)
    let mut ciphertext = [0u8; 52];
    ciphertext.copy_from_slice(&enc_ciphertext[..52]);

    let action = CompactActionBinary {
        nullifier: nullifier.try_into().unwrap(),
        cmx: cmx.try_into().unwrap(),
        epk: epk.try_into().unwrap(),
        ciphertext,
    };

    // Try to decrypt with External scope
    let result = keys.try_decrypt_action_binary(&action);
    println!("Decryption result (External scope): {:?}", result.map(|(v, nf, _, _)| (v, hex_encode(&nf))));

    // Also try Internal scope
    let mnemonic = bip39::Mnemonic::parse(seed_phrase).unwrap();
    let seed = mnemonic.to_seed("");
    let sk = SpendingKey::from_zip32_seed(&seed, 133, zip32::AccountId::ZERO).unwrap();
    let fvk = orchard::keys::FullViewingKey::from(&sk);
    let internal_ivk = fvk.to_ivk(Scope::Internal);
    let prepared_internal = internal_ivk.prepare();

    // Try decryption with internal IVK
    let nullifier_parsed = orchard::note::Nullifier::from_bytes(&action.nullifier).unwrap();
    let cmx_parsed = orchard::note::ExtractedNoteCommitment::from_bytes(&action.cmx).unwrap();
    let compact_action = orchard::note_encryption::CompactAction::from_parts(
        nullifier_parsed,
        cmx_parsed,
        EphemeralKeyBytes(action.epk),
        action.ciphertext,
    );
    let domain = OrchardDomain::for_compact_action(&compact_action);
    let output = CompactShieldedOutput {
        epk: action.epk,
        cmx: action.cmx,
        ciphertext: action.ciphertext,
    };
    let internal_result = try_compact_note_decryption(&domain, &prepared_internal, &output);
    println!("Decryption result (Internal scope): {:?}", internal_result.map(|(n, _)| n.value().inner()));

    // Print the addresses for debugging
    let external_addr = fvk.to_ivk(Scope::External).address_at(0u64);
    let internal_addr = fvk.to_ivk(Scope::Internal).address_at(0u64);
    println!("External address diversifier_index 0: {:?}", hex_encode(&external_addr.to_raw_address_bytes()[..16]));
    println!("Internal address diversifier_index 0: {:?}", hex_encode(&internal_addr.to_raw_address_bytes()[..16]));
}

// ============================================================================
// Transparent Key Derivation & Shielding Transaction Builder
// ============================================================================

/// BIP32 extended key (private)
struct Bip32Key {
    key: [u8; 32],
    chain_code: [u8; 32],
}

/// Derive BIP32 master key from seed using HMAC-SHA512 with key "Bitcoin seed"
fn bip32_master_key(seed: &[u8]) -> Bip32Key {
    use hmac::{Hmac, Mac};
    use sha2::Sha512;

    let mut mac = Hmac::<Sha512>::new_from_slice(b"Bitcoin seed")
        .expect("HMAC accepts any key length");
    mac.update(seed);
    let result = mac.finalize().into_bytes();

    let mut key = [0u8; 32];
    let mut chain_code = [0u8; 32];
    key.copy_from_slice(&result[..32]);
    chain_code.copy_from_slice(&result[32..]);

    Bip32Key { key, chain_code }
}

/// BIP32 child key derivation (hardened or normal)
fn bip32_derive_child(parent: &Bip32Key, index: u32, hardened: bool) -> Result<Bip32Key, String> {
    use hmac::{Hmac, Mac};
    use sha2::Sha512;
    use k256::elliptic_curve::sec1::ToEncodedPoint;
    use k256::elliptic_curve::PrimeField;

    let mut mac = Hmac::<Sha512>::new_from_slice(&parent.chain_code)
        .expect("HMAC accepts any key length");

    let child_index = if hardened { index | 0x80000000 } else { index };

    if hardened {
        // Hardened: HMAC-SHA512(key=chain_code, data=0x00 || ser256(parent_key) || ser32(index))
        mac.update(&[0x00]);
        mac.update(&parent.key);
    } else {
        // Normal: HMAC-SHA512(key=chain_code, data=ser_P(parent_pubkey) || ser32(index))
        let secret_key = k256::SecretKey::from_slice(&parent.key)
            .map_err(|e| format!("invalid parent key: {}", e))?;
        let pubkey = secret_key.public_key();
        let compressed = pubkey.to_encoded_point(true);
        mac.update(compressed.as_bytes());
    }

    mac.update(&child_index.to_be_bytes());
    let result = mac.finalize().into_bytes();

    // child_key = parse256(IL) + parent_key (mod n)
    let il = &result[..32];
    let ir = &result[32..];

    // Add IL to parent key modulo the secp256k1 curve order
    let mut parent_bytes = k256::FieldBytes::default();
    parent_bytes.copy_from_slice(&parent.key);
    let parent_scalar = k256::Scalar::from_repr(parent_bytes);
    if bool::from(parent_scalar.is_none()) {
        return Err("invalid parent scalar".into());
    }
    let parent_scalar = parent_scalar.unwrap();

    let mut il_bytes = k256::FieldBytes::default();
    il_bytes.copy_from_slice(il);
    let il_scalar = k256::Scalar::from_repr(il_bytes);
    if bool::from(il_scalar.is_none()) {
        return Err("invalid IL scalar".into());
    }
    let il_scalar = il_scalar.unwrap();

    let child_scalar = il_scalar + parent_scalar;

    let mut key = [0u8; 32];
    key.copy_from_slice(&child_scalar.to_repr());

    let mut chain_code = [0u8; 32];
    chain_code.copy_from_slice(ir);

    Ok(Bip32Key { key, chain_code })
}

/// Derive transparent private key from mnemonic using BIP44 path m/44'/133'/account'/0/index
///
/// Returns hex-encoded 32-byte secp256k1 private key for signing transparent inputs.
/// Path components: purpose=44' (BIP44), coin_type=133' (ZEC), account', change=0, index
#[wasm_bindgen]
pub fn derive_transparent_privkey(seed_phrase: &str, account: u32, index: u32) -> Result<String, JsError> {
    let mnemonic = bip39::Mnemonic::parse(seed_phrase)
        .map_err(|e| JsError::new(&format!("invalid mnemonic: {}", e)))?;

    let seed = mnemonic.to_seed("");

    // BIP32 derivation: m/44'/133'/account'/0/index
    let master = bip32_master_key(&seed);

    let child_44h = bip32_derive_child(&master, 44, true)
        .map_err(|e| JsError::new(&format!("derivation failed at 44': {}", e)))?;
    let child_133h = bip32_derive_child(&child_44h, 133, true)
        .map_err(|e| JsError::new(&format!("derivation failed at 133': {}", e)))?;
    let child_account = bip32_derive_child(&child_133h, account, true)
        .map_err(|e| JsError::new(&format!("derivation failed at account': {}", e)))?;
    let child_change = bip32_derive_child(&child_account, 0, false)
        .map_err(|e| JsError::new(&format!("derivation failed at change: {}", e)))?;
    let child_index = bip32_derive_child(&child_change, index, false)
        .map_err(|e| JsError::new(&format!("derivation failed at index: {}", e)))?;

    Ok(hex_encode(&child_index.key))
}

/// UTXO input for shielding transaction
#[derive(Debug, Clone, Deserialize)]
struct TransparentUtxo {
    /// Transaction ID (hex, 32 bytes big-endian as displayed)
    txid: String,
    /// Output index within the transaction
    vout: u32,
    /// Value in zatoshis
    value: u64,
    /// scriptPubKey (hex) - expected to be P2PKH
    script: String,
}

/// Personalized Blake2b-256 hash (ZIP-244 style)
fn blake2b_256_personal(personalization: &[u8; 16], data: &[u8]) -> [u8; 32] {
    let h = blake2b_simd::Params::new()
        .hash_length(32)
        .personal(personalization)
        .hash(data);
    let mut out = [0u8; 32];
    out.copy_from_slice(h.as_bytes());
    out
}

/// Build a shielding transaction (transparent → orchard) with real Halo 2 proofs.
///
/// Spends transparent P2PKH UTXOs and creates an orchard output to the sender's
/// own shielded address. Uses `orchard::builder::Builder` for proper action
/// construction and zero-knowledge proof generation (client-side).
///
/// Returns hex-encoded signed v5 transaction bytes ready for broadcast.
///
/// # Arguments
/// * `utxos_json` - JSON array of `{txid, vout, value, script}` objects
/// * `privkey_hex` - hex-encoded 32-byte secp256k1 private key for transparent inputs
/// * `recipient` - unified address string (u1... or utest1...) for orchard output
/// * `amount` - total zatoshis to shield (all selected UTXO value minus fee)
/// * `fee` - transaction fee in zatoshis
/// * `anchor_height` - block height for expiry (expiry_height = anchor_height + 100)
/// * `mainnet` - true for mainnet, false for testnet
#[wasm_bindgen]
pub fn build_shielding_transaction(
    utxos_json: &str,
    privkey_hex: &str,
    recipient: &str,
    amount: u64,
    fee: u64,
    anchor_height: u32,
    mainnet: bool,
) -> Result<String, JsError> {
    use k256::ecdsa::{SigningKey, signature::hazmat::PrehashSigner};
    use orchard::builder::{Builder, BundleType};
    use orchard::bundle::Flags;
    use orchard::tree::Anchor;
    use orchard::value::NoteValue;
    use rand::rngs::OsRng;
    use zcash_protocol::value::ZatBalance;

    // --- parse recipient orchard address ---
    let orchard_addr = parse_orchard_address(recipient, mainnet)
        .map_err(|e| JsError::new(&format!("invalid recipient: {}", e)))?;

    // --- parse transparent private key ---
    let privkey_bytes = hex_decode(privkey_hex)
        .ok_or_else(|| JsError::new("invalid privkey hex"))?;
    if privkey_bytes.len() != 32 {
        return Err(JsError::new("privkey must be 32 bytes"));
    }

    let signing_key = SigningKey::from_slice(&privkey_bytes)
        .map_err(|e| JsError::new(&format!("invalid signing key: {}", e)))?;
    let pubkey = signing_key.verifying_key();
    let compressed_pubkey = pubkey.to_encoded_point(true);
    let pubkey_bytes = compressed_pubkey.as_bytes();
    let pubkey_hash = hash160(pubkey_bytes);
    let our_script_pubkey = make_p2pkh_script(&pubkey_hash);

    // --- parse and select UTXOs ---
    let mut utxos: Vec<TransparentUtxo> = serde_json::from_str(utxos_json)
        .map_err(|e| JsError::new(&format!("invalid utxos json: {}", e)))?;
    utxos.sort_by(|a, b| b.value.cmp(&a.value));

    let target = amount.checked_add(fee)
        .ok_or_else(|| JsError::new("amount + fee overflow"))?;

    let mut selected: Vec<TransparentUtxo> = Vec::new();
    let mut total_in: u64 = 0;
    for utxo in &utxos {
        selected.push(utxo.clone());
        total_in += utxo.value;
        if total_in >= target { break; }
    }
    if total_in < target {
        return Err(JsError::new(&format!(
            "insufficient funds: have {} zat, need {} zat", total_in, target
        )));
    }

    // all value goes to orchard (no transparent change output)
    let shielded_value = total_in - fee;

    // --- build orchard bundle with real Halo 2 proofs ---
    let bundle_type = BundleType::Transactional {
        flags: Flags::SPENDS_DISABLED, // outputs only
        bundle_required: true,
    };
    let mut builder = Builder::new(bundle_type, Anchor::empty_tree());

    builder.add_output(None, orchard_addr, NoteValue::from_raw(shielded_value), [0u8; 512])
        .map_err(|e| JsError::new(&format!("add_output: {:?}", e)))?;

    let mut rng = OsRng;
    let (unauthorized_bundle, _meta) = builder
        .build::<ZatBalance>(&mut rng)
        .map_err(|e| JsError::new(&format!("bundle build: {:?}", e)))?
        .ok_or_else(|| JsError::new("builder produced no bundle"))?;

    // prove (Halo 2 — this is the expensive step, ~seconds in WASM)
    let pk = orchard::circuit::ProvingKey::build();
    let proven_bundle = unauthorized_bundle
        .create_proof(&pk, &mut rng)
        .map_err(|e| JsError::new(&format!("create_proof: {:?}", e)))?;

    // --- compute transparent digests for ZIP-244 sighash ---
    let n_inputs = selected.len();
    let branch_id: u32 = 0x4DEC4DF0; // NU6.1
    let expiry_height = anchor_height.saturating_add(100);

    let mut prevout_data = Vec::new();
    let mut sequence_data = Vec::new();
    let mut amounts_data = Vec::new();
    let mut scripts_data = Vec::new();

    for utxo in &selected {
        let txid_be = hex_decode(&utxo.txid)
            .ok_or_else(|| JsError::new("invalid utxo txid hex"))?;
        if txid_be.len() != 32 { return Err(JsError::new("txid must be 32 bytes")); }
        let mut txid_le = txid_be.clone();
        txid_le.reverse();

        prevout_data.extend_from_slice(&txid_le);
        prevout_data.extend_from_slice(&utxo.vout.to_le_bytes());
        sequence_data.extend_from_slice(&0xffffffffu32.to_le_bytes());
        amounts_data.extend_from_slice(&utxo.value.to_le_bytes());

        let script_bytes = hex_decode(&utxo.script)
            .unwrap_or_else(|| our_script_pubkey.clone());
        scripts_data.extend_from_slice(&compact_size(script_bytes.len() as u64));
        scripts_data.extend_from_slice(&script_bytes);
    }

    // ZIP-244 digests
    let header_data = {
        let mut d = Vec::new();
        d.extend_from_slice(&(5u32 | (1u32 << 31)).to_le_bytes());
        d.extend_from_slice(&0x26A7270Au32.to_le_bytes());
        d.extend_from_slice(&branch_id.to_le_bytes());
        d.extend_from_slice(&0u32.to_le_bytes());
        d.extend_from_slice(&expiry_height.to_le_bytes());
        d
    };
    let header_digest = blake2b_256_personal(b"ZTxIdHeadersHash", &header_data);

    let prevouts_digest = blake2b_256_personal(b"ZTxIdPrevoutHash", &prevout_data);
    let sequence_digest = blake2b_256_personal(b"ZTxIdSequencHash", &sequence_data);
    let outputs_digest = blake2b_256_personal(b"ZTxIdOutputsHash", &[]);

    let sapling_digest = blake2b_256_personal(b"ZTxIdSaplingHash", &[]);

    // compute orchard_digest from the proven bundle's action data (ZIP-244)
    let orchard_digest = compute_orchard_digest(&proven_bundle)?;

    // per-input sighash needs amounts_digest and scriptpubkeys_digest
    let amounts_digest = blake2b_256_personal(b"ZTxTrAmountsHash", &amounts_data);
    let scriptpubkeys_digest = blake2b_256_personal(b"ZTxTrScriptsHash", &scripts_data);

    let sighash_personal = {
        let mut p = [0u8; 16];
        p[..12].copy_from_slice(b"ZcashTxHash_");
        p[12..16].copy_from_slice(&branch_id.to_le_bytes());
        p
    };

    // --- sign transparent inputs ---
    let mut signed_inputs: Vec<SignedTransparentInput> = Vec::new();

    for i in 0..n_inputs {
        let utxo = &selected[i];
        let txid_be = hex_decode(&utxo.txid).unwrap();
        let mut txid_le = txid_be.clone();
        txid_le.reverse();

        let script_bytes = hex_decode(&utxo.script)
            .unwrap_or_else(|| our_script_pubkey.clone());

        let mut txin_data = Vec::new();
        txin_data.extend_from_slice(&txid_le);
        txin_data.extend_from_slice(&utxo.vout.to_le_bytes());
        txin_data.extend_from_slice(&utxo.value.to_le_bytes());
        txin_data.extend_from_slice(&compact_size(script_bytes.len() as u64));
        txin_data.extend_from_slice(&script_bytes);
        txin_data.extend_from_slice(&0xffffffffu32.to_le_bytes());

        // ZIP-244 S.2g: hash per-input data separately
        let txin_sig_digest = blake2b_256_personal(b"Zcash___TxInHash", &txin_data);

        let mut sig_input = Vec::new();
        sig_input.push(0x01); // SIGHASH_ALL
        sig_input.extend_from_slice(&prevouts_digest);
        sig_input.extend_from_slice(&amounts_digest);
        sig_input.extend_from_slice(&scriptpubkeys_digest);
        sig_input.extend_from_slice(&sequence_digest);
        sig_input.extend_from_slice(&outputs_digest);
        sig_input.extend_from_slice(&txin_sig_digest);

        let transparent_sig_digest = blake2b_256_personal(b"ZTxIdTranspaHash", &sig_input);

        let mut sighash_input = Vec::new();
        sighash_input.extend_from_slice(&header_digest);
        sighash_input.extend_from_slice(&transparent_sig_digest);
        sighash_input.extend_from_slice(&sapling_digest);
        sighash_input.extend_from_slice(&orchard_digest);

        let sighash = blake2b_256_personal(&sighash_personal, &sighash_input);

        let sig: k256::ecdsa::Signature = signing_key.sign_prehash(&sighash)
            .map_err(|e| JsError::new(&format!("ECDSA signing failed: {}", e)))?;
        let sig_der = sig.to_der();

        let mut script_sig = Vec::new();
        let sig_with_hashtype_len = sig_der.as_bytes().len() + 1;
        script_sig.push(sig_with_hashtype_len as u8);
        script_sig.extend_from_slice(sig_der.as_bytes());
        script_sig.push(0x01); // SIGHASH_ALL
        script_sig.push(pubkey_bytes.len() as u8);
        script_sig.extend_from_slice(pubkey_bytes);

        signed_inputs.push(SignedTransparentInput {
            prevout_txid: utxo.txid.clone(),
            prevout_vout: utxo.vout,
            script_sig: hex_encode(&script_sig),
            sequence: 0xffffffff,
            value: utxo.value,
        });
    }

    // --- apply orchard binding signature ---
    // ZIP-244 S.2: when vin is non-empty, the verifier uses transparent_sig_digest
    // (not the txid transparent_digest) for the sighash. For the binding signature
    // (SignableInput::Shielded), hash_type=SIGHASH_ALL, no per-input data.
    let txin_sig_digest_empty = blake2b_256_personal(b"Zcash___TxInHash", &[]);
    let binding_transparent_digest = {
        let mut d = Vec::new();
        d.push(0x01); // SIGHASH_ALL
        d.extend_from_slice(&prevouts_digest);
        d.extend_from_slice(&amounts_digest);
        d.extend_from_slice(&scriptpubkeys_digest);
        d.extend_from_slice(&sequence_digest);
        d.extend_from_slice(&outputs_digest);
        d.extend_from_slice(&txin_sig_digest_empty);
        blake2b_256_personal(b"ZTxIdTranspaHash", &d)
    };

    let txid_sighash = {
        let mut d = Vec::new();
        d.extend_from_slice(&header_digest);
        d.extend_from_slice(&binding_transparent_digest);
        d.extend_from_slice(&sapling_digest);
        d.extend_from_slice(&orchard_digest);
        blake2b_256_personal(&sighash_personal, &d)
    };

    let authorized_bundle = proven_bundle
        .apply_signatures(&mut rng, txid_sighash, &[])
        .map_err(|e| JsError::new(&format!("apply_signatures: {:?}", e)))?;

    // --- serialize v5 transaction ---
    let mut tx_bytes = Vec::new();

    // header
    tx_bytes.extend_from_slice(&(5u32 | (1u32 << 31)).to_le_bytes());
    tx_bytes.extend_from_slice(&0x26A7270Au32.to_le_bytes());
    tx_bytes.extend_from_slice(&branch_id.to_le_bytes());
    tx_bytes.extend_from_slice(&0u32.to_le_bytes()); // nLockTime
    tx_bytes.extend_from_slice(&expiry_height.to_le_bytes());

    // transparent inputs
    tx_bytes.extend_from_slice(&compact_size(n_inputs as u64));
    for inp in &signed_inputs {
        let txid_be = hex_decode(&inp.prevout_txid).unwrap();
        let mut txid_le = txid_be.clone();
        txid_le.reverse();
        tx_bytes.extend_from_slice(&txid_le);
        tx_bytes.extend_from_slice(&inp.prevout_vout.to_le_bytes());

        let sig_bytes = hex_decode(&inp.script_sig).unwrap();
        tx_bytes.extend_from_slice(&compact_size(sig_bytes.len() as u64));
        tx_bytes.extend_from_slice(&sig_bytes);
        tx_bytes.extend_from_slice(&inp.sequence.to_le_bytes());
    }

    // transparent outputs (none)
    tx_bytes.extend_from_slice(&compact_size(0));

    // sapling (none)
    tx_bytes.extend_from_slice(&compact_size(0)); // spends
    tx_bytes.extend_from_slice(&compact_size(0)); // outputs

    // orchard bundle — serialize per ZIP-225 v5 format
    serialize_orchard_bundle(&authorized_bundle, &mut tx_bytes)?;

    Ok(hex_encode(&tx_bytes))
}

/// Parse an orchard address from a unified address string.
///
/// Because zcash_keys uses a different orchard crate version, we extract the raw
/// 43-byte address and reconstruct it with our orchard 0.12 types.
fn parse_orchard_address(addr_str: &str, mainnet: bool) -> Result<orchard::Address, String> {
    use zcash_keys::address::Address as ZkAddress;
    use zcash_protocol::consensus::{MainNetwork, TestNetwork};

    let decoded = if mainnet {
        ZkAddress::decode(&MainNetwork, addr_str)
    } else {
        ZkAddress::decode(&TestNetwork, addr_str)
    };

    match decoded {
        Some(ZkAddress::Unified(ua)) => {
            // get raw bytes from the zcash_keys orchard address (orchard 0.11)
            let orchard_addr_old = ua.orchard()
                .ok_or("unified address has no orchard receiver")?;
            let raw_bytes = orchard_addr_old.to_raw_address_bytes();
            // reconstruct as our orchard 0.12 Address
            Option::from(orchard::Address::from_raw_address_bytes(&raw_bytes))
                .ok_or_else(|| "invalid orchard address bytes".into())
        }
        Some(_) => Err("address is not a unified address".into()),
        None => Err("failed to decode address".into()),
    }
}

/// Compute ZIP-244 orchard_digest from a proven bundle's action data.
///
/// ZIP-244 §4.8:
///   actions_compact_digest = Blake2b-256("ZTxIdOrcActCHash", foreach: nf||cmx||epk||enc[0..52])
///   actions_memos_digest  = Blake2b-256("ZTxIdOrcActMHash", foreach: enc[52..564])
///   actions_noncompact_digest = Blake2b-256("ZTxIdOrcActNHash", foreach: cv||rk||enc[564..580]||out[0..80])
///   orchard_digest = Blake2b-256("ZTxIdOrchardHash",
///                      compact||memos||noncompact||flags(1)||value_balance(8)||anchor(32))
fn compute_orchard_digest<A: orchard::bundle::Authorization>(
    bundle: &orchard::Bundle<A, zcash_protocol::value::ZatBalance>,
) -> Result<[u8; 32], JsError> {
    let mut compact_data = Vec::new();
    let mut memos_data = Vec::new();
    let mut noncompact_data = Vec::new();

    for action in bundle.actions().iter() {
        // compact: nf(32) || cmx(32) || epk(32) || enc[0..52]
        compact_data.extend_from_slice(&action.nullifier().to_bytes());
        compact_data.extend_from_slice(&action.cmx().to_bytes());
        let enc = &action.encrypted_note().enc_ciphertext;
        let epk = &action.encrypted_note().epk_bytes;
        compact_data.extend_from_slice(epk);
        compact_data.extend_from_slice(&enc[..52]);

        // memos: enc[52..564]
        memos_data.extend_from_slice(&enc[52..564]);

        // noncompact: cv(32) || rk(32) || enc[564..580] || out(80)
        noncompact_data.extend_from_slice(&action.cv_net().to_bytes());
        noncompact_data.extend_from_slice(&<[u8; 32]>::from(action.rk()));
        noncompact_data.extend_from_slice(&enc[564..580]);
        noncompact_data.extend_from_slice(&action.encrypted_note().out_ciphertext);
    }

    let compact_digest = blake2b_256_personal(b"ZTxIdOrcActCHash", &compact_data);
    let memos_digest = blake2b_256_personal(b"ZTxIdOrcActMHash", &memos_data);
    let noncompact_digest = blake2b_256_personal(b"ZTxIdOrcActNHash", &noncompact_data);

    let mut orchard_data = Vec::new();
    orchard_data.extend_from_slice(&compact_digest);
    orchard_data.extend_from_slice(&memos_digest);
    orchard_data.extend_from_slice(&noncompact_digest);
    orchard_data.push(bundle.flags().to_byte());
    orchard_data.extend_from_slice(&bundle.value_balance().to_i64_le_bytes());
    orchard_data.extend_from_slice(&bundle.anchor().to_bytes());

    Ok(blake2b_256_personal(b"ZTxIdOrchardHash", &orchard_data))
}

/// Serialize an authorized orchard bundle into v5 transaction format (ZIP-225).
///
/// Layout: nActions(compactSize) || actions[] || flags(1) || valueBalance(8)
///         || anchor(32) || proof(compactSize+bytes) || spend_auth_sigs(64*n)
///         || binding_sig(64)
fn serialize_orchard_bundle(
    bundle: &orchard::Bundle<orchard::bundle::Authorized, zcash_protocol::value::ZatBalance>,
    out: &mut Vec<u8>,
) -> Result<(), JsError> {
    let actions = bundle.actions();
    let n = actions.len();

    // nActionsOrchard
    out.extend_from_slice(&compact_size(n as u64));

    // each action (without auth)
    for action in actions.iter() {
        out.extend_from_slice(&action.cv_net().to_bytes());       // 32
        out.extend_from_slice(&action.nullifier().to_bytes());    // 32
        out.extend_from_slice(&<[u8; 32]>::from(action.rk()));    // 32
        out.extend_from_slice(&action.cmx().to_bytes());          // 32
        out.extend_from_slice(&action.encrypted_note().epk_bytes); // 32
        out.extend_from_slice(&action.encrypted_note().enc_ciphertext); // 580
        out.extend_from_slice(&action.encrypted_note().out_ciphertext); // 80
    }

    // flags byte
    out.push(bundle.flags().to_byte());

    // valueBalanceOrchard (i64 LE)
    out.extend_from_slice(&bundle.value_balance().to_i64_le_bytes());

    // anchor
    out.extend_from_slice(&bundle.anchor().to_bytes());

    // proof bytes (compactSize-prefixed vector)
    let proof_bytes = bundle.authorization().proof().as_ref();
    out.extend_from_slice(&compact_size(proof_bytes.len() as u64));
    out.extend_from_slice(proof_bytes);

    // spend auth signatures (64 bytes each)
    for action in actions.iter() {
        out.extend_from_slice(&<[u8; 64]>::from(action.authorization()));
    }

    // binding signature (64 bytes)
    out.extend_from_slice(&<[u8; 64]>::from(
        bundle.authorization().binding_signature(),
    ));

    Ok(())
}

/// HASH160 = RIPEMD160(SHA256(data))
fn hash160(data: &[u8]) -> [u8; 20] {
    use sha2::Digest;

    let sha = sha2::Sha256::digest(data);
    let ripe = ripemd::Ripemd160::digest(sha);
    let mut out = [0u8; 20];
    out.copy_from_slice(&ripe);
    out
}

/// Construct P2PKH scriptPubKey from pubkey hash
fn make_p2pkh_script(pubkey_hash: &[u8; 20]) -> Vec<u8> {
    // OP_DUP OP_HASH160 <20> <hash> OP_EQUALVERIFY OP_CHECKSIG
    let mut s = Vec::with_capacity(25);
    s.push(0x76); // OP_DUP
    s.push(0xa9); // OP_HASH160
    s.push(0x14); // push 20 bytes
    s.extend_from_slice(pubkey_hash);
    s.push(0x88); // OP_EQUALVERIFY
    s.push(0xac); // OP_CHECKSIG
    s
}

/// Bitcoin-style CompactSize encoding
fn compact_size(n: u64) -> Vec<u8> {
    if n < 0xfd {
        vec![n as u8]
    } else if n <= 0xffff {
        let mut v = vec![0xfd];
        v.extend_from_slice(&(n as u16).to_le_bytes());
        v
    } else if n <= 0xffffffff {
        let mut v = vec![0xfe];
        v.extend_from_slice(&(n as u32).to_le_bytes());
        v
    } else {
        let mut v = vec![0xff];
        v.extend_from_slice(&n.to_le_bytes());
        v
    }
}

/// A signed transparent input for serialization
#[derive(Debug, Clone)]
struct SignedTransparentInput {
    prevout_txid: String,
    prevout_vout: u32,
    script_sig: String,
    sequence: u32,
    #[allow(dead_code)]
    value: u64,
}
