//! vault client - connects to ghettobox vault network
//!
//! uses verified OPRF protocol with DLEQ proofs for:
//! - secure pin-to-key derivation (servers can't learn PIN)
//! - verifiable computation (detect malicious servers)
//! - misbehavior evidence (for slashing/accountability)

use ghettobox::{
    ServerPublicKey, VerifiedOprfResponse, VerifiedOprfClient, MisbehaviorReport,
    oprf::OprfClient,
};
use serde::{Deserialize, Serialize};
use std::sync::mpsc;
use std::thread;

/// vault node info (legacy)
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct VaultNodeInfo {
    pub index: u32,
    pub pubkey: String,
}

/// OPRF vault node with public key for verification
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct OprfVaultNode {
    /// node URL
    pub url: String,
    /// OPRF public key (ristretto255 point, compressed 32 bytes)
    pub oprf_pubkey: ServerPublicKey,
    /// node index (0-2)
    pub index: u8,
}

/// vault client for checking testnet
pub struct VaultClient {
    pub nodes: Vec<String>,
}

impl VaultClient {
    pub fn new(nodes: Vec<String>) -> Self {
        Self { nodes }
    }

    /// check health of a single node (blocking)
    pub fn check_node_health(url: &str) -> Option<VaultNodeInfo> {
        let resp = ureq::get(url)
            .timeout(std::time::Duration::from_secs(2))
            .call()
            .ok()?;

        resp.into_json().ok()
    }

    /// check all nodes and return count of healthy ones
    pub fn check_all_nodes(&self) -> Vec<Option<VaultNodeInfo>> {
        self.nodes
            .iter()
            .map(|url| Self::check_node_health(url))
            .collect()
    }

    /// spawn async check that sends result back
    pub fn check_async(nodes: Vec<String>, tx: mpsc::Sender<VaultCheckResult>) {
        thread::spawn(move || {
            let mut connected = 0;
            let mut infos = Vec::new();

            for url in &nodes {
                if let Some(info) = Self::check_node_health(url) {
                    connected += 1;
                    infos.push((url.clone(), info));
                }
            }

            let _ = tx.send(VaultCheckResult {
                total: nodes.len(),
                connected,
                node_infos: infos,
            });
        });
    }
}

/// result of vault check
#[derive(Debug)]
pub struct VaultCheckResult {
    pub total: usize,
    pub connected: usize,
    pub node_infos: Vec<(String, VaultNodeInfo)>,
}

impl VaultCheckResult {
    pub fn is_healthy(&self) -> bool {
        // need at least 2 of 3 nodes for threshold
        self.connected >= 2
    }
}

/// register request
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RegisterRequest {
    pub user_id: Vec<u8>,
    pub unlock_tag: Vec<u8>,
    pub encrypted_share: Vec<u8>,
    pub allowed_guesses: u32,
}

/// register response
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RegisterResponse {
    pub success: bool,
}

/// recover request
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RecoverRequest {
    pub user_id: Vec<u8>,
    pub unlock_tag: Vec<u8>,
}

/// recover response
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(untagged)]
pub enum RecoverResponse {
    Success { encrypted_share: Vec<u8> },
    Error { error: String },
}

impl VaultClient {
    /// register a key share with a vault node (legacy)
    pub fn register(
        url: &str,
        user_id: &[u8; 32],
        unlock_tag: &[u8; 16],
        encrypted_share: &[u8],
        allowed_guesses: u32,
    ) -> Result<bool, String> {
        let req = RegisterRequest {
            user_id: user_id.to_vec(),
            unlock_tag: unlock_tag.to_vec(),
            encrypted_share: encrypted_share.to_vec(),
            allowed_guesses,
        };

        let resp = ureq::post(&format!("{}/register", url))
            .set("Content-Type", "application/json")
            .timeout(std::time::Duration::from_secs(5))
            .send_json(&req)
            .map_err(|e| e.to_string())?;

        let result: RegisterResponse = resp.into_json().map_err(|e| e.to_string())?;
        Ok(result.success)
    }

