//! Generic Host Primitives for Monitoring Contracts
//!
//! These primitives can be composed by any monitoring guest contract
//! to build different kinds of checks.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::time::Duration;

/// Host call IDs - generic primitives
pub mod host_calls {
    // Network primitives
    pub const TCP_CONNECT: u32 = 0x100;
    pub const TCP_PING: u32 = 0x101;
    pub const UDP_SEND_RECV: u32 = 0x102;
    pub const DNS_RESOLVE: u32 = 0x103;
    pub const DNS_RESOLVE_V6: u32 = 0x104;

    // HTTP primitives
    pub const HTTP_GET: u32 = 0x110;
    pub const HTTP_POST: u32 = 0x111;
    pub const HTTP_HEAD: u32 = 0x112;

    // WebSocket primitives
    pub const WSS_CONNECT: u32 = 0x120;
    pub const WSS_SEND: u32 = 0x121;
    pub const WSS_RECV: u32 = 0x122;
    pub const WSS_CLOSE: u32 = 0x123;

    // JSON-RPC primitives
    pub const RPC_CALL: u32 = 0x130;
    pub const RPC_BATCH: u32 = 0x131;
    pub const RPC_SUBSCRIBE: u32 = 0x132;

    // Substrate/Polkadot specific
    pub const SUBSTRATE_CHAIN_HEAD: u32 = 0x140;
    pub const SUBSTRATE_CHAIN_FINALIZED: u32 = 0x141;
    pub const SUBSTRATE_SYSTEM_HEALTH: u32 = 0x142;
    pub const SUBSTRATE_SYSTEM_PEERS: u32 = 0x143;
    pub const SUBSTRATE_STATE_CALL: u32 = 0x144;
    pub const SUBSTRATE_SYNC_STATE: u32 = 0x145;

    // Relay chain reference
    pub const RELAY_FINALIZED: u32 = 0x150;
    pub const RELAY_VALIDATORS: u32 = 0x151;
    pub const PARACHAIN_HEAD: u32 = 0x152;

    // System primitives
    pub const TIMESTAMP_MS: u32 = 0x160;
    pub const RANDOM_BYTES: u32 = 0x161;

    // I/O primitives
    pub const READ_INPUT: u32 = 0x170;
    pub const WRITE_OUTPUT: u32 = 0x171;
    pub const LOG_DEBUG: u32 = 0x172;

    // Crypto primitives
    pub const HASH_BLAKE2B: u32 = 0x180;
    pub const HASH_KECCAK256: u32 = 0x181;
    pub const VERIFY_ED25519: u32 = 0x182;
    pub const VERIFY_SR25519: u32 = 0x183;
}

/// Result of a network check
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NetworkResult {
    pub success: bool,
    pub latency_ms: u32,
    pub error: Option<String>,
    pub data: Vec<u8>,
}

/// DNS resolution result
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DnsResult {
    pub success: bool,
    pub ipv4_addrs: Vec<String>,
    pub ipv6_addrs: Vec<String>,
    pub latency_ms: u32,
    pub error: Option<String>,
}

/// HTTP response
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HttpResult {
    pub success: bool,
    pub status_code: u16,
    pub latency_ms: u32,
    pub headers: HashMap<String, String>,
    pub body: Vec<u8>,
    pub error: Option<String>,
}

/// WebSocket connection handle
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WssHandle {
    pub id: u32,
    pub connected: bool,
    pub latency_ms: u32,
}

/// RPC call result
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RpcResult {
    pub success: bool,
    pub latency_ms: u32,
    pub result: serde_json::Value,
    pub error: Option<String>,
}

/// Substrate chain info
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SubstrateChainInfo {
    pub chain: String,
    pub node_name: String,
    pub node_version: String,
    pub best_block: u32,
    pub best_hash: [u8; 32],
    pub finalized_block: u32,
    pub finalized_hash: [u8; 32],
    pub peer_count: u32,
    pub is_syncing: bool,
    pub sync_target: Option<u32>,
}

/// System health from substrate node
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SystemHealth {
    pub is_syncing: bool,
    pub peers: u32,
    pub should_have_peers: bool,
}

