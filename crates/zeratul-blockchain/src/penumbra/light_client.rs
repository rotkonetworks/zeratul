//! Embedded Penumbra Light Client
//!
//! Each Zeratul validator runs an embedded Penumbra ViewServer to:
//! - Observe Penumbra DEX batch swap prices
//! - Verify IBC packet proofs
//! - Query Penumbra chain state
//!
//! Uses Penumbra SDK directly instead of running pclientd as separate process.

use anyhow::{bail, Result};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::RwLock;

use super::super::lending::types::{AssetId, Amount, Price};

// These would be the actual Penumbra SDK imports:
// use penumbra_view::{ViewServer, Storage};
// use penumbra_keys::FullViewingKey;
// use penumbra_dex::BatchSwapOutputData;
// use penumbra_asset::asset;
//
// For now, we'll define the interface that would use these

/// Configuration for embedded Penumbra light client
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PenumbraClientConfig {
    /// Directory for view database (SQLite storage)
    pub storage_path: PathBuf,

    /// Penumbra full node gRPC endpoint
    pub node_url: String,

    /// Full Viewing Key (for decrypting notes/swaps we care about)
    /// For oracle-only mode, can use a dummy FVK
    pub fvk_hex: Option<String>,

    /// Trading pairs to monitor for oracle prices
    pub oracle_pairs: Vec<(AssetId, AssetId)>,

    /// How often to update oracle prices (in blocks)
    pub oracle_update_interval: u64,
}

impl Default for PenumbraClientConfig {
    fn default() -> Self {
        Self {
            storage_path: PathBuf::from("/var/lib/zeratul/view.db"),
            node_url: "https://grpc.testnet.penumbra.zone:443".to_string(),
            fvk_hex: None, // View-only mode
            oracle_pairs: Vec::new(),
            oracle_update_interval: 10,
        }
    }
}

/// Embedded Penumbra light client
///
/// Embeds Penumbra ViewServer directly in the validator process.
///
/// In production this would use:
/// - penumbra_view::ViewServer for sync and queries
/// - penumbra_tct::Tree for state commitment tracking
/// - penumbra_keys::FullViewingKey for note decryption
pub struct EmbeddedPenumbraClient {
    /// Configuration
    config: PenumbraClientConfig,

    /// ViewServer handle (would be Arc<ViewServer> in real impl)
    /// For now, we simulate the interface
    view_server: Arc<RwLock<MockViewServer>>,

    /// Current Penumbra block height
    current_height: Arc<RwLock<u64>>,
}

/// Mock ViewServer interface (would be penumbra_view::ViewServer)
struct MockViewServer {
    node_url: String,
    storage_path: PathBuf,
    synced_height: u64,
}

impl EmbeddedPenumbraClient {
    /// Start embedded light client
    ///
    /// Initializes Penumbra ViewServer with:
    /// 1. Storage (SQLite database)
    /// 2. State Commitment Tree (in-memory)
    /// 3. Full Viewing Key (optional, for decryption)
    /// 4. gRPC connection to Penumbra node
    ///
    /// The ViewServer spawns a background sync worker that:
    /// - Fetches compact blocks
    /// - Scans for relevant notes/swaps
    /// - Updates state commitment tree
    /// - Stores to SQLite
    pub async fn start(config: PenumbraClientConfig) -> Result<Self> {
        // Ensure storage directory exists
        if let Some(parent) = config.storage_path.parent() {
            std::fs::create_dir_all(parent)?;
        }

        // In real implementation:
        // let fvk = if let Some(fvk_hex) = &config.fvk_hex {
        //     FullViewingKey::from_str(fvk_hex)?
        // } else {
        //     // Use dummy FVK for oracle-only mode
        //     FullViewingKey::dummy()
        // };
        //
        // let view_server = ViewServer::load_or_initialize(
        //     Some(&config.storage_path),
        //     None, // asset registry
        //     &fvk,
        //     url::Url::parse(&config.node_url)?,
        // ).await?;
        //
        // ViewServer spawns background sync worker automatically

        // For now, mock implementation
        let view_server = Arc::new(RwLock::new(MockViewServer {
            node_url: config.node_url.clone(),
            storage_path: config.storage_path.clone(),
            synced_height: 0,
        }));

        Ok(Self {
            config,
            view_server,
            current_height: Arc::new(RwLock::new(0)),
        })
    }

    /// Get latest Penumbra block height
    pub async fn get_latest_height(&self) -> Result<u64> {
        // In real implementation:
        // self.view_server.status().await?.full_sync_height

        let mut height = self.current_height.write().await;
        *height += 1;
        Ok(*height)
    }

    /// Wait for sync to reach target height
    pub async fn wait_for_sync(&self, target_height: u64) -> Result<()> {
        // In real implementation:
        // while self.view_server.status().await?.full_sync_height < target_height {
        //     tokio::time::sleep(Duration::from_secs(1)).await;
        // }

        Ok(())
    }

    /// Get oracle prices from Penumbra DEX batch swaps
    ///
    /// Queries the latest batch swap output data for configured trading pairs.
    /// This data is PUBLIC on Penumbra (batch aggregates, not individual swaps).
    ///
    /// In real implementation, this would:
    /// 1. Query CompactBlock via ViewServer
    /// 2. Extract swap_outputs map (BTreeMap<TradingPair, BatchSwapOutputData>)
    /// 3. Calculate clearing prices from batch data
    pub async fn get_oracle_prices(&self) -> Result<HashMap<(AssetId, AssetId), Price>> {
        let mut prices = HashMap::new();

        for (base, quote) in &self.config.oracle_pairs {
            // Query latest batch swap for this pair
            let batch_data = self.query_batch_swap(*base, *quote).await?;

            // Calculate clearing price from batch data
            let price = self.calculate_price_from_batch(&batch_data)?;

            prices.insert((*base, *quote), price);
        }

        Ok(prices)
    }

