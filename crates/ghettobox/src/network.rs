//! network client for distributed TPM nodes
//!
//! talks to 3 realm nodes that each hold a VSS share sealed to their TPM
//!
//! supports two protocols:
//! - legacy VSS: simple share distribution (deprecated)
//! - verified OPRF: threshold OPRF with DLEQ proofs (production)

use crate::vss::{Share, THRESHOLD};
use crate::zoda_oprf::{ServerPublicKey, VerifiedOprfResponse, MisbehaviorReport};
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

// ============================================================================
// OPRF Network Protocol (verified with DLEQ proofs)
// ============================================================================

/// OPRF realm node with public key for verification
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct OprfRealmNode {
    /// node url
    pub url: String,
    /// OPRF public key (ristretto255 point, compressed)
    pub oprf_pubkey: ServerPublicKey,
    /// node index (0-2)
    pub index: u8,
}

/// OPRF registration request
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct OprfRegisterRequest {
    /// user identifier (email hash)
    pub user_id: String,
    /// blinded element for OPRF evaluation (compressed point)
    pub blinded: [u8; 32],
    /// encrypted seed (after OPRF key derivation)
    pub encrypted_seed: Vec<u8>,
    /// allowed PIN guesses before lockout
    pub allowed_guesses: u32,
}

/// OPRF registration response
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct OprfRegisterResponse {
    /// success
    pub ok: bool,
    /// evaluated OPRF response with DLEQ proof
    pub response: Option<VerifiedOprfResponse>,
    /// error message if failed
    pub error: Option<String>,
}

/// OPRF recovery request
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct OprfRecoverRequest {
    /// user identifier
    pub user_id: String,
    /// blinded element for OPRF evaluation (compressed point)
    pub blinded: [u8; 32],
}

/// OPRF recovery response
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct OprfRecoverResponse {
    /// success
    pub ok: bool,
    /// evaluated OPRF response with DLEQ proof
    pub response: Option<VerifiedOprfResponse>,
    /// encrypted seed (stored during registration)
    pub encrypted_seed: Option<Vec<u8>>,
    /// remaining guesses
    pub guesses_remaining: u32,
    /// error message if failed
    pub error: Option<String>,
}

/// OPRF network client with verification
pub struct OprfNetworkClient {
    nodes: Vec<OprfRealmNode>,
    threshold: usize,
    #[cfg(feature = "network")]
    http: reqwest::Client,
}

impl OprfNetworkClient {
    /// create client with OPRF realm nodes
    pub fn new(nodes: Vec<OprfRealmNode>, threshold: usize) -> Result<Self> {
        if nodes.len() < threshold {
            return Err(Error::NotEnoughNodes {
                have: nodes.len(),
                need: threshold,
            });
        }

        Ok(Self {
            nodes,
            threshold,
            #[cfg(feature = "network")]
            http: reqwest::Client::new(),
        })
    }

    /// get node public keys for client verification
    pub fn public_keys(&self) -> Vec<ServerPublicKey> {
        self.nodes.iter().map(|n| n.oprf_pubkey.clone()).collect()
    }

    /// get threshold
    pub fn threshold(&self) -> usize {
        self.threshold
    }

    /// register with OPRF protocol (returns raw responses, caller verifies)
    #[cfg(feature = "network")]
    pub async fn oprf_register(
        &self,
        user_id: &str,
        blinded: [u8; 32],
        encrypted_seed: Vec<u8>,
        allowed_guesses: u32,
    ) -> Result<Vec<VerifiedOprfResponse>> {
        use futures::future::join_all;

        let req = OprfRegisterRequest {
            user_id: user_id.to_string(),
            blinded,
            encrypted_seed,
            allowed_guesses,
        };

        let futures: Vec<_> = self.nodes.iter().map(|node| {
            self.oprf_register_one(node, req.clone())
        }).collect();

        let results = join_all(futures).await;

        let mut responses = Vec::new();
        let mut errors = Vec::new();

        for result in results {
            match result {
                Ok(resp) if resp.ok => {
                    if let Some(oprf_resp) = resp.response {
                        responses.push(oprf_resp);
                    }
                }
                Ok(resp) => {
                    if let Some(err) = resp.error {
                        errors.push(Error::RegistrationFailed(err));
                    }
                }
                Err(e) => errors.push(e),
            }
        }

        // need all nodes for registration
        if responses.len() < self.nodes.len() {
            return Err(Error::RegistrationFailed(format!(
                "only {} of {} nodes succeeded",
                responses.len(),
                self.nodes.len()
            )));
        }

        Ok(responses)
    }

    #[cfg(feature = "network")]
    async fn oprf_register_one(
        &self,
        node: &OprfRealmNode,
        req: OprfRegisterRequest,
    ) -> Result<OprfRegisterResponse> {
        let resp = self.http
            .post(format!("{}/oprf/register", node.url))
            .json(&req)
            .send()
            .await
            .map_err(|e| Error::NetworkError(e.to_string()))?;

        resp.json()
            .await
            .map_err(|e| Error::NetworkError(e.to_string()))
    }

    /// recover with OPRF protocol (returns raw data, caller verifies & finalizes)
    #[cfg(feature = "network")]
    pub async fn oprf_recover(
        &self,
        user_id: &str,
        blinded: [u8; 32],
    ) -> Result<OprfRecoverRawResult> {
        use futures::future::join_all;

        let req = OprfRecoverRequest {
            user_id: user_id.to_string(),
            blinded,
        };

        let futures: Vec<_> = self.nodes.iter().map(|node| {
            self.oprf_recover_one(node, req.clone())
        }).collect();

        let results = join_all(futures).await;

        let mut responses = Vec::new();
        let mut encrypted_seed = None;
        let mut min_guesses = u32::MAX;
        let mut errors = Vec::new();

        for result in results {
            match result {
                Ok(resp) if resp.ok => {
                    if let Some(oprf_resp) = resp.response {
                        responses.push(oprf_resp);
                    }
                    if encrypted_seed.is_none() {
                        encrypted_seed = resp.encrypted_seed;
                    }
                    min_guesses = min_guesses.min(resp.guesses_remaining);
                }
                Ok(resp) => {
                    if let Some(err) = resp.error {
                        errors.push(Error::RecoveryFailed(err));
                    }
                    min_guesses = min_guesses.min(resp.guesses_remaining);
                }
                Err(e) => errors.push(e),
            }
        }

        // need threshold responses
        if responses.len() < self.threshold {
            return Err(Error::NotEnoughShares {
                have: responses.len(),
                need: self.threshold,
            });
        }

        let encrypted_seed = encrypted_seed.ok_or_else(|| {
            Error::RecoveryFailed("no encrypted seed returned".into())
        })?;

        Ok(OprfRecoverRawResult {
            responses,
            encrypted_seed,
            guesses_remaining: min_guesses,
        })
    }

    #[cfg(feature = "network")]
    async fn oprf_recover_one(
        &self,
        node: &OprfRealmNode,
        req: OprfRecoverRequest,
    ) -> Result<OprfRecoverResponse> {
        let resp = self.http
            .post(format!("{}/oprf/recover", node.url))
            .json(&req)
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

/// raw OPRF recovery result from network (caller must verify)
#[derive(Clone, Debug)]
pub struct OprfRecoverRawResult {
    /// OPRF responses from servers (need to verify with VerifiedOprfClient)
    pub responses: Vec<VerifiedOprfResponse>,
    /// encrypted seed from registration
    pub encrypted_seed: Vec<u8>,
    /// remaining guess attempts
    pub guesses_remaining: u32,
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
