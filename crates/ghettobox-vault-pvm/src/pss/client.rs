//! reshare HTTP client for provider-to-provider communication
//!
//! makes authenticated requests to other providers during reshare:
//! - broadcast commitments
//! - collect subshares
//! - coordinate epoch state

use super::config::{NetworkConfig, ProviderAddr, ProviderConfig};
use super::http::{
    CommitmentSubmitResponse, EpochResponse, ReshareStatusResponse, StartEpochRequest,
    VerifyResponse,
};
use super::reshare::{AggregatorState, CommitmentMsg, ReshareEpoch, ReshareError, SubShareMsg};
use curve25519_dalek::ristretto::RistrettoPoint;
use ed25519_dalek::{Signature, Signer, SigningKey, VerifyingKey};
use reqwest::Client;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::time::Duration;

/// signed request envelope for provider-to-provider communication
#[derive(Debug, Serialize, Deserialize)]
pub struct SignedRequest<T> {
    /// the actual payload
    pub payload: T,
    /// unix timestamp (seconds)
    pub timestamp: u64,
    /// sender's provider index
    pub sender_index: u32,
    /// ed25519 signature over (payload || timestamp || sender_index)
    pub signature: String,
}

impl<T: Serialize> SignedRequest<T> {
    pub fn new(payload: T, sender_index: u32, signing_key: &SigningKey) -> Self {
        let timestamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs();

        let payload_bytes = serde_json::to_vec(&payload).unwrap();
        let mut msg = payload_bytes;
        msg.extend_from_slice(&timestamp.to_le_bytes());
        msg.extend_from_slice(&sender_index.to_le_bytes());

        let signature = signing_key.sign(&msg);

        Self {
            payload,
            timestamp,
            sender_index,
            signature: hex::encode(signature.to_bytes()),
        }
    }
}

impl<T: for<'de> Deserialize<'de> + Serialize> SignedRequest<T> {
    /// verify signature against a known pubkey
    pub fn verify(&self, pubkey: &VerifyingKey, max_age_secs: u64) -> Result<(), ClientError> {
        // check timestamp freshness
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs();

        if now.saturating_sub(self.timestamp) > max_age_secs {
            return Err(ClientError::RequestExpired);
        }

        // reconstruct message
        let payload_bytes = serde_json::to_vec(&self.payload)
            .map_err(|e| ClientError::Serialization(e.to_string()))?;
        let mut msg = payload_bytes;
        msg.extend_from_slice(&self.timestamp.to_le_bytes());
        msg.extend_from_slice(&self.sender_index.to_le_bytes());

        // verify signature
        let sig_bytes = hex::decode(&self.signature)
            .map_err(|_| ClientError::InvalidSignature)?;
        let sig_arr: [u8; 64] = sig_bytes
            .try_into()
            .map_err(|_| ClientError::InvalidSignature)?;
        let signature = Signature::from_bytes(&sig_arr);

        pubkey
            .verify_strict(&msg, &signature)
            .map_err(|_| ClientError::InvalidSignature)
    }
}

/// HTTP client for reshare coordination
pub struct ReshareClient {
    http: Client,
    /// our provider config
    config: ProviderConfig,
    /// network topology
    network: NetworkConfig,
    /// ed25519 signing key
    signing_key: SigningKey,
    /// peer pubkeys for verification (index -> pubkey)
    peer_pubkeys: HashMap<u32, VerifyingKey>,
}

impl ReshareClient {
    pub fn new(
        config: ProviderConfig,
        network: NetworkConfig,
        signing_key: SigningKey,
    ) -> Result<Self, ClientError> {
        let http = Client::builder()
            .timeout(Duration::from_secs(30))
            .build()
            .map_err(|e| ClientError::HttpError(e.to_string()))?;

        // parse peer pubkeys
        let mut peer_pubkeys = HashMap::new();
        for peer in &network.providers {
            let pubkey = parse_verifying_key(&peer.pubkey)?;
            peer_pubkeys.insert(peer.index, pubkey);
        }

        Ok(Self {
            http,
            config,
            network,
            signing_key,
            peer_pubkeys,
        })
    }