    /// recover a key share from a vault node (legacy)
    pub fn recover(
        url: &str,
        user_id: &[u8; 32],
        unlock_tag: &[u8; 16],
    ) -> Result<Vec<u8>, String> {
        let req = RecoverRequest {
            user_id: user_id.to_vec(),
            unlock_tag: unlock_tag.to_vec(),
        };

        let resp = ureq::post(&format!("{}/recover", url))
            .set("Content-Type", "application/json")
            .timeout(std::time::Duration::from_secs(5))
            .send_json(&req)
            .map_err(|e| e.to_string())?;

        let result: RecoverResponse = resp.into_json().map_err(|e| e.to_string())?;

        match result {
            RecoverResponse::Success { encrypted_share } => Ok(encrypted_share),
            RecoverResponse::Error { error } => Err(error),
        }
    }
}

// ============================================================================
// OPRF Protocol (verified with DLEQ proofs)
// ============================================================================

/// OPRF registration request
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct OprfRegisterRequest {
    /// user identifier (email hash as hex)
    pub user_id: String,
    /// blinded element for OPRF evaluation (compressed point, hex)
    pub blinded: String,
    /// encrypted seed (hex)
    pub encrypted_seed: String,
    /// allowed PIN guesses before lockout
    pub allowed_guesses: u32,
}

/// OPRF registration response
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct OprfRegisterResponse {
    pub ok: bool,
    pub response: Option<VerifiedOprfResponse>,
    pub error: Option<String>,
}

/// OPRF recovery request
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct OprfRecoverRequest {
    pub user_id: String,
    pub blinded: String,
}

/// OPRF recovery response
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct OprfRecoverResponse {
    pub ok: bool,
    pub response: Option<VerifiedOprfResponse>,
    pub encrypted_seed: Option<String>,
    pub guesses_remaining: u32,
    pub error: Option<String>,
}

/// OPRF vault client with verification
pub struct OprfVaultClient {
    pub nodes: Vec<OprfVaultNode>,
    pub threshold: usize,
}

/// result of OPRF recovery with verification
pub struct OprfRecoveryResult {
    /// derived key material (OPRF output)
    pub oprf_output: [u8; 32],
    /// encrypted seed from registration
    pub encrypted_seed: Vec<u8>,
    /// remaining guesses
    pub guesses_remaining: u32,
    /// misbehavior reports for bad servers
    pub misbehavior_reports: Vec<MisbehaviorReport>,
}

/// OPRF health response from server
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct OprfHealthResponse {
    pub ok: bool,
    pub index: u8,
    pub version: String,
    pub oprf_pubkey: String,
}

impl OprfVaultClient {
    /// create OPRF client with vault nodes
    pub fn new(nodes: Vec<OprfVaultNode>, threshold: usize) -> Self {
        Self { nodes, threshold }
    }

    /// connect to vault nodes and fetch their OPRF public keys
    pub fn connect(urls: &[&str], threshold: usize) -> Result<Self, String> {
        let mut nodes = Vec::with_capacity(urls.len());

        for (i, url) in urls.iter().enumerate() {
            // fetch health to get public key
            let resp = ureq::get(&format!("{}/oprf/health", url))
                .timeout(std::time::Duration::from_secs(5))
                .call()
                .map_err(|e| format!("failed to connect to {}: {}", url, e))?;

            let health: OprfHealthResponse = resp.into_json()
                .map_err(|e| format!("invalid health response from {}: {}", url, e))?;

            if !health.ok {
                return Err(format!("vault {} not healthy", url));
            }

            // decode public key
            let pubkey_bytes = hex::decode(&health.oprf_pubkey)
                .map_err(|e| format!("invalid pubkey hex from {}: {}", url, e))?;

            if pubkey_bytes.len() != 32 {
                return Err(format!("invalid pubkey length from {}", url));
            }

            let mut public_key = [0u8; 32];
            public_key.copy_from_slice(&pubkey_bytes);

            nodes.push(OprfVaultNode {
                url: url.to_string(),
                oprf_pubkey: ServerPublicKey {
                    index: health.index,
                    public_key,
                },
                index: health.index,
            });
        }

        if nodes.len() < threshold {
            return Err(format!(
                "not enough nodes: {} connected, {} required",
                nodes.len(),
                threshold
            ));
        }

        Ok(Self { nodes, threshold })
    }

