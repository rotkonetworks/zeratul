//! penumbra adapter
//!
//! private defi syndicate support via penumbra's shielded pool.
//! uses decaf377 curve (native penumbra curve).
//!
//! # features
//!
//! - fully shielded transactions
//! - private swaps via zswap
//! - staking delegation
//! - ibc transfers (private)
//!
//! # address derivation
//!
//! syndicate viewing key = osst_group_key (shared among members)
//! each action requires threshold signature to authorize spend.

use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;

use crate::traits::{NetworkAdapter, ActionBuilder, TxHash, TxStatus};
use crate::wire::Hash32;

/// penumbra address (diversified, 80 bytes)
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PenumbraAddress(pub Vec<u8>);

impl AsRef<[u8]> for PenumbraAddress {
    fn as_ref(&self) -> &[u8] {
        &self.0
    }
}

/// penumbra transaction
#[derive(Clone, Debug)]
pub struct PenumbraTransaction {
    /// transaction body (protobuf encoded)
    pub body: Vec<u8>,
    /// binding signature
    pub binding_sig: Option<[u8; 64]>,
    /// anchor (merkle root)
    pub anchor: Hash32,
}

/// penumbra network adapter
#[derive(Clone, Debug)]
pub struct PenumbraAdapter {
    /// grpc endpoint
    endpoint: String,
    /// chain id
    chain_id: String,
}

impl PenumbraAdapter {
    /// create adapter for mainnet
    pub fn mainnet(endpoint: impl Into<String>) -> Self {
        Self {
            endpoint: endpoint.into(),
            chain_id: "penumbra-1".into(),
        }
    }

    /// create adapter for testnet
    pub fn testnet(endpoint: impl Into<String>) -> Self {
        Self {
            endpoint: endpoint.into(),
            chain_id: "penumbra-testnet".into(),
        }
    }

    /// get endpoint
    pub fn endpoint(&self) -> &str {
        &self.endpoint
    }
}

impl NetworkAdapter for PenumbraAdapter {
    type Address = PenumbraAddress;
    type Transaction = PenumbraTransaction;
    type Receipt = PenumbraReceipt;
    type Error = PenumbraError;

    fn network_id(&self) -> &str {
        &self.chain_id
    }

    fn is_connected(&self) -> bool {
        // TODO: check grpc connection
        true
    }

    fn submit(&self, tx: &Self::Transaction) -> Result<TxHash, Self::Error> {
        if tx.binding_sig.is_none() {
            return Err(PenumbraError::NotSigned);
        }
        // TODO: submit via grpc
        Ok([0u8; 32])
    }

    fn tx_status(&self, _hash: &TxHash) -> Result<TxStatus, Self::Error> {
        Ok(TxStatus::Unknown)
    }

    fn current_height(&self) -> Result<u64, Self::Error> {
        Ok(0)
    }

    fn estimate_fee(&self, _tx: &Self::Transaction) -> Result<u64, Self::Error> {
        Ok(0)
    }
}

/// penumbra receipt
#[derive(Clone, Debug)]
pub struct PenumbraReceipt {
    pub height: u64,
    pub tx_hash: Hash32,
}

/// penumbra errors
#[derive(Clone, Debug)]
pub enum PenumbraError {
    NotConnected,
    NotSigned,
    Grpc(String),
    InvalidTransaction(String),
}

/// penumbra action types
#[derive(Clone, Debug)]
pub enum PenumbraAction {
    /// spend from shielded pool
    Spend {
        value: AssetValue,
        note_commitment: Hash32,
    },
    /// output to shielded pool
    Output {
        value: AssetValue,
        dest_address: PenumbraAddress,
    },
    /// swap via zswap
    Swap {
        input: AssetValue,
        output_asset: Hash32,
        claim_address: PenumbraAddress,
    },
    /// delegate stake
    Delegate {
        validator: Hash32,
        amount: u128,
    },
    /// undelegate stake
    Undelegate {
        validator: Hash32,
        amount: u128,
    },
    /// ibc transfer
    IbcTransfer {
        channel: String,
        value: AssetValue,
        dest_address: Vec<u8>,
    },
}

/// asset value (amount + asset id)
#[derive(Clone, Debug)]
pub struct AssetValue {
    pub amount: u128,
    pub asset_id: Hash32,
}

/// penumbra action builder
pub struct PenumbraActionBuilder;

impl ActionBuilder for PenumbraActionBuilder {
    type Action = PenumbraAction;
    type Error = &'static str;

