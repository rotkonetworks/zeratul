//! Substrate-compatible types for pallet integration
//!
//! These types can be encoded/decoded for submission to pallet-sla-monitor.

use parity_scale_codec::{Decode, Encode};
use scale_info::TypeInfo;

/// Measurement result matching pallet's MeasurementResult
#[derive(Clone, Copy, Encode, Decode, TypeInfo, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum MeasurementResult {
    Up = 0,
    Down = 1,
    Timeout = 2,
    Degraded = 3,
}

impl Default for MeasurementResult {
    fn default() -> Self {
        MeasurementResult::Down
    }
}

/// Probe report matching pallet's ProbeReport
#[derive(Clone, Encode, Decode, TypeInfo, Debug, Default)]
pub struct ProbeReport {
    pub result: MeasurementResult,
    pub latency_ms: u32,
    pub timestamp: u64,
}

/// Extended report data (goes to IPFS, hash stored on-chain)
#[derive(Clone, Encode, Decode, TypeInfo, Debug)]
pub struct ExtendedReport {
    /// Basic report for consensus
    pub summary: ProbeReport,
    /// Target endpoint
    pub endpoint: Vec<u8>,
    /// Network name
    pub network: Vec<u8>,
    /// Individual check results
    pub checks: Vec<CheckDetail>,
    /// IPv6 check
    pub is_ipv6: bool,
    /// Probe geographic zone
    pub zone: u8,
}

/// Individual check detail
#[derive(Clone, Encode, Decode, TypeInfo, Debug)]
pub struct CheckDetail {
    /// Check name (tcp_ping, wss_connect, etc.)
    pub name: Vec<u8>,
    /// Passed
    pub passed: bool,
    /// Latency in ms
    pub latency_ms: u32,
    /// Error message if failed
    pub error: Option<Vec<u8>>,
}

/// Report submission for chain
#[derive(Clone, Encode, Decode, TypeInfo, Debug)]
pub struct ReportSubmission {
    /// Node being monitored (blake2-256 of endpoint)
    pub node_id: [u8; 32],
    /// Epoch number
    pub epoch: u64,
    /// Basic report for consensus voting
    pub report: ProbeReport,
    /// Blake2-256 hash of ExtendedReport
    pub extended_hash: [u8; 32],
    /// IPFS CID of ExtendedReport (optional)
    pub cid: Option<Vec<u8>>,
    /// Ligerito proof of execution (optional, for trustless mode)
    pub proof: Option<Vec<u8>>,
}

/// Convert CheckResult to pallet types
impl From<&crate::CheckResult> for ProbeReport {
    fn from(result: &crate::CheckResult) -> Self {
        // Determine overall result
        let measurement = if !result.healthy {
            // Check if it was timeout or just down
            let has_timeout = result.checks.iter().any(|c| {
                c.latency_ms == u32::MAX || c.error.as_ref().map(|e| e.contains("timeout")).unwrap_or(false)
            });
            if has_timeout {
                MeasurementResult::Timeout
            } else {
                MeasurementResult::Down
            }
        } else {
            // Check for degraded (high latency)
            let avg_latency: u32 = if result.checks.is_empty() {
                0
            } else {
                let valid_checks: Vec<_> = result.checks.iter()
                    .filter(|c| c.latency_ms != u32::MAX)
                    .collect();
                if valid_checks.is_empty() {
                    0
                } else {
                    valid_checks.iter().map(|c| c.latency_ms).sum::<u32>() / valid_checks.len() as u32
                }
            };

            if avg_latency > 500 {
                MeasurementResult::Degraded
            } else {
                MeasurementResult::Up
            }
        };

        // Get best latency from checks
        let latency_ms = result.checks.iter()
            .filter(|c| c.passed && c.latency_ms != u32::MAX)
            .map(|c| c.latency_ms)
            .min()
            .unwrap_or(0);

        ProbeReport {
            result: measurement,
            latency_ms,
            timestamp: result.started_at_ms,
        }
    }
}

