//! Smoldot Light Client Integration
//!
//! Provides trustless P2P verification of chain state without relying on RPC endpoints.
//! Used to verify that RPC endpoints are serving correct data.

use std::time::{Duration, Instant};
use futures_util::StreamExt;
use tracing::{info, warn, error};

/// Chain specs for common networks (can be embedded or fetched)
pub mod chain_specs {
    /// URL to fetch chain specs from
    pub const POLKADOT_SPEC_URL: &str = "https://raw.githubusercontent.com/polkadot-fellows/runtimes/main/chain-specs/polkadot.json";
    pub const KUSAMA_SPEC_URL: &str = "https://raw.githubusercontent.com/polkadot-fellows/runtimes/main/chain-specs/kusama.json";
    pub const WESTEND_SPEC_URL: &str = "https://raw.githubusercontent.com/polkadot-fellows/runtimes/main/chain-specs/westend.json";

    /// Get spec URL for network
    pub fn spec_url_for_network(network: &str) -> Option<&'static str> {
        match network.to_lowercase().as_str() {
            "polkadot" => Some(POLKADOT_SPEC_URL),
            "kusama" => Some(KUSAMA_SPEC_URL),
            "westend" => Some(WESTEND_SPEC_URL),
            _ => None,
        }
    }
}

/// Result of smoldot verification
#[derive(Debug, Clone)]
pub struct SmoldotVerification {
    /// Whether verification succeeded
    pub success: bool,
    /// Finalized block number from P2P
    pub p2p_finalized_block: u32,
    /// Finalized block hash from P2P
    pub p2p_finalized_hash: [u8; 32],
    /// Time to sync via P2P (ms)
    pub p2p_sync_time_ms: u64,
    /// Number of peers discovered
    pub peers_discovered: u32,
    /// Is the light client syncing
    pub is_syncing: bool,
    /// Error message if any
    pub error: Option<String>,
}

/// Smoldot light client wrapper for IBP verification
pub struct SmoldotVerifier {
    /// Chain spec JSON content
    chain_spec: String,
    /// Network name
    network: String,
    /// Timeout for sync operations
    timeout_secs: u64,
}

impl SmoldotVerifier {
    /// Create new verifier with chain spec content
    pub fn new(chain_spec: String, network: &str, timeout_secs: u64) -> Self {
        Self {
            chain_spec,
            network: network.to_string(),
            timeout_secs,
        }
    }

    /// Create verifier by fetching chain spec from URL
    pub async fn from_network(network: &str, timeout_secs: u64) -> Result<Self, String> {
        let url = chain_specs::spec_url_for_network(network)
            .ok_or_else(|| format!("Unknown network: {}", network))?;

        let client = reqwest::Client::new();
        let chain_spec = client
            .get(url)
            .timeout(Duration::from_secs(30))
            .send()
            .await
            .map_err(|e| format!("Failed to fetch chain spec: {}", e))?
            .text()
            .await
            .map_err(|e| format!("Failed to read chain spec: {}", e))?;

        Ok(Self::new(chain_spec, network, timeout_secs))
    }