    /// get peer address by index
    fn peer_addr(&self, index: u32) -> Option<&ProviderAddr> {
        self.network.providers.iter().find(|p| p.index == index)
    }

    /// get reshare status from a peer
    pub async fn get_status(&self, peer_index: u32) -> Result<ReshareStatusResponse, ClientError> {
        let peer = self.peer_addr(peer_index).ok_or(ClientError::UnknownPeer)?;
        let url = format!("http://{}/reshare/status", peer.addr);

        let resp = self
            .http
            .get(&url)
            .send()
            .await
            .map_err(|e| ClientError::HttpError(e.to_string()))?;

        resp.json()
            .await
            .map_err(|e| ClientError::Serialization(e.to_string()))
    }

    /// start reshare epoch on a peer
    pub async fn start_epoch(
        &self,
        peer_index: u32,
        epoch: u64,
        old_threshold: u32,
        new_threshold: u32,
        old_provider_count: u32,
        new_provider_count: u32,
    ) -> Result<EpochResponse, ClientError> {
        let peer = self.peer_addr(peer_index).ok_or(ClientError::UnknownPeer)?;
        let url = format!("http://{}/reshare/epoch", peer.addr);

        let req = StartEpochRequest {
            epoch,
            old_threshold,
            new_threshold,
            old_provider_count,
            new_provider_count,
        };

        let signed = SignedRequest::new(req, self.config.index, &self.signing_key);

        let resp = self
            .http
            .post(&url)
            .json(&signed)
            .send()
            .await
            .map_err(|e| ClientError::HttpError(e.to_string()))?;

        resp.json()
            .await
            .map_err(|e| ClientError::Serialization(e.to_string()))
    }

    /// broadcast start epoch to all peers
    pub async fn broadcast_start_epoch(
        &self,
        epoch: u64,
        old_threshold: u32,
        new_threshold: u32,
        old_provider_count: u32,
        new_provider_count: u32,
    ) -> Vec<(u32, Result<EpochResponse, ClientError>)> {
        let mut results = Vec::new();

        for peer in &self.network.providers {
            if peer.index == self.config.index {
                continue; // skip self
            }

            let result = self
                .start_epoch(
                    peer.index,
                    epoch,
                    old_threshold,
                    new_threshold,
                    old_provider_count,
                    new_provider_count,
                )
                .await;

            results.push((peer.index, result));
        }

        results
    }

    /// get epoch state from a peer
    pub async fn get_epoch(&self, peer_index: u32) -> Result<EpochResponse, ClientError> {
        let peer = self.peer_addr(peer_index).ok_or(ClientError::UnknownPeer)?;
        let url = format!("http://{}/reshare/epoch", peer.addr);

        let resp = self
            .http
            .get(&url)
            .send()
            .await
            .map_err(|e| ClientError::HttpError(e.to_string()))?;

        resp.json()
            .await
            .map_err(|e| ClientError::Serialization(e.to_string()))
    }

    /// submit commitment to a peer
    pub async fn submit_commitment(
        &self,
        peer_index: u32,
        commitment: &CommitmentMsg,
    ) -> Result<CommitmentSubmitResponse, ClientError> {
        let peer = self.peer_addr(peer_index).ok_or(ClientError::UnknownPeer)?;
        let url = format!("http://{}/reshare/commitment", peer.addr);

        let signed = SignedRequest::new(commitment.clone(), self.config.index, &self.signing_key);

        let resp = self
            .http
            .post(&url)
            .json(&signed)
            .send()
            .await
            .map_err(|e| ClientError::HttpError(e.to_string()))?;

        resp.json()
            .await
            .map_err(|e| ClientError::Serialization(e.to_string()))
    }

    /// broadcast commitment to all peers
    pub async fn broadcast_commitment(
        &self,
        commitment: &CommitmentMsg,
    ) -> Vec<(u32, Result<CommitmentSubmitResponse, ClientError>)> {
        let mut results = Vec::new();

        for peer in &self.network.providers {
            if peer.index == self.config.index {
                continue; // skip self
            }

            let result = self.submit_commitment(peer.index, commitment).await;
            results.push((peer.index, result));
        }

        results
    }