    /// Query batch swap output data from Penumbra DEX
    ///
    /// Returns the aggregate batch swap result (public data):
    /// - Total input/output amounts (delta_1, delta_2)
    /// - Pro-rata outputs (lambda_1, lambda_2)
    /// - Unfilled amounts
    ///
    /// In real implementation:
    /// ```rust,ignore
    /// let compact_block = self.view_server
    ///     .compact_block_range(CompactBlockRangeRequest {
    ///         start_height: height,
    ///         end_height: height + 1,
    ///         ..Default::default()
    ///     })
    ///     .await?
    ///     .next()
    ///     .await?;
    ///
    /// let trading_pair = TradingPair::new(base.into(), quote.into());
    /// let batch_data = compact_block
    ///     .swap_outputs
    ///     .get(&trading_pair)
    ///     .ok_or_else(|| anyhow!("no batch swap for pair"))?;
    /// ```
    async fn query_batch_swap(
        &self,
        _base: AssetId,
        _quote: AssetId,
    ) -> Result<BatchSwapOutputData> {
        let height = self.current_height.read().await;

        // Mock data for now
        Ok(BatchSwapOutputData {
            height: *height,
            delta_1: 1000000, // e.g., 1M UM
            delta_2: 1050000, // e.g., 1.05M gm
            lambda_1: 10000,
            lambda_2: 10000,
            unfilled_1: 0,
            unfilled_2: 0,
        })
    }

    /// Calculate clearing price from batch swap output
    ///
    /// Batch swap output contains:
    /// - delta_1, delta_2: Total amounts swapped
    /// - lambda_1, lambda_2: Pro-rata distribution parameters
    ///
    /// Price = delta_2 / delta_1 (quote per base)
    fn calculate_price_from_batch(&self, batch: &BatchSwapOutputData) -> Result<Price> {
        if batch.delta_1 == 0 {
            bail!("no swaps in batch");
        }

        // Calculate clearing price as a ratio (fixed point with denominator 1e18)
        let numerator = batch.delta_2 as u128;
        let denominator = batch.delta_1 as u128;

        Ok(Price { numerator, denominator })
    }

    /// Verify IBC packet proof against Penumbra light client
    ///
    /// Uses embedded ViewServer's consensus state to verify Merkle proof.
    /// This ensures the IBC transfer actually happened on Penumbra.
    ///
    /// In real implementation:
    /// ```rust,ignore
    /// // Get trusted consensus state at proof_height
    /// let consensus_state = self.view_server
    ///     .get_consensus_state(proof_height)
    ///     .await?;
    ///
    /// // Verify Merkle proof using ics23
    /// verify_merkle_proof(
    ///     &proof_specs,
    ///     &merkle_prefix,
    ///     merkle_proof,
    ///     &consensus_state.root,
    ///     IbcPath::packet_commitment(channel, sequence),
    ///     packet_commitment.to_vec(),
    /// )?;
    /// ```
    pub async fn verify_ibc_packet(
        &self,
        _packet_commitment: &[u8],
        _proof_height: u64,
        _merkle_proof: &[u8],
    ) -> Result<bool> {
        // Placeholder for now
        Ok(true)
    }

    /// Shutdown embedded light client
    pub async fn shutdown(&self) -> Result<()> {
        // In real implementation:
        // self.view_server.shutdown().await?;

        Ok(())
    }
}

/// Batch swap output data from Penumbra DEX
///
/// This is PUBLIC data revealed by Penumbra batch swaps.
/// Contains only aggregates, not individual swap details.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BatchSwapOutputData {
    /// Penumbra block height where batch executed
    pub height: u64,

    /// Total amount of asset 1 swapped
    pub delta_1: u64,

    /// Total amount of asset 2 swapped
    pub delta_2: u64,

    /// Pro-rata distribution parameter for asset 1
    pub lambda_1: u64,

    /// Pro-rata distribution parameter for asset 2
    pub lambda_2: u64,

    /// Unfilled amount of asset 1 (no counterparty)
    pub unfilled_1: u64,

    /// Unfilled amount of asset 2 (no counterparty)
    pub unfilled_2: u64,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_embedded_client_start() {
        let config = PenumbraClientConfig::default();
        let client = EmbeddedPenumbraClient::start(config).await.unwrap();

        // Should initialize successfully
        let height = client.get_latest_height().await.unwrap();
        assert!(height > 0);
    }

    #[tokio::test]
    async fn test_price_calculation() {
        let config = PenumbraClientConfig::default();
        let client = EmbeddedPenumbraClient::start(config).await.unwrap();

        // Batch swap: 1M UM → 1.05M gm
        let batch = BatchSwapOutputData {
            height: 1000,
            delta_1: 1_000_000,
            delta_2: 1_050_000,
            lambda_1: 10000,
            lambda_2: 10000,
            unfilled_1: 0,
            unfilled_2: 0,
        };

        let price = client.calculate_price_from_batch(&batch).unwrap();

        // Price should be 1.05 gm/UM
        assert!((price.0 - 1.05).abs() < 0.01);
    }

    #[tokio::test]
    async fn test_oracle_prices() {
        let mut config = PenumbraClientConfig::default();
        config.oracle_pairs = vec![
            (AssetId([1; 32]), AssetId([2; 32])),
        ];

        let client = EmbeddedPenumbraClient::start(config).await.unwrap();
        let prices = client.get_oracle_prices().await.unwrap();

        assert_eq!(prices.len(), 1);
    }
}