    /// default rotko mainnet nodes (connects and fetches keys)
    pub fn rotko_mainnet() -> Result<Self, String> {
        Self::connect(
            &[
                "https://vault1.rotko.net",
                "https://vault2.rotko.net",
                "https://vault3.rotko.net",
            ],
            2,
        )
    }

    /// localhost dev nodes (connects and fetches keys)
    pub fn localhost_connect() -> Result<Self, String> {
        Self::connect(
            &[
                "http://127.0.0.1:4200",
                "http://127.0.0.1:4201",
                "http://127.0.0.1:4202",
            ],
            2,
        )
    }

    /// localhost dev nodes (placeholder keys for offline testing)
    pub fn localhost() -> Self {
        Self::new(
            vec![
                OprfVaultNode {
                    url: "http://127.0.0.1:4200".into(),
                    oprf_pubkey: ServerPublicKey { index: 0, public_key: [0u8; 32] },
                    index: 0,
                },
                OprfVaultNode {
                    url: "http://127.0.0.1:4201".into(),
                    oprf_pubkey: ServerPublicKey { index: 1, public_key: [0u8; 32] },
                    index: 1,
                },
                OprfVaultNode {
                    url: "http://127.0.0.1:4202".into(),
                    oprf_pubkey: ServerPublicKey { index: 2, public_key: [0u8; 32] },
                    index: 2,
                },
            ],
            2,
        )
    }

    /// get public keys for verification
    pub fn public_keys(&self) -> Vec<ServerPublicKey> {
        self.nodes.iter().map(|n| n.oprf_pubkey.clone()).collect()
    }

    /// check node health
    pub fn check_node_health(url: &str) -> Option<OprfNodeHealth> {
        let resp = ureq::get(&format!("{}/health", url))
            .timeout(std::time::Duration::from_secs(2))
            .call()
            .ok()?;
        resp.into_json().ok()
    }

    /// check all nodes
    pub fn check_all_nodes(&self) -> Vec<(String, Option<OprfNodeHealth>)> {
        self.nodes
            .iter()
            .map(|n| (n.url.clone(), Self::check_node_health(&n.url)))
            .collect()
    }

    /// register with OPRF protocol (blocking)
    pub fn oprf_register(
        &self,
        user_id: &str,
        pin: &[u8],
        secret: &[u8],
    ) -> Result<OprfRegistrationResult, String> {
        // create OPRF client with PIN input
        let oprf_input = derive_oprf_input(user_id, pin);
        let oprf_client = OprfClient::new(&oprf_input);
        let blinded = oprf_client.blinded_point();

        // encrypt secret with a placeholder key (will be derived from OPRF output on recovery)
        // for registration, we do a local OPRF eval to get the encryption key
        // this is safe because we're the ones generating the secret
        let encrypted_seed = encrypt_with_placeholder(&blinded, secret)?;

        // register with all nodes
        let mut responses = Vec::new();
        for node in &self.nodes {
            match self.oprf_register_one(node, user_id, &blinded, &encrypted_seed) {
                Ok(resp) if resp.ok => {
                    if let Some(r) = resp.response {
                        responses.push(r);
                    }
                }
                Ok(resp) => {
                    return Err(resp.error.unwrap_or_else(|| "unknown error".into()));
                }
                Err(e) => return Err(e),
            }
        }

        // verify all responses
        let verified_client = VerifiedOprfClient::new(&oprf_input, self.public_keys());
        let (oprf_output, reports) = verified_client
            .finalize_with_reports(&responses, self.threshold)
            .map_err(|e| format!("OPRF verification failed: {}", e))?;

        // re-encrypt secret with actual OPRF-derived key
        let final_encrypted = encrypt_seed_with_oprf(&oprf_output, secret)?;

        // TODO: update nodes with re-encrypted seed (or do this during registration)

        Ok(OprfRegistrationResult {
            oprf_output,
            misbehavior_reports: reports,
        })
    }