impl From<&crate::CheckResult> for ExtendedReport {
    fn from(result: &crate::CheckResult) -> Self {
        let (endpoint, network, is_ipv6) = match &result.target {
            crate::CheckTarget::Endpoint { url, ipv6, .. } => {
                (url.as_bytes().to_vec(), Vec::new(), *ipv6)
            }
            crate::CheckTarget::Site { hostname, ipv6 } => {
                (hostname.as_bytes().to_vec(), Vec::new(), *ipv6)
            }
            crate::CheckTarget::Domain { domain, network, ipv6 } => {
                (domain.as_bytes().to_vec(), network.as_bytes().to_vec(), *ipv6)
            }
            crate::CheckTarget::BootNode { multiaddr, network } => {
                (multiaddr.as_bytes().to_vec(), network.as_bytes().to_vec(), false)
            }
        };

        let checks = result.checks.iter().map(|c| CheckDetail {
            name: c.name.as_bytes().to_vec(),
            passed: c.passed,
            latency_ms: c.latency_ms,
            error: c.error.as_ref().map(|e| e.as_bytes().to_vec()),
        }).collect();

        ExtendedReport {
            summary: ProbeReport::from(result),
            endpoint,
            network,
            checks,
            is_ipv6,
            zone: 0, // Set by probe based on location
        }
    }
}

/// Compute node_id from endpoint URL
pub fn node_id_from_endpoint(endpoint: &str) -> [u8; 32] {
    use sha2::{Sha256, Digest};
    let mut hasher = Sha256::new();
    hasher.update(endpoint.as_bytes());
    let result = hasher.finalize();
    let mut id = [0u8; 32];
    id.copy_from_slice(&result);
    id
}

/// Compute hash of extended report
pub fn hash_extended_report(report: &ExtendedReport) -> [u8; 32] {
    use sha2::{Sha256, Digest};
    let encoded = report.encode();
    let mut hasher = Sha256::new();
    hasher.update(&encoded);
    let result = hasher.finalize();
    let mut hash = [0u8; 32];
    hash.copy_from_slice(&result);
    hash
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{CheckResult, CheckTarget, IndividualCheck};

    #[test]
    fn test_convert_healthy_result() {
        let mut result = CheckResult::new(CheckTarget::Endpoint {
            url: "https://rpc.rotko.net/polkadot".to_string(),
            service_type: crate::ServiceType::Rpc,
            ipv6: false,
        });
        result.add_check(IndividualCheck {
            name: "tcp_ping".to_string(),
            passed: true,
            latency_ms: 10,
            details: serde_json::Value::Null,
            error: None,
        });
        result.finalize();

        let report = ProbeReport::from(&result);
        assert_eq!(report.result, MeasurementResult::Up);
        assert_eq!(report.latency_ms, 10);
    }

    #[test]
    fn test_convert_degraded_result() {
        let mut result = CheckResult::new(CheckTarget::Endpoint {
            url: "https://slow.example.com".to_string(),
            service_type: crate::ServiceType::Rpc,
            ipv6: false,
        });
        result.add_check(IndividualCheck {
            name: "tcp_ping".to_string(),
            passed: true,
            latency_ms: 800, // High latency
            details: serde_json::Value::Null,
            error: None,
        });
        result.finalize();

        let report = ProbeReport::from(&result);
        assert_eq!(report.result, MeasurementResult::Degraded);
    }

    #[test]
    fn test_node_id() {
        let id1 = node_id_from_endpoint("https://rpc.rotko.net/polkadot");
        let id2 = node_id_from_endpoint("https://rpc.rotko.net/polkadot");
        let id3 = node_id_from_endpoint("https://rpc.rotko.net/kusama");

        assert_eq!(id1, id2);
        assert_ne!(id1, id3);
    }
}
