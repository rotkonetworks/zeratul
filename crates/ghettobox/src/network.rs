//! network client for distributed TPM nodes
//!
//! talks to 3 realm nodes that each hold a VSS share sealed to their TPM

use crate::vss::{Share, THRESHOLD};
use crate::{Error, Result};
use serde::{Deserialize, Serialize};

/// realm node endpoint
#[derive(Clone, Debug)]
pub struct RealmNode {
    /// node url
    pub url: String,
    /// node public key (for verifying responses)
    pub pubkey: [u8; 32],
    /// node index (1-3)
    pub index: u8,
}

/// network client for distributed VSS
pub struct NetworkClient {
    nodes: Vec<RealmNode>,
    #[cfg(feature = "network")]
    http: reqwest::Client,
}

/// registration request to a realm node
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RegisterRequest {
    /// user identifier (email hash)
    pub user_id: [u8; 32],
    /// pin-stretched unlock key tag
    pub unlock_tag: [u8; 16],
    /// encrypted share (sealed to this node's TPM)
    pub encrypted_share: Vec<u8>,
    /// allowed PIN guesses before lockout
    pub allowed_guesses: u32,
}

/// registration response from realm node
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RegisterResponse {
    /// success
    pub ok: bool,
    /// node signature over registration (hex encoded)
    pub signature: String,
}

/// recovery request to a realm node
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RecoverRequest {
    /// user identifier
    pub user_id: [u8; 32],
    /// unlock key tag (derived from PIN)
    pub unlock_tag: [u8; 16],
}

/// recovery response from realm node
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RecoverResponse {
    /// success
    pub ok: bool,
    /// the decrypted share (if unlock_tag verified)
    pub share: Option<Share>,
    /// remaining guesses
    pub guesses_remaining: u32,
    /// error message if failed
    pub error: Option<String>,
}

impl NetworkClient {
    /// create client with 3 realm nodes
    pub fn new(nodes: Vec<RealmNode>) -> Result<Self> {
        if nodes.len() < THRESHOLD {
            return Err(Error::NotEnoughNodes {
                have: nodes.len(),
                need: THRESHOLD,
            });
        }

        Ok(Self {
            nodes,
            #[cfg(feature = "network")]
            http: reqwest::Client::new(),
        })
    }

    /// default rotko network nodes
    pub fn rotko_mainnet() -> Result<Self> {
        Self::new(vec![
            RealmNode {
                url: "https://realm1.rotko.net".into(),
                pubkey: [0u8; 32], // TODO: real pubkey
                index: 1,
            },
            RealmNode {
                url: "https://realm2.rotko.net".into(),
                pubkey: [0u8; 32],
                index: 2,
            },
            RealmNode {
                url: "https://realm3.rotko.net".into(),
                pubkey: [0u8; 32],
                index: 3,
            },
        ])
    }

    /// local dev nodes
    pub fn localhost() -> Result<Self> {
        Self::new(vec![
            RealmNode {
                url: "http://localhost:3001".into(),
                pubkey: [0u8; 32],
                index: 1,
            },
            RealmNode {
                url: "http://localhost:3002".into(),
                pubkey: [0u8; 32],
                index: 2,
            },
            RealmNode {
                url: "http://localhost:3003".into(),
                pubkey: [0u8; 32],
                index: 3,
            },
        ])
    }

    /// register shares with all nodes
    #[cfg(feature = "network")]
    pub async fn register(
        &self,
        user_id: [u8; 32],
        unlock_tag: [u8; 16],
        shares: &[Share; 3],
        allowed_guesses: u32,
    ) -> Result<Vec<RegisterResponse>> {
        use futures::future::join_all;

        let futures: Vec<_> = self.nodes.iter().zip(shares.iter()).map(|(node, share)| {
            let req = RegisterRequest {
                user_id,
                unlock_tag,
                encrypted_share: share.data.clone(),
                allowed_guesses,
            };
            self.register_one(node, req)
        }).collect();

        let results = join_all(futures).await;

        let mut responses = Vec::new();
        let mut errors = Vec::new();

        for result in results {
            match result {
                Ok(resp) => responses.push(resp),
                Err(e) => errors.push(e),
            }
        }

        // need all 3 for registration
        if responses.len() < 3 {
            return Err(Error::RegistrationFailed(format!(
                "only {} of 3 nodes succeeded: {:?}",
                responses.len(),
                errors
            )));
        }

        Ok(responses)
    }

    #[cfg(feature = "network")]
    async fn register_one(&self, node: &RealmNode, req: RegisterRequest) -> Result<RegisterResponse> {
        let resp = self.http
            .post(format!("{}/register", node.url))
            .json(&req)
            .send()
            .await
            .map_err(|e| Error::NetworkError(e.to_string()))?;

        resp.json()
            .await
            .map_err(|e| Error::NetworkError(e.to_string()))
    }

    /// recover shares from nodes (need 2 of 3)
    #[cfg(feature = "network")]
    pub async fn recover(
        &self,
        user_id: [u8; 32],
        unlock_tag: [u8; 16],
    ) -> Result<Vec<Share>> {
        use futures::future::join_all;

        let req = RecoverRequest { user_id, unlock_tag };

        let futures: Vec<_> = self.nodes.iter().map(|node| {
            self.recover_one(node, req.clone())
        }).collect();

        let results = join_all(futures).await;

        let mut shares = Vec::new();
        let mut errors = Vec::new();

        for result in results {
            match result {
                Ok(resp) if resp.ok => {
                    if let Some(share) = resp.share {
                        shares.push(share);
                    }
                }
                Ok(resp) => {
                    if let Some(err) = resp.error {
                        errors.push(Error::RecoveryFailed(err));
                    }
                }
                Err(e) => errors.push(e),
            }
        }

        // need at least 2 shares
        if shares.len() < THRESHOLD {
            return Err(Error::NotEnoughShares {
                have: shares.len(),
                need: THRESHOLD,
            });
        }

        Ok(shares)
    }

    #[cfg(feature = "network")]
    async fn recover_one(&self, node: &RealmNode, req: RecoverRequest) -> Result<RecoverResponse> {
        let resp = self.http
            .post(format!("{}/recover", node.url))
            .json(&req)
            .send()
            .await
            .map_err(|e| Error::NetworkError(e.to_string()))?;

        resp.json()
            .await
            .map_err(|e| Error::NetworkError(e.to_string()))
    }

    /// check account status
    #[cfg(feature = "network")]
    pub async fn status(&self, user_id: [u8; 32]) -> Result<AccountNetworkStatus> {
        // query first available node
        for node in &self.nodes {
            match self.status_one(node, user_id).await {
                Ok(status) => return Ok(status),
                Err(_) => continue,
            }
        }
        Err(Error::NetworkError("all nodes unreachable".into()))
    }

    #[cfg(feature = "network")]
    async fn status_one(&self, node: &RealmNode, user_id: [u8; 32]) -> Result<AccountNetworkStatus> {
        let resp = self.http
            .get(format!("{}/status/{}", node.url, hex::encode(user_id)))
            .send()
            .await
            .map_err(|e| Error::NetworkError(e.to_string()))?;

        resp.json()
            .await
            .map_err(|e| Error::NetworkError(e.to_string()))
    }

    /// get node count
    pub fn node_count(&self) -> usize {
        self.nodes.len()
    }
}

/// account status from network
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct AccountNetworkStatus {
    pub registered: bool,
    pub guesses_remaining: u32,
    pub locked: bool,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_localhost_client() {
        let client = NetworkClient::localhost().unwrap();
        assert_eq!(client.node_count(), 3);
    }
}
