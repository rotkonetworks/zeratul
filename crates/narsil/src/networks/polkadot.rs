//! polkadot/asset-hub adapter
//!
//! supports polkadot asset hub for multi-asset syndicates.
//! uses ristretto255 curve (sr25519-compatible threshold signatures).
//!
//! # features
//!
//! - multi-asset support (DOT, USDC, USDT, etc.)
//! - xcm for cross-chain transfers
//! - proxy accounts for syndicate addresses
//!
//! # address derivation
//!
//! syndicate address = ss58(osst_group_pubkey)
//! the group key looks like a normal sr25519 account.

use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;

use crate::traits::{NetworkAdapter, ActionBuilder, TxHash, TxStatus};
use crate::wire::Hash32;

/// polkadot address (ss58 encoded, 32 bytes raw)
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PolkadotAddress(pub [u8; 32]);

impl AsRef<[u8]> for PolkadotAddress {
    fn as_ref(&self) -> &[u8] {
        &self.0
    }
}

/// polkadot transaction (scale encoded)
#[derive(Clone, Debug)]
pub struct PolkadotTransaction {
    /// call data (scale encoded)
    pub call: Vec<u8>,
    /// era (mortal/immortal)
    pub era: Era,
    /// nonce
    pub nonce: u32,
    /// tip
    pub tip: u128,
    /// signature (filled after signing)
    pub signature: Option<[u8; 64]>,
}

/// transaction era
#[derive(Clone, Copy, Debug)]
pub enum Era {
    Immortal,
    Mortal { period: u64, phase: u64 },
}

/// polkadot network adapter
#[derive(Clone, Debug)]
pub struct PolkadotAdapter {
    /// rpc endpoint
    endpoint: String,
    /// chain id (polkadot = 0, kusama = 2, asset-hub = 1000)
    chain_id: u32,
    /// ss58 prefix
    ss58_prefix: u16,
    /// genesis hash
    genesis_hash: Hash32,
}

impl PolkadotAdapter {
    /// create adapter for asset hub
    pub fn asset_hub(endpoint: impl Into<String>) -> Self {
        Self {
            endpoint: endpoint.into(),
            chain_id: 1000, // asset hub parachain id
            ss58_prefix: 0, // polkadot prefix
            genesis_hash: [0u8; 32], // filled on connect
        }
    }

    /// create adapter for kusama asset hub
    pub fn kusama_asset_hub(endpoint: impl Into<String>) -> Self {
        Self {
            endpoint: endpoint.into(),
            chain_id: 1000,
            ss58_prefix: 2, // kusama prefix
            genesis_hash: [0u8; 32],
        }
    }

    /// get endpoint
    pub fn endpoint(&self) -> &str {
        &self.endpoint
    }

    /// encode address to ss58
    pub fn encode_ss58(&self, pubkey: &[u8; 32]) -> String {
        // simplified - real impl uses ss58-registry
        let mut addr = String::from("1"); // polkadot prefix
        addr.push_str(&hex::encode(&pubkey[..8]));
        addr
    }
}

impl NetworkAdapter for PolkadotAdapter {
    type Address = PolkadotAddress;
    type Transaction = PolkadotTransaction;
    type Receipt = PolkadotReceipt;
    type Error = PolkadotError;

    fn network_id(&self) -> &str {
        match self.chain_id {
            1000 => "polkadot-asset-hub",
            _ => "polkadot-unknown",
        }
    }

    fn is_connected(&self) -> bool {
        // TODO: check websocket connection
        true
    }

    fn submit(&self, tx: &Self::Transaction) -> Result<TxHash, Self::Error> {
        if tx.signature.is_none() {
            return Err(PolkadotError::NotSigned);
        }
        // TODO: submit via rpc
        Ok([0u8; 32])
    }

    fn tx_status(&self, _hash: &TxHash) -> Result<TxStatus, Self::Error> {
        // TODO: query chain
        Ok(TxStatus::Unknown)
    }

    fn current_height(&self) -> Result<u64, Self::Error> {
        // TODO: query chain
        Ok(0)
    }

    fn estimate_fee(&self, _tx: &Self::Transaction) -> Result<u64, Self::Error> {
        // TODO: query payment.queryInfo
        Ok(0)
    }
}

/// polkadot receipt
#[derive(Clone, Debug)]
pub struct PolkadotReceipt {
    pub block_hash: Hash32,
    pub block_number: u32,
    pub extrinsic_index: u32,
    pub success: bool,
}

/// polkadot errors
#[derive(Clone, Debug)]
pub enum PolkadotError {
    /// not connected
    NotConnected,
    /// transaction not signed
    NotSigned,
    /// rpc error
    Rpc(String),
    /// invalid transaction
    InvalidTransaction(String),
}

/// asset hub action types
#[derive(Clone, Debug)]
pub enum AssetHubAction {
    /// transfer native token (DOT/KSM)
    TransferNative {
        to: PolkadotAddress,
        amount: u128,
    },
    /// transfer asset (USDC, USDT, etc.)
    TransferAsset {
        asset_id: u32,
        to: PolkadotAddress,
        amount: u128,
    },
    /// xcm transfer to another chain
    XcmTransfer {
        dest_chain: u32,
        to: Vec<u8>, // multilocation encoded
        asset_id: u32,
        amount: u128,
    },
}

