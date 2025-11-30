//! Zanchor parachain client for trustless Zcash attestations
//!
//! Connects to the zanchor Polkadot parachain to:
//! 1. Fetch finalized block attestations
//! 2. Submit attestations as a relayer (if enabled)
//! 3. Monitor attestation status
//!
//! ## Per-Block Attestation Model
//!
//! Each Zcash block (75s) gets attested individually.
//! When N relayers agree → block finalized.
//! Light clients query finalized state.

use crate::error::{Result, ZidecarError};
use serde::{Deserialize, Serialize};
use sp_core::{sr25519, Pair};
use subxt::{OnlineClient, PolkadotConfig};
use subxt::tx::Signer;
use tracing::{debug, error, info, warn};

/// Zanchor RPC endpoint (parachain)
const DEFAULT_ZANCHOR_RPC: &str = "ws://127.0.0.1:19944";

/// Block attestation data to submit
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BlockAttestationData {
    /// Zcash block height
    pub height: u32,
    /// Block hash
    pub block_hash: [u8; 32],
    /// Previous block hash
    pub prev_hash: [u8; 32],
    /// Orchard note commitment tree root
    pub orchard_root: [u8; 32],
    /// Sapling note commitment tree root
    pub sapling_root: [u8; 32],
}

/// Finalized block from zanchor
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ZanchorFinalizedBlock {
    /// Zcash block height
    pub height: u32,
    /// Block hash
    pub block_hash: [u8; 32],
    /// Orchard tree root
    pub orchard_root: [u8; 32],
    /// Sapling tree root
    pub sapling_root: [u8; 32],
    /// Number of relayers who attested
    pub attester_count: u32,
    /// Parachain block when finalized
    pub finalized_at: u32,
}

/// Subxt signer wrapper for sr25519
pub struct SubxtSigner {
    pair: sr25519::Pair,
    account_id: subxt::utils::AccountId32,
}

impl SubxtSigner {
    pub fn new(pair: sr25519::Pair) -> Self {
        let account_id = subxt::utils::AccountId32::from(pair.public().0);
        Self { pair, account_id }
    }
}

impl Signer<PolkadotConfig> for SubxtSigner {
    fn account_id(&self) -> subxt::utils::AccountId32 {
        self.account_id.clone()
    }

    fn address(&self) -> <PolkadotConfig as subxt::Config>::Address {
        self.account_id.clone().into()
    }

    fn sign(&self, payload: &[u8]) -> <PolkadotConfig as subxt::Config>::Signature {
        let sig = self.pair.sign(payload);
        subxt::utils::MultiSignature::Sr25519(sig.0)
    }
}

/// Zanchor client for interacting with the parachain
pub struct ZanchorClient {
    /// Subxt client (None if not connected)
    client: Option<OnlineClient<PolkadotConfig>>,
    /// RPC endpoint URL
    rpc_url: String,
    /// Whether relayer mode is enabled
    relayer_enabled: bool,
    /// Relayer keypair (if enabled)
    relayer_signer: Option<SubxtSigner>,
    /// Last attested height (to avoid duplicates)
    last_attested_height: u32,
}

impl ZanchorClient {
    /// Create new client (not connected yet)
    pub fn new(rpc_url: Option<&str>) -> Self {
        Self {
            client: None,
            rpc_url: rpc_url.unwrap_or(DEFAULT_ZANCHOR_RPC).to_string(),
            relayer_enabled: false,
            relayer_signer: None,
            last_attested_height: 0,
        }
    }

    /// Enable relayer mode with seed phrase
    pub fn with_relayer(mut self, seed: String) -> Self {
        match sr25519::Pair::from_string(&seed, None) {
            Ok(pair) => {
                let account = subxt::utils::AccountId32::from(pair.public().0);
                info!("relayer account: {}", account);
                self.relayer_signer = Some(SubxtSigner::new(pair));
                self.relayer_enabled = true;
            }
            Err(e) => {
                error!("failed to parse relayer seed: {:?}", e);
            }
        }
        self
    }

    /// Connect to zanchor parachain
    pub async fn connect(&mut self) -> Result<()> {
        info!("connecting to zanchor at {}", self.rpc_url);

        match OnlineClient::<PolkadotConfig>::from_url(&self.rpc_url).await {
            Ok(client) => {
                // Get chain info
                let genesis = client.genesis_hash();
                let runtime_version = client.runtime_version();

                info!("connected to zanchor");
                info!("  genesis: {:?}", genesis);
                info!("  spec version: {}", runtime_version.spec_version);

                self.client = Some(client);
                Ok(())
            }
            Err(e) => {
                warn!("failed to connect to zanchor: {}", e);
                Err(ZidecarError::Network(format!(
                    "zanchor connection failed: {}",
                    e
                )))
            }
        }
    }