    fn action_kind(&self) -> &str {
        "penumbra"
    }

    fn build_from_bytes(&self, data: &[u8]) -> Result<Self::Action, Self::Error> {
        if data.is_empty() {
            return Err("empty data");
        }

        match data[0] {
            0 => {
                // spend
                if data.len() < 1 + 16 + 32 + 32 {
                    return Err("insufficient data for spend");
                }
                let amount = u128::from_le_bytes(data[1..17].try_into().unwrap());
                let asset_id: [u8; 32] = data[17..49].try_into().unwrap();
                let note_commitment: [u8; 32] = data[49..81].try_into().unwrap();
                Ok(PenumbraAction::Spend {
                    value: AssetValue { amount, asset_id },
                    note_commitment,
                })
            }
            1 => {
                // output
                if data.len() < 1 + 16 + 32 + 4 {
                    return Err("insufficient data for output");
                }
                let amount = u128::from_le_bytes(data[1..17].try_into().unwrap());
                let asset_id: [u8; 32] = data[17..49].try_into().unwrap();
                let addr_len = u32::from_le_bytes(data[49..53].try_into().unwrap()) as usize;
                if data.len() < 53 + addr_len {
                    return Err("insufficient data for address");
                }
                let dest_address = PenumbraAddress(data[53..53 + addr_len].to_vec());
                Ok(PenumbraAction::Output {
                    value: AssetValue { amount, asset_id },
                    dest_address,
                })
            }
            _ => Err("unknown action type"),
        }
    }

    fn to_bytes(&self, action: &Self::Action) -> Vec<u8> {
        let mut buf = Vec::new();
        match action {
            PenumbraAction::Spend { value, note_commitment } => {
                buf.push(0);
                buf.extend_from_slice(&value.amount.to_le_bytes());
                buf.extend_from_slice(&value.asset_id);
                buf.extend_from_slice(note_commitment);
            }
            PenumbraAction::Output { value, dest_address } => {
                buf.push(1);
                buf.extend_from_slice(&value.amount.to_le_bytes());
                buf.extend_from_slice(&value.asset_id);
                buf.extend_from_slice(&(dest_address.0.len() as u32).to_le_bytes());
                buf.extend_from_slice(&dest_address.0);
            }
            _ => {
                // other actions TODO
                buf.push(255);
            }
        }
        buf
    }

    fn validate(&self, action: &Self::Action) -> Result<(), Self::Error> {
        match action {
            PenumbraAction::Spend { value, .. } => {
                if value.amount == 0 {
                    return Err("amount must be > 0");
                }
            }
            PenumbraAction::Output { value, .. } => {
                if value.amount == 0 {
                    return Err("amount must be > 0");
                }
            }
            _ => {}
        }
        Ok(())
    }

    fn describe(&self, action: &Self::Action) -> String {
        match action {
            PenumbraAction::Spend { value, .. } => {
                format!("spend {} of {:?}", value.amount, &value.asset_id[..4])
            }
            PenumbraAction::Output { value, .. } => {
                format!("output {} of {:?}", value.amount, &value.asset_id[..4])
            }
            PenumbraAction::Swap { input, .. } => {
                format!("swap {} of {:?}", input.amount, &input.asset_id[..4])
            }
            PenumbraAction::Delegate { amount, .. } => {
                format!("delegate {}", amount)
            }
            PenumbraAction::Undelegate { amount, .. } => {
                format!("undelegate {}", amount)
            }
            PenumbraAction::IbcTransfer { channel, value, .. } => {
                format!("ibc {} of {:?} via {}", value.amount, &value.asset_id[..4], channel)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_adapter_creation() {
        let adapter = PenumbraAdapter::mainnet("https://grpc.penumbra.zone");
        assert_eq!(adapter.network_id(), "penumbra-1");
    }

    #[test]
    fn test_action_roundtrip() {
        let builder = PenumbraActionBuilder;

        let action = PenumbraAction::Output {
            value: AssetValue {
                amount: 1_000_000,
                asset_id: [1u8; 32],
            },
            dest_address: PenumbraAddress(vec![2u8; 80]),
        };

        let bytes = builder.to_bytes(&action);
        let recovered = builder.build_from_bytes(&bytes).unwrap();

        if let PenumbraAction::Output { value, dest_address } = recovered {
            assert_eq!(value.amount, 1_000_000);
            assert_eq!(dest_address.0.len(), 80);
        } else {
            panic!("wrong action type");
        }
    }
}