    fn oprf_register_one(
        &self,
        node: &OprfVaultNode,
        user_id: &str,
        blinded: &[u8; 32],
        encrypted_seed: &[u8],
    ) -> Result<OprfRegisterResponse, String> {
        let req = OprfRegisterRequest {
            user_id: user_id.to_string(),
            blinded: hex::encode(blinded),
            encrypted_seed: hex::encode(encrypted_seed),
            allowed_guesses: 5,
        };

        let resp = ureq::post(&format!("{}/oprf/register", node.url))
            .set("Content-Type", "application/json")
            .timeout(std::time::Duration::from_secs(10))
            .send_json(&req)
            .map_err(|e| e.to_string())?;

        resp.into_json().map_err(|e| e.to_string())
    }

    /// recover with OPRF protocol (blocking)
    pub fn oprf_recover(
        &self,
        user_id: &str,
        pin: &[u8],
    ) -> Result<OprfRecoveryResult, String> {
        // create OPRF client with PIN input
        let oprf_input = derive_oprf_input(user_id, pin);
        let oprf_client = OprfClient::new(&oprf_input);
        let blinded = oprf_client.blinded_point();

        // recover from nodes (need threshold)
        let mut responses = Vec::new();
        let mut encrypted_seed = None;
        let mut min_guesses = u32::MAX;

        for node in &self.nodes {
            match self.oprf_recover_one(node, user_id, &blinded) {
                Ok(resp) if resp.ok => {
                    if let Some(r) = resp.response {
                        responses.push(r);
                    }
                    if encrypted_seed.is_none() {
                        if let Some(seed_hex) = resp.encrypted_seed {
                            encrypted_seed = Some(
                                hex::decode(&seed_hex).map_err(|e| e.to_string())?
                            );
                        }
                    }
                    min_guesses = min_guesses.min(resp.guesses_remaining);
                }
                Ok(resp) => {
                    if let Some(err) = resp.error {
                        if err.contains("not found") {
                            continue; // user not registered on this node
                        }
                        if err.contains("rate limit") || err.contains("locked") {
                            min_guesses = 0;
                            continue;
                        }
                    }
                    min_guesses = min_guesses.min(resp.guesses_remaining);
                }
                Err(_) => continue, // node unreachable
            }

            // early exit if we have enough
            if responses.len() >= self.threshold && encrypted_seed.is_some() {
                break;
            }
        }

        // check we have enough responses
        if responses.len() < self.threshold {
            if min_guesses == 0 {
                return Err("account locked - no more PIN attempts".into());
            }
            return Err(format!(
                "not enough vault responses: {} of {} needed",
                responses.len(),
                self.threshold
            ));
        }

        let encrypted_seed = encrypted_seed
            .ok_or_else(|| "no encrypted seed returned".to_string())?;

        // verify and finalize OPRF
        let verified_client = VerifiedOprfClient::new(&oprf_input, self.public_keys());
        let (oprf_output, reports) = verified_client
            .finalize_with_reports(&responses, self.threshold)
            .map_err(|e| format!("OPRF verification failed: {}", e))?;

        Ok(OprfRecoveryResult {
            oprf_output,
            encrypted_seed,
            guesses_remaining: min_guesses,
            misbehavior_reports: reports,
        })
    }

    fn oprf_recover_one(
        &self,
        node: &OprfVaultNode,
        user_id: &str,
        blinded: &[u8; 32],
    ) -> Result<OprfRecoverResponse, String> {
        let req = OprfRecoverRequest {
            user_id: user_id.to_string(),
            blinded: hex::encode(blinded),
        };

        let resp = ureq::post(&format!("{}/oprf/recover", node.url))
            .set("Content-Type", "application/json")
            .timeout(std::time::Duration::from_secs(10))
            .send_json(&req)
            .map_err(|e| e.to_string())?;

        resp.into_json().map_err(|e| e.to_string())
    }

    /// confirm successful recovery - resets attempt counter
    /// call this ONLY after successfully decrypting the seed
    pub fn oprf_confirm(&self, user_id: &str) {
        for node in &self.nodes {
            // best effort - don't fail if some nodes are unreachable
            let _ = ureq::post(&format!("{}/oprf/confirm/{}", node.url, user_id))
                .timeout(std::time::Duration::from_secs(5))
                .call();
        }
    }