    /// Check if connected
    pub fn is_connected(&self) -> bool {
        self.client.is_some()
    }

    /// Get latest finalized Zcash height from zanchor
    pub async fn get_latest_finalized_height(&self) -> Result<Option<u32>> {
        let client = self
            .client
            .as_ref()
            .ok_or_else(|| ZidecarError::Network("not connected to zanchor".into()))?;

        // Query LatestFinalizedHeight storage
        let storage_key = Self::storage_key(b"ZcashLight", b"LatestFinalizedHeight");

        match client
            .storage()
            .at_latest()
            .await
            .map_err(|e| ZidecarError::Network(e.to_string()))?
            .fetch_raw(storage_key)
            .await
        {
            Ok(Some(data)) => {
                if data.len() >= 4 {
                    let height = u32::from_le_bytes([data[0], data[1], data[2], data[3]]);
                    Ok(Some(height))
                } else {
                    Ok(None)
                }
            }
            Ok(None) => Ok(None),
            Err(e) => {
                debug!("failed to fetch latest height: {}", e);
                Ok(None)
            }
        }
    }

    /// Get finalized block by height
    pub async fn get_finalized_block(&self, height: u32) -> Result<Option<ZanchorFinalizedBlock>> {
        let client = self
            .client
            .as_ref()
            .ok_or_else(|| ZidecarError::Network("not connected to zanchor".into()))?;

        let storage_key = Self::finalized_block_key(height);

        match client
            .storage()
            .at_latest()
            .await
            .map_err(|e| ZidecarError::Network(e.to_string()))?
            .fetch_raw(storage_key)
            .await
        {
            Ok(Some(data)) => Self::decode_finalized_block(&data, height),
            Ok(None) => Ok(None),
            Err(e) => {
                debug!("failed to fetch block {}: {}", height, e);
                Ok(None)
            }
        }
    }

    /// Submit attestation for a Zcash block (relayer mode only)
    pub async fn submit_attestation(&mut self, attestation: BlockAttestationData) -> Result<()> {
        if !self.relayer_enabled {
            return Err(ZidecarError::Validation("relayer mode not enabled".into()));
        }

        let client = self
            .client
            .as_ref()
            .ok_or_else(|| ZidecarError::Network("not connected to zanchor".into()))?;

        let signer = self
            .relayer_signer
            .as_ref()
            .ok_or_else(|| ZidecarError::Validation("no relayer signer configured".into()))?;

        // Skip if already attested to this height
        if attestation.height <= self.last_attested_height {
            debug!("already attested to height {}", attestation.height);
            return Ok(());
        }

        info!(
            "submitting attestation for height {} (hash: {})",
            attestation.height,
            hex::encode(&attestation.block_hash[..8])
        );

        // Build the call data for ZcashLight::submit_attestation
        // Call index 2, params: height, block_hash, prev_hash, orchard_root, sapling_root
        let mut call_data = Vec::new();

        // Pallet index 50, call index 2
        call_data.push(50u8);
        call_data.push(2u8);

        // height: u32
        call_data.extend_from_slice(&attestation.height.to_le_bytes());
        // block_hash: [u8; 32]
        call_data.extend_from_slice(&attestation.block_hash);
        // prev_hash: [u8; 32]
        call_data.extend_from_slice(&attestation.prev_hash);
        // orchard_root: [u8; 32]
        call_data.extend_from_slice(&attestation.orchard_root);
        // sapling_root: [u8; 32]
        call_data.extend_from_slice(&attestation.sapling_root);

        // Submit as dynamic/raw call
        let tx = subxt::dynamic::tx("ZcashLight", "submit_attestation", vec![
            subxt::dynamic::Value::u128(attestation.height as u128),
            subxt::dynamic::Value::from_bytes(attestation.block_hash),
            subxt::dynamic::Value::from_bytes(attestation.prev_hash),
            subxt::dynamic::Value::from_bytes(attestation.orchard_root),
            subxt::dynamic::Value::from_bytes(attestation.sapling_root),
        ]);

        match client
            .tx()
            .sign_and_submit_then_watch_default(&tx, signer)
            .await
        {
            Ok(progress) => {
                match progress.wait_for_finalized_success().await {
                    Ok(_events) => {
                        info!(
                            "attestation submitted for height {}",
                            attestation.height
                        );
                        self.last_attested_height = attestation.height;
                    }
                    Err(e) => {
                        warn!("attestation tx failed: {}", e);
                        return Err(ZidecarError::Network(format!("tx failed: {}", e)));
                    }
                }
            }
            Err(e) => {
                warn!("failed to submit attestation: {}", e);
                return Err(ZidecarError::Network(format!("submit failed: {}", e)));
            }
        }

        Ok(())
    }