    /// get commitment from a peer (for dealers)
    pub async fn get_commitment(&self, peer_index: u32) -> Result<CommitmentMsg, ClientError> {
        let peer = self.peer_addr(peer_index).ok_or(ClientError::UnknownPeer)?;
        let url = format!("http://{}/reshare/commitment", peer.addr);

        let resp = self
            .http
            .get(&url)
            .send()
            .await
            .map_err(|e| ClientError::HttpError(e.to_string()))?;

        if !resp.status().is_success() {
            return Err(ClientError::HttpError(format!(
                "status {}",
                resp.status()
            )));
        }

        resp.json()
            .await
            .map_err(|e| ClientError::Serialization(e.to_string()))
    }

    /// collect commitments from all old providers
    pub async fn collect_commitments(
        &self,
        old_provider_indices: &[u32],
    ) -> Vec<(u32, Result<CommitmentMsg, ClientError>)> {
        let mut results = Vec::new();

        for &idx in old_provider_indices {
            let result = self.get_commitment(idx).await;
            results.push((idx, result));
        }

        results
    }

    /// get subshare from a dealer
    pub async fn get_subshare(
        &self,
        dealer_index: u32,
        player_index: u32,
    ) -> Result<SubShareMsg, ClientError> {
        let peer = self.peer_addr(dealer_index).ok_or(ClientError::UnknownPeer)?;
        let url = format!("http://{}/reshare/subshare/{}", peer.addr, player_index);

        let resp = self
            .http
            .get(&url)
            .send()
            .await
            .map_err(|e| ClientError::HttpError(e.to_string()))?;

        if !resp.status().is_success() {
            return Err(ClientError::HttpError(format!(
                "status {}",
                resp.status()
            )));
        }

        resp.json()
            .await
            .map_err(|e| ClientError::Serialization(e.to_string()))
    }

    /// collect subshares from all dealers for our player index
    pub async fn collect_subshares(
        &self,
        dealer_indices: &[u32],
        player_index: u32,
    ) -> Vec<(u32, Result<SubShareMsg, ClientError>)> {
        let mut results = Vec::new();

        for &dealer_idx in dealer_indices {
            let result = self.get_subshare(dealer_idx, player_index).await;
            results.push((dealer_idx, result));
        }

        results
    }

    /// verify group key at a peer
    pub async fn verify_group_key(&self, peer_index: u32) -> Result<VerifyResponse, ClientError> {
        let peer = self.peer_addr(peer_index).ok_or(ClientError::UnknownPeer)?;
        let url = format!("http://{}/reshare/verify", peer.addr);

        let resp = self
            .http
            .get(&url)
            .send()
            .await
            .map_err(|e| ClientError::HttpError(e.to_string()))?;

        resp.json()
            .await
            .map_err(|e| ClientError::Serialization(e.to_string()))
    }

    /// run full reshare protocol as aggregator
    ///
    /// 1. collect commitments from all old providers
    /// 2. collect subshares from dealers
    /// 3. aggregate into new share
    pub async fn run_aggregator(
        &self,
        epoch: &ReshareEpoch,
        group_pubkey: &RistrettoPoint,
    ) -> Result<curve25519_dalek::scalar::Scalar, ClientError> {
        let old_indices: Vec<u32> = (1..=epoch.old_provider_count).collect();

        // collect commitments
        let commitment_results = self.collect_commitments(&old_indices).await;

        let mut commitments: HashMap<u32, CommitmentMsg> = HashMap::new();
        for (idx, result) in commitment_results {
            match result {
                Ok(c) => {
                    commitments.insert(idx, c);
                }
                Err(e) => {
                    log::warn!("failed to get commitment from dealer {}: {:?}", idx, e);
                }
            }
        }

        if commitments.len() < epoch.old_threshold as usize {
            return Err(ClientError::InsufficientResponses {
                got: commitments.len(),
                need: epoch.old_threshold as usize,
            });
        }

        // create aggregator state
        let mut aggregator =
            AggregatorState::new(self.config.index, epoch.old_threshold, *group_pubkey);

        // collect subshares from each dealer we got commitment from
        let dealer_indices: Vec<u32> = commitments.keys().copied().collect();
        let subshare_results = self
            .collect_subshares(&dealer_indices, self.config.index)
            .await;

        for (dealer_idx, result) in subshare_results {
            match result {
                Ok(subshare) => {
                    if let Some(commitment) = commitments.get(&dealer_idx) {
                        if let Err(e) = aggregator.add_subshare(&subshare, commitment) {
                            log::warn!(
                                "failed to add subshare from dealer {}: {:?}",
                                dealer_idx,
                                e
                            );
                        }
                    }
                }
                Err(e) => {
                    log::warn!("failed to get subshare from dealer {}: {:?}", dealer_idx, e);
                }
            }
        }

        if !aggregator.has_threshold() {
            return Err(ClientError::InsufficientResponses {
                got: aggregator.count(),
                need: epoch.old_threshold as usize,
            });
        }

        // finalize
        aggregator
            .finalize()
            .map_err(|e| ClientError::ReshareError(e))
    }
}

