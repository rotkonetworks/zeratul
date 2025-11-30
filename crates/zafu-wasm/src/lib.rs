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