    /// Build storage key from pallet and item names
    fn storage_key(pallet: &[u8], item: &[u8]) -> Vec<u8> {
        let mut key = Vec::new();

        // Hash pallet name (twox128)
        let pallet_hash = sp_core::twox_128(pallet);
        key.extend_from_slice(&pallet_hash);

        // Hash item name (twox128)
        let item_hash = sp_core::twox_128(item);
        key.extend_from_slice(&item_hash);

        key
    }

    /// Build storage key for FinalizedBlocks(height)
    fn finalized_block_key(height: u32) -> Vec<u8> {
        let mut key = Self::storage_key(b"ZcashLight", b"FinalizedBlocks");

        // Blake2_128Concat: hash + raw key
        let height_bytes = height.to_le_bytes();
        let hash = sp_core::blake2_128(&height_bytes);
        key.extend_from_slice(&hash);
        key.extend_from_slice(&height_bytes);

        key
    }

    /// Decode FinalizedBlock from raw bytes
    fn decode_finalized_block(
        data: &[u8],
        height: u32,
    ) -> Result<Option<ZanchorFinalizedBlock>> {
        // FinalizedBlock layout (SCALE encoded):
        // - block_hash: [u8; 32]
        // - prev_hash: [u8; 32]
        // - orchard_root: [u8; 32]
        // - sapling_root: [u8; 32]
        // - attester_count: u32
        // - finalized_at: u32

        let min_size = 32 * 4 + 4 + 4; // 136 bytes
        if data.len() < min_size {
            return Ok(None);
        }

        let mut offset = 0;

        let block_hash: [u8; 32] = data[offset..offset + 32].try_into().unwrap();
        offset += 32;

        let _prev_hash: [u8; 32] = data[offset..offset + 32].try_into().unwrap();
        offset += 32;

        let orchard_root: [u8; 32] = data[offset..offset + 32].try_into().unwrap();
        offset += 32;

        let sapling_root: [u8; 32] = data[offset..offset + 32].try_into().unwrap();
        offset += 32;

        let attester_count = u32::from_le_bytes(data[offset..offset + 4].try_into().unwrap());
        offset += 4;

        let finalized_at = u32::from_le_bytes(data[offset..offset + 4].try_into().unwrap());

        Ok(Some(ZanchorFinalizedBlock {
            height,
            block_hash,
            orchard_root,
            sapling_root,
            attester_count,
            finalized_at,
        }))
    }
}

// Keep old types for backward compat
pub type ZanchorFinalizedEpoch = ZanchorFinalizedBlock;
pub type AttestationData = BlockAttestationData;

/// Checkpoint source for hybrid verification
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CheckpointSource {
    /// From FROST threshold signature
    Frost,
    /// From zanchor parachain finality
    Polkadot,
    /// Verified by both
    Both,
}

/// Checkpoint with source information
#[derive(Debug, Clone)]
pub struct SourcedCheckpoint {
    pub checkpoint: crate::checkpoint::EpochCheckpoint,
    pub source: CheckpointSource,
}

impl SourcedCheckpoint {
    /// Check if this checkpoint has the required trust level
    pub fn meets_trust_level(&self, required: CheckpointSource) -> bool {
        match required {
            CheckpointSource::Frost => {
                self.source == CheckpointSource::Frost || self.source == CheckpointSource::Both
            }
            CheckpointSource::Polkadot => {
                self.source == CheckpointSource::Polkadot || self.source == CheckpointSource::Both
            }
            CheckpointSource::Both => self.source == CheckpointSource::Both,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_storage_key_generation() {
        let key = ZanchorClient::storage_key(b"ZcashLight", b"LatestFinalizedHeight");
        assert_eq!(key.len(), 32); // twox128 + twox128
    }

    #[test]
    fn test_finalized_block_key() {
        let key = ZanchorClient::finalized_block_key(12345);
        // twox128 + twox128 + blake2_128 + u32
        assert_eq!(key.len(), 32 + 16 + 4);
    }
}