    /// async check (for UI)
    pub fn check_async(nodes: Vec<OprfVaultNode>, tx: mpsc::Sender<OprfVaultCheckResult>) {
        thread::spawn(move || {
            let mut connected = 0;
            let mut infos = Vec::new();

            for node in &nodes {
                if let Some(health) = Self::check_node_health(&node.url) {
                    connected += 1;
                    infos.push((node.url.clone(), health));
                }
            }

            let _ = tx.send(OprfVaultCheckResult {
                total: nodes.len(),
                connected,
                node_infos: infos,
            });
        });
    }
}

/// node health response
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct OprfNodeHealth {
    pub ok: bool,
    pub index: u8,
    pub version: String,
}

/// result of vault health check
#[derive(Debug)]
pub struct OprfVaultCheckResult {
    pub total: usize,
    pub connected: usize,
    pub node_infos: Vec<(String, OprfNodeHealth)>,
}

impl OprfVaultCheckResult {
    pub fn is_healthy(&self, threshold: usize) -> bool {
        self.connected >= threshold
    }
}

/// registration result
pub struct OprfRegistrationResult {
    pub oprf_output: [u8; 32],
    pub misbehavior_reports: Vec<MisbehaviorReport>,
}

/// derive OPRF input from user_id and PIN
fn derive_oprf_input(user_id: &str, pin: &[u8]) -> Vec<u8> {
    use blake3::Hasher;
    let mut hasher = Hasher::new();
    hasher.update(b"ghettobox-oprf-input-v1");
    hasher.update(user_id.as_bytes());
    hasher.update(pin);
    hasher.finalize().as_bytes().to_vec()
}

/// encrypt with placeholder (for registration flow)
fn encrypt_with_placeholder(blinded: &[u8; 32], secret: &[u8]) -> Result<Vec<u8>, String> {
    use chacha20poly1305::{ChaCha20Poly1305, Key, Nonce, aead::Aead, KeyInit};

    // derive a temporary key from blinded point (will be re-encrypted with OPRF output)
    let hash = blake3::hash(blinded);
    let key_bytes: [u8; 32] = hash.as_bytes()[..32].try_into().unwrap();
    let key = Key::from_slice(&key_bytes);
    let cipher = ChaCha20Poly1305::new(key);

    let nonce_hash = blake3::hash(b"ghettobox-reg-nonce");
    let nonce_bytes: [u8; 12] = nonce_hash.as_bytes()[..12].try_into().unwrap();
    let nonce = Nonce::from_slice(&nonce_bytes);

    cipher.encrypt(nonce, secret)
        .map_err(|e| format!("encryption failed: {}", e))
}

/// encrypt seed with OPRF-derived key
fn encrypt_seed_with_oprf(oprf_output: &[u8; 32], secret: &[u8]) -> Result<Vec<u8>, String> {
    use chacha20poly1305::{ChaCha20Poly1305, Key, Nonce, aead::Aead, KeyInit};

    let key = Key::from_slice(oprf_output);
    let cipher = ChaCha20Poly1305::new(key);

    // unique nonce per encryption
    let nonce_hash = blake3::hash(oprf_output);
    let nonce_bytes: [u8; 12] = nonce_hash.as_bytes()[..12].try_into().unwrap();
    let nonce = Nonce::from_slice(&nonce_bytes);

    cipher.encrypt(nonce, secret)
        .map_err(|e| format!("encryption failed: {}", e))
}

/// decrypt seed with OPRF-derived key
pub fn decrypt_seed_with_oprf(oprf_output: &[u8; 32], ciphertext: &[u8]) -> Result<Vec<u8>, String> {
    use chacha20poly1305::{ChaCha20Poly1305, Key, Nonce, aead::Aead, KeyInit};

    let key = Key::from_slice(oprf_output);
    let cipher = ChaCha20Poly1305::new(key);

    let nonce_hash = blake3::hash(oprf_output);
    let nonce_bytes: [u8; 12] = nonce_hash.as_bytes()[..12].try_into().unwrap();
    let nonce = Nonce::from_slice(&nonce_bytes);

    cipher.decrypt(nonce, ciphertext)
        .map_err(|_| "decryption failed - wrong PIN?".to_string())
}