/// Check configuration - generic input for any monitoring contract
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CheckConfig {
    /// What to check
    pub target: CheckTarget,
    /// Check parameters
    pub params: CheckParams,
    /// Timeout for all operations
    pub timeout_ms: u32,
}

/// Target specification
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum CheckTarget {
    /// Site-level check (hostname only)
    Site {
        hostname: String,
        ipv6: bool,
    },
    /// Domain/network check
    Domain {
        domain: String,
        network: String, // polkadot, kusama, westend, etc.
        ipv6: bool,
    },
    /// Specific endpoint
    Endpoint {
        url: String,
        service_type: ServiceType,
        ipv6: bool,
    },
    /// Boot node
    BootNode {
        multiaddr: String,
        network: String,
    },
}

/// Service types for different checks
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum ServiceType {
    /// HTTP/HTTPS RPC endpoint
    Rpc,
    /// WebSocket RPC endpoint
    WssRpc,
    /// Boot node (p2p)
    BootNode,
    /// Archive node
    Archive,
    /// Light client endpoint
    LightClient,
    /// Parachain collator
    Collator,
    /// Custom service
    Custom(String),
}

/// Parameters for checks
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct CheckParams {
    /// Maximum acceptable latency (ms)
    pub max_latency_ms: Option<u32>,
    /// Require finalized block matches relay
    pub verify_finalized: bool,
    /// Check sync state
    pub check_sync: bool,
    /// Minimum peer count
    pub min_peers: Option<u32>,
    /// Check archive capability (query old blocks)
    pub verify_archive: bool,
    /// Historical block to query (for archive check)
    pub historical_block: Option<u32>,
    /// Custom RPC methods to call
    pub custom_rpc: Vec<String>,
}

/// Result from any check
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CheckResult {
    /// Overall health status
    pub healthy: bool,
    /// Individual check results
    pub checks: Vec<IndividualCheck>,
    /// Timing info
    pub started_at_ms: u64,
    pub completed_at_ms: u64,
    /// Target info
    pub target: CheckTarget,
    /// Error if failed
    pub error: Option<String>,
}

/// Individual sub-check result
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IndividualCheck {
    pub name: String,
    pub passed: bool,
    pub latency_ms: u32,
    pub details: serde_json::Value,
    pub error: Option<String>,
}

impl CheckResult {
    pub fn new(target: CheckTarget) -> Self {
        Self {
            healthy: true,
            checks: Vec::new(),
            started_at_ms: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_millis() as u64,
            completed_at_ms: 0,
            target,
            error: None,
        }
    }

    pub fn add_check(&mut self, check: IndividualCheck) {
        if !check.passed {
            self.healthy = false;
        }
        self.checks.push(check);
    }

    pub fn finalize(&mut self) {
        self.completed_at_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_millis() as u64;
    }

    pub fn fail(&mut self, error: &str) {
        self.healthy = false;
        self.error = Some(error.to_string());
        self.finalize();
    }
}

/// IBP-compatible output format
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IbpMonitorResult {
    pub member_name: String,
    pub status: bool,
    pub error_text: String,
    pub data: HashMap<String, serde_json::Value>,
    pub is_ipv6: bool,
    pub check_time: String, // ISO 8601
}

impl From<CheckResult> for IbpMonitorResult {
    fn from(result: CheckResult) -> Self {
        let is_ipv6 = match &result.target {
            CheckTarget::Site { ipv6, .. } => *ipv6,
            CheckTarget::Domain { ipv6, .. } => *ipv6,
            CheckTarget::Endpoint { ipv6, .. } => *ipv6,
            CheckTarget::BootNode { .. } => false,
        };

        let mut data = HashMap::new();
        for check in &result.checks {
            data.insert(
                check.name.clone(),
                serde_json::json!({
                    "passed": check.passed,
                    "latency_ms": check.latency_ms,
                    "details": check.details,
                }),
            );
        }

        IbpMonitorResult {
            member_name: String::new(), // Set by caller
            status: result.healthy,
            error_text: result.error.unwrap_or_default(),
            data,
            is_ipv6,
            check_time: chrono::Utc::now().to_rfc3339(),
        }
    }
}