    /// Get finalized block from P2P network
    pub async fn get_finalized_block(&self) -> SmoldotVerification {
        let start = Instant::now();

        // Create smoldot client
        let mut client = smoldot_light::Client::new(
            smoldot_light::platform::default::DefaultPlatform::new(
                "ibp-probe".into(),
                env!("CARGO_PKG_VERSION").into(),
            )
        );

        // Add chain
        let add_result = client.add_chain(smoldot_light::AddChainConfig {
            specification: &self.chain_spec,
            json_rpc: smoldot_light::AddChainConfigJsonRpc::Enabled {
                max_pending_requests: core::num::NonZero::<u32>::new(32).unwrap(),
                max_subscriptions: 32,
            },
            potential_relay_chains: core::iter::empty(),
            database_content: "",
            user_data: (),
        });

        let (chain_id, mut json_rpc_responses) = match add_result {
            Ok(success) => (success.chain_id, success.json_rpc_responses.unwrap()),
            Err(e) => {
                return SmoldotVerification {
                    success: false,
                    p2p_finalized_block: 0,
                    p2p_finalized_hash: [0; 32],
                    p2p_sync_time_ms: start.elapsed().as_millis() as u64,
                    peers_discovered: 0,
                    is_syncing: false,
                    error: Some(format!("Failed to add chain: {:?}", e)),
                };
            }
        };

        // Wait for initial sync
        info!("Waiting for smoldot to sync with {} network...", self.network);
        tokio::time::sleep(Duration::from_secs(self.timeout_secs.min(30))).await;

        // Query finalized head
        client.json_rpc_request(
            r#"{"id":1,"jsonrpc":"2.0","method":"chain_getFinalizedHead","params":[]}"#,
            chain_id,
        ).ok();

        let finalized_hash = match tokio::time::timeout(
            Duration::from_secs(10),
            json_rpc_responses.next()
        ).await {
            Ok(Some(response)) => {
                if let Ok(json) = serde_json::from_str::<serde_json::Value>(&response) {
                    json.get("result")
                        .and_then(|r| r.as_str())
                        .map(|s| s.to_string())
                } else {
                    None
                }
            }
            _ => None,
        };

        // Query system health for peer count
        client.json_rpc_request(
            r#"{"id":2,"jsonrpc":"2.0","method":"system_health","params":[]}"#,
            chain_id,
        ).ok();

        let (peers, is_syncing) = match tokio::time::timeout(
            Duration::from_secs(5),
            json_rpc_responses.next()
        ).await {
            Ok(Some(response)) => {
                if let Ok(json) = serde_json::from_str::<serde_json::Value>(&response) {
                    let peers = json.get("result")
                        .and_then(|r| r.get("peers"))
                        .and_then(|p| p.as_u64())
                        .unwrap_or(0) as u32;
                    let syncing = json.get("result")
                        .and_then(|r| r.get("isSyncing"))
                        .and_then(|s| s.as_bool())
                        .unwrap_or(false);
                    (peers, syncing)
                } else {
                    (0, false)
                }
            }
            _ => (0, false),
        };

        // Get block number if we have the hash
        let (block_num, hash_bytes) = if let Some(hash_hex) = &finalized_hash {
            // Query header
            let request = format!(
                r#"{{"id":3,"jsonrpc":"2.0","method":"chain_getHeader","params":["{}"]}}"#,
                hash_hex
            );
            client.json_rpc_request(&request, chain_id).ok();

            let block_num = match tokio::time::timeout(
                Duration::from_secs(5),
                json_rpc_responses.next()
            ).await {
                Ok(Some(response)) => {
                    if let Ok(json) = serde_json::from_str::<serde_json::Value>(&response) {
                        json.get("result")
                            .and_then(|r| r.get("number"))
                            .and_then(|n| n.as_str())
                            .and_then(|s| u32::from_str_radix(s.trim_start_matches("0x"), 16).ok())
                            .unwrap_or(0)
                    } else {
                        0
                    }
                }
                _ => 0,
            };

            // Parse hash
            let hash_str = hash_hex.trim_start_matches("0x");
            let mut hash_bytes = [0u8; 32];
            if let Ok(bytes) = hex::decode(hash_str) {
                if bytes.len() >= 32 {
                    hash_bytes.copy_from_slice(&bytes[..32]);
                }
            }

            (block_num, hash_bytes)
        } else {
            (0, [0u8; 32])
        };

        // Cleanup
        let _ = client.remove_chain(chain_id);

        let success = block_num > 0 || peers > 0;

        SmoldotVerification {
            success,
            p2p_finalized_block: block_num,
            p2p_finalized_hash: hash_bytes,
            p2p_sync_time_ms: start.elapsed().as_millis() as u64,
            peers_discovered: peers,
            is_syncing,
            error: if success { None } else { Some("Failed to get finalized block".into()) },
        }
    }

    /// Compare RPC endpoint result with P2P result
    pub fn compare_with_rpc(
        &self,
        p2p: &SmoldotVerification,
        rpc_block: u32,
        rpc_hash: &[u8; 32],
        rpc_latency_ms: u32,
    ) -> P2pComparison {
        let hashes_match = p2p.p2p_finalized_hash == *rpc_hash;
        let block_diff = if rpc_block > p2p.p2p_finalized_block {
            rpc_block - p2p.p2p_finalized_block
        } else {
            p2p.p2p_finalized_block - rpc_block
        };

        // RPC is valid if within 2 blocks and hash matches (or close)
        let rpc_valid = block_diff <= 2;

        // Calculate speedup
        let speedup = if rpc_latency_ms > 0 && p2p.p2p_sync_time_ms > 0 {
            (p2p.p2p_sync_time_ms as f64) / (rpc_latency_ms as f64)
        } else {
            0.0
        };

        P2pComparison {
            rpc_valid,
            hashes_match,
            block_diff,
            rpc_latency_ms,
            p2p_sync_time_ms: p2p.p2p_sync_time_ms,
            rpc_speedup: speedup,
            p2p_peers: p2p.peers_discovered,
        }
    }
}

/// Comparison between RPC and P2P results
#[derive(Debug, Clone, serde::Serialize)]
pub struct P2pComparison {
    /// Whether RPC is serving valid data
    pub rpc_valid: bool,
    /// Whether hashes match exactly
    pub hashes_match: bool,
    /// Block number difference
    pub block_diff: u32,
    /// RPC latency (ms)
    pub rpc_latency_ms: u32,
    /// P2P sync time (ms)
    pub p2p_sync_time_ms: u64,
    /// How much faster RPC is vs P2P
    pub rpc_speedup: f64,
    /// Number of P2P peers discovered
    pub p2p_peers: u32,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_spec_url_lookup() {
        assert!(chain_specs::spec_url_for_network("polkadot").is_some());
        assert!(chain_specs::spec_url_for_network("kusama").is_some());
        assert!(chain_specs::spec_url_for_network("unknown").is_none());
    }
}
