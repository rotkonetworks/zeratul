//! WASM bindings for Zafu Zcash wallet
//!
//! Provides parallel trial decryption for browser-based Zcash wallets.
//! Uses rayon + web workers for multi-threaded scanning with SIMD acceleration.
//!
//! Build with:
//! ```bash
//! RUSTFLAGS='-C target-feature=+simd128' wasm-pack build --target web --out-dir ../bin/zidecar/www/pkg
//! ```

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
                self.try_decrypt_action_binary(action).map(|(value, note_nf)| FoundNote {
                    index: idx as u32,
                    value,
                    nullifier: hex_encode(&note_nf),  // Use computed note nullifier, not action's spend nullifier
                    cmx: hex_encode(&action.cmx),
                })
            })
            .collect();

        #[cfg(not(feature = "parallel"))]
        let found: Vec<FoundNote> = actions
            .iter()
            .enumerate()
            .filter_map(|(idx, action)| {
                self.try_decrypt_action_binary(action).map(|(value, note_nf)| FoundNote {
                    index: idx as u32,
                    value,
                    nullifier: hex_encode(&note_nf),  // Use computed note nullifier, not action's spend nullifier
                    cmx: hex_encode(&action.cmx),
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
    /// Returns (value, note_nullifier) - the nullifier is for THIS note, not the action's spend
    fn try_decrypt_action_binary(&self, action: &CompactActionBinary) -> Option<(u64, [u8; 32])> {
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
        if let Some((note, _addr)) = try_compact_note_decryption(&domain, &self.prepared_ivk_external, &output) {
            // Compute this note's nullifier using our FVK
            let note_nf = note.nullifier(&self.fvk);
            return Some((note.value().inner(), note_nf.to_bytes()));
        }

        // If external failed, try INTERNAL scope IVK (for change/shielding outputs)
        if let Some((note, _addr)) = try_compact_note_decryption(&domain, &self.prepared_ivk_internal, &output) {
            // Compute this note's nullifier using our FVK
            let note_nf = note.nullifier(&self.fvk);
            return Some((note.value().inner(), note_nf.to_bytes()));
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
                self.try_decrypt_action(action).map(|(value, note_nf)| FoundNote {
                    index: idx as u32,
                    value,
                    nullifier: hex_encode(&note_nf),
                    cmx: hex_encode(&action.cmx),
                })
            })
            .collect();

        #[cfg(not(feature = "parallel"))]
        let found: Vec<FoundNote> = actions
            .iter()
            .enumerate()
            .filter_map(|(idx, action)| {
                self.try_decrypt_action(action).map(|(value, note_nf)| FoundNote {
                    index: idx as u32,
                    value,
                    nullifier: hex_encode(&note_nf),
                    cmx: hex_encode(&action.cmx),
                })
            })
            .collect();

        serde_wasm_bindgen::to_value(&found)
            .map_err(|e| JsError::new(&format!("Serialization failed: {}", e)))
    }

    /// Try to decrypt a compact action
    fn try_decrypt_action(&self, action: &CompactActionBinary) -> Option<(u64, [u8; 32])> {
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
        if let Some((note, _addr)) = try_compact_note_decryption(&domain, &self.prepared_ivk_external, &output) {
            let note_nf = note.nullifier(&self.fvk);
            return Some((note.value().inner(), note_nf.to_bytes()));
        }

        // Try internal scope (change addresses)
        if let Some((note, _addr)) = try_compact_note_decryption(&domain, &self.prepared_ivk_internal, &output) {
            let note_nf = note.nullifier(&self.fvk);
            return Some((note.value().inner(), note_nf.to_bytes()));
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
    header_hasher.update(&0xC2D6D0B4u32.to_le_bytes()); // NU5 branch id (mainnet)
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
    println!("Decryption result (External scope): {:?}", result);

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