/// asset hub action builder
pub struct AssetHubActionBuilder;

impl ActionBuilder for AssetHubActionBuilder {
    type Action = AssetHubAction;
    type Error = &'static str;

    fn action_kind(&self) -> &str {
        "asset-hub"
    }

    fn build_from_bytes(&self, data: &[u8]) -> Result<Self::Action, Self::Error> {
        if data.is_empty() {
            return Err("empty data");
        }

        match data[0] {
            0 => {
                // transfer native
                if data.len() < 1 + 32 + 16 {
                    return Err("insufficient data for transfer");
                }
                let to: [u8; 32] = data[1..33].try_into().unwrap();
                let amount = u128::from_le_bytes(data[33..49].try_into().unwrap());
                Ok(AssetHubAction::TransferNative {
                    to: PolkadotAddress(to),
                    amount,
                })
            }
            1 => {
                // transfer asset
                if data.len() < 1 + 4 + 32 + 16 {
                    return Err("insufficient data for asset transfer");
                }
                let asset_id = u32::from_le_bytes(data[1..5].try_into().unwrap());
                let to: [u8; 32] = data[5..37].try_into().unwrap();
                let amount = u128::from_le_bytes(data[37..53].try_into().unwrap());
                Ok(AssetHubAction::TransferAsset {
                    asset_id,
                    to: PolkadotAddress(to),
                    amount,
                })
            }
            _ => Err("unknown action type"),
        }
    }

    fn to_bytes(&self, action: &Self::Action) -> Vec<u8> {
        let mut buf = Vec::new();
        match action {
            AssetHubAction::TransferNative { to, amount } => {
                buf.push(0);
                buf.extend_from_slice(&to.0);
                buf.extend_from_slice(&amount.to_le_bytes());
            }
            AssetHubAction::TransferAsset { asset_id, to, amount } => {
                buf.push(1);
                buf.extend_from_slice(&asset_id.to_le_bytes());
                buf.extend_from_slice(&to.0);
                buf.extend_from_slice(&amount.to_le_bytes());
            }
            AssetHubAction::XcmTransfer { dest_chain, to, asset_id, amount } => {
                buf.push(2);
                buf.extend_from_slice(&dest_chain.to_le_bytes());
                buf.extend_from_slice(&(to.len() as u32).to_le_bytes());
                buf.extend_from_slice(to);
                buf.extend_from_slice(&asset_id.to_le_bytes());
                buf.extend_from_slice(&amount.to_le_bytes());
            }
        }
        buf
    }

    fn validate(&self, action: &Self::Action) -> Result<(), Self::Error> {
        match action {
            AssetHubAction::TransferNative { amount, .. } => {
                if *amount == 0 {
                    return Err("amount must be > 0");
                }
            }
            AssetHubAction::TransferAsset { amount, .. } => {
                if *amount == 0 {
                    return Err("amount must be > 0");
                }
            }
            AssetHubAction::XcmTransfer { amount, .. } => {
                if *amount == 0 {
                    return Err("amount must be > 0");
                }
            }
        }
        Ok(())
    }

    fn describe(&self, action: &Self::Action) -> String {
        match action {
            AssetHubAction::TransferNative { to, amount } => {
                format!("transfer {} native to {:?}", amount, &to.0[..4])
            }
            AssetHubAction::TransferAsset { asset_id, to, amount } => {
                format!("transfer {} of asset {} to {:?}", amount, asset_id, &to.0[..4])
            }
            AssetHubAction::XcmTransfer { dest_chain, amount, asset_id, .. } => {
                format!("xcm {} of asset {} to chain {}", amount, asset_id, dest_chain)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_adapter_creation() {
        let adapter = PolkadotAdapter::asset_hub("wss://asset-hub.polkadot.io");
        assert_eq!(adapter.network_id(), "polkadot-asset-hub");
    }

    #[test]
    fn test_action_roundtrip() {
        let builder = AssetHubActionBuilder;

        let action = AssetHubAction::TransferNative {
            to: PolkadotAddress([1u8; 32]),
            amount: 1_000_000_000_000,
        };

        let bytes = builder.to_bytes(&action);
        let recovered = builder.build_from_bytes(&bytes).unwrap();

        if let AssetHubAction::TransferNative { to, amount } = recovered {
            assert_eq!(to.0, [1u8; 32]);
            assert_eq!(amount, 1_000_000_000_000);
        } else {
            panic!("wrong action type");
        }
    }

    #[test]
    fn test_action_validation() {
        let builder = AssetHubActionBuilder;

        let valid = AssetHubAction::TransferNative {
            to: PolkadotAddress([1u8; 32]),
            amount: 100,
        };
        assert!(builder.validate(&valid).is_ok());

        let invalid = AssetHubAction::TransferNative {
            to: PolkadotAddress([1u8; 32]),
            amount: 0,
        };
        assert!(builder.validate(&invalid).is_err());
    }
}