fn parse_verifying_key(hex_str: &str) -> Result<VerifyingKey, ClientError> {
    let bytes = hex::decode(hex_str).map_err(|_| ClientError::InvalidPubkey)?;
    let arr: [u8; 32] = bytes.try_into().map_err(|_| ClientError::InvalidPubkey)?;
    VerifyingKey::from_bytes(&arr).map_err(|_| ClientError::InvalidPubkey)
}

#[derive(Debug)]
pub enum ClientError {
    HttpError(String),
    Serialization(String),
    UnknownPeer,
    InvalidPubkey,
    InvalidSignature,
    RequestExpired,
    InsufficientResponses { got: usize, need: usize },
    ReshareError(ReshareError),
}

impl std::fmt::Display for ClientError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::HttpError(e) => write!(f, "http error: {}", e),
            Self::Serialization(e) => write!(f, "serialization error: {}", e),
            Self::UnknownPeer => write!(f, "unknown peer"),
            Self::InvalidPubkey => write!(f, "invalid pubkey"),
            Self::InvalidSignature => write!(f, "invalid signature"),
            Self::RequestExpired => write!(f, "request expired"),
            Self::InsufficientResponses { got, need } => {
                write!(f, "insufficient responses: got {}, need {}", got, need)
            }
            Self::ReshareError(e) => write!(f, "reshare error: {}", e),
        }
    }
}

impl std::error::Error for ClientError {}

#[cfg(test)]
mod tests {
    use super::*;
    use rand::rngs::OsRng;

    #[test]
    fn test_signed_request_roundtrip() {
        let mut rng = OsRng;
        let signing_key = SigningKey::generate(&mut rng);
        let verifying_key = signing_key.verifying_key();

        let payload = CommitmentMsg {
            dealer_index: 1,
            threshold: 3,
            coefficients: vec!["abc123".into(), "def456".into()],
        };

        let signed = SignedRequest::new(payload, 1, &signing_key);

        // should verify with correct key
        assert!(signed.verify(&verifying_key, 60).is_ok());
    }

    #[test]
    fn test_signed_request_wrong_key() {
        let mut rng = OsRng;
        let signing_key = SigningKey::generate(&mut rng);
        let wrong_key = SigningKey::generate(&mut rng);

        let payload = CommitmentMsg {
            dealer_index: 1,
            threshold: 3,
            coefficients: vec!["abc123".into()],
        };

        let signed = SignedRequest::new(payload, 1, &signing_key);

        // should fail with wrong key
        assert!(signed.verify(&wrong_key.verifying_key(), 60).is_err());
    }

    #[test]
    fn test_parse_verifying_key() {
        let mut rng = OsRng;
        let signing_key = SigningKey::generate(&mut rng);
        let pubkey_hex = hex::encode(signing_key.verifying_key().as_bytes());

        let parsed = parse_verifying_key(&pubkey_hex).unwrap();
        assert_eq!(parsed, signing_key.verifying_key());
    }
}
