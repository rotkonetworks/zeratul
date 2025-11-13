// Node operational modes for Zeratul blockchain
//
// This module defines the different operational modes a Zeratul node can run in:
// - Validator: Participates in consensus with deterministic PolkaVM verification
// - Full: Full verification with fast native Ligerito verification
// - Light: Minimal state with succinct proof verification
// - Archive: Full history with RPC serving of complete proofs

use anyhow::Result;
use std::path::PathBuf;

/// Node operational mode
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NodeMode {
    /// Validator mode: participates in consensus
    /// - Uses PolkaVM for deterministic verification
    /// - Requires FROST key share for threshold signing
    /// - Maintains full state
    Validator,

    /// Full node mode: verifies all transactions
    /// - Uses native Ligerito verification (faster)
    /// - Maintains full state
    /// - Can serve RPC queries
    /// - Does NOT participate in consensus
    Full,

    /// Light client mode: minimal resource usage
    /// - Verifies succinct proofs only
    /// - Minimal state (headers + recent proofs)
    /// - Can use PolkaVM or native verification
    Light,

    /// Archive mode: stores complete history
    /// - Uses native verification
    /// - Stores full Ligerito proofs (~MB each)
    /// - Serves RPC queries with historical data
    /// - Can reconstruct witness data
    Archive,
}

impl NodeMode {
    /// Returns whether this mode requires PolkaVM verification
    pub fn requires_polkavm(&self) -> bool {
        matches!(self, NodeMode::Validator)
    }

    /// Returns whether this mode can serve RPC queries
    pub fn supports_rpc(&self) -> bool {
        matches!(
            self,
            NodeMode::Full | NodeMode::Archive | NodeMode::Validator
        )
    }

    /// Returns whether this mode stores full proofs
    pub fn stores_full_proofs(&self) -> bool {
        matches!(self, NodeMode::Archive)
    }

    /// Returns whether this mode participates in consensus
    pub fn participates_in_consensus(&self) -> bool {
        matches!(self, NodeMode::Validator)
    }

    /// Returns expected resource usage tier
    pub fn resource_tier(&self) -> ResourceTier {
        match self {
            NodeMode::Validator => ResourceTier::High,
            NodeMode::Full => ResourceTier::High,
            NodeMode::Light => ResourceTier::Low,
            NodeMode::Archive => ResourceTier::VeryHigh,
        }
    }
}

/// Resource usage tier for a node mode
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum ResourceTier {
    Low,      // Light client: ~MB storage, minimal bandwidth
    High,     // Full/Validator: ~GB storage, normal bandwidth
    VeryHigh, // Archive: ~TB storage, high bandwidth
}

/// Configuration for a Zeratul node
#[derive(Debug, Clone)]
pub struct NodeConfig {
    /// Operational mode
    pub mode: NodeMode,

    /// Path to PolkaVM verifier binary (required for Validator mode)
    pub polkavm_verifier: Option<PathBuf>,

    /// Path to FROST key share (required for Validator mode)
    pub frost_key_share: Option<PathBuf>,

    /// RPC port (optional, enables RPC server)
    pub rpc_port: Option<u16>,

    /// Archive directory (required for Archive mode)
    pub archive_dir: Option<PathBuf>,

    /// Data directory for blockchain state
    pub data_dir: PathBuf,

    /// For Light mode: whether to trust native verification (faster)
    /// If false, uses PolkaVM (slower but guaranteed deterministic)
    pub light_trust_native: bool,
}

impl NodeConfig {
    /// Validate configuration for the specified mode
    pub fn validate(&self) -> Result<()> {
        match self.mode {
            NodeMode::Validator => {
                if self.polkavm_verifier.is_none() {
                    anyhow::bail!("Validator mode requires --polkavm-verifier");
                }
                if self.frost_key_share.is_none() {
                    anyhow::bail!("Validator mode requires --frost-key-share");
                }
            }

            NodeMode::Archive => {
                if self.archive_dir.is_none() {
                    anyhow::bail!("Archive mode requires --archive-dir");
                }
            }

            NodeMode::Full | NodeMode::Light => {
                // No special requirements
            }
        }

        Ok(())
    }

    /// Create a default configuration for testing
    #[cfg(test)]
    pub fn test_config(mode: NodeMode) -> Self {
        use std::env;

        let temp_dir = env::temp_dir();

        Self {
            mode,
            polkavm_verifier: Some(temp_dir.join("verifier.polkavm")),
            frost_key_share: Some(temp_dir.join("keyshare.json")),
            rpc_port: Some(9933),
            archive_dir: Some(temp_dir.join("archive")),
            data_dir: temp_dir.join("data"),
            light_trust_native: false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_node_mode_properties() {
        assert!(NodeMode::Validator.requires_polkavm());
        assert!(!NodeMode::Full.requires_polkavm());

        assert!(NodeMode::Archive.stores_full_proofs());
        assert!(!NodeMode::Light.stores_full_proofs());

        assert!(NodeMode::Validator.participates_in_consensus());
        assert!(!NodeMode::Full.participates_in_consensus());
    }

    #[test]
    fn test_resource_tiers() {
        assert_eq!(NodeMode::Light.resource_tier(), ResourceTier::Low);
        assert_eq!(NodeMode::Archive.resource_tier(), ResourceTier::VeryHigh);
        assert!(NodeMode::Archive.resource_tier() > NodeMode::Light.resource_tier());
    }

    #[test]
    fn test_validator_config_validation() {
        let mut config = NodeConfig::test_config(NodeMode::Validator);

        // Should pass with all required fields
        assert!(config.validate().is_ok());

        // Should fail without PolkaVM verifier
        config.polkavm_verifier = None;
        assert!(config.validate().is_err());

        config.polkavm_verifier = Some(PathBuf::from("verifier.polkavm"));

        // Should fail without FROST key share
        config.frost_key_share = None;
        assert!(config.validate().is_err());
    }

    #[test]
    fn test_archive_config_validation() {
        let mut config = NodeConfig::test_config(NodeMode::Archive);

        // Should pass with archive_dir
        assert!(config.validate().is_ok());

        // Should fail without archive_dir
        config.archive_dir = None;
        assert!(config.validate().is_err());
    }

    #[test]
    fn test_full_and_light_config_validation() {
        // Full and Light modes have no special requirements
        let full_config = NodeConfig::test_config(NodeMode::Full);
        assert!(full_config.validate().is_ok());

        let light_config = NodeConfig::test_config(NodeMode::Light);
        assert!(light_config.validate().is_ok());
    }
}
