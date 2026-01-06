//! vault client - connects to ghettobox vault testnet
//!
//! checks vault node health and handles key registration/recovery

use serde::{Deserialize, Serialize};
use std::sync::mpsc;
use std::thread;

/// vault node info
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct VaultNodeInfo {
    pub index: u32,
    pub pubkey: String,
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
    /// register a key share with a vault node
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

    /// recover a key share from a vault node
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
