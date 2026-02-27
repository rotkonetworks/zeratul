//! zcash adapter
//!
//! private syndicate support via zcash's shielded pool (orchard).
//! uses pallas curve (native orchard curve).
//!
//! # features
//!
//! - orchard shielded transactions
//! - transparent fallback for exchanges
//! - memo field for coordination
//!
//! # address derivation
//!
//! syndicate full viewing key = osst_group_key
//! each spend requires threshold signature authorization.

use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;

use crate::traits::{NetworkAdapter, ActionBuilder, TxHash, TxStatus};
use crate::wire::Hash32;

/// zcash unified address
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ZcashAddress {
    /// orchard receiver (43 bytes)
    pub orchard: Option<[u8; 43]>,
    /// sapling receiver (43 bytes)
    pub sapling: Option<[u8; 43]>,
    /// transparent receiver (20 bytes)
    pub transparent: Option<[u8; 20]>,
}

impl ZcashAddress {
    /// create orchard-only address
    pub fn orchard(receiver: [u8; 43]) -> Self {
        Self {
            orchard: Some(receiver),
            sapling: None,
            transparent: None,
        }
    }

    /// create transparent address
    pub fn transparent(receiver: [u8; 20]) -> Self {
        Self {
            orchard: None,
            sapling: None,
            transparent: Some(receiver),
        }
    }
}

impl AsRef<[u8]> for ZcashAddress {
    fn as_ref(&self) -> &[u8] {
        // return first available receiver
        if let Some(ref o) = self.orchard {
            return o;
        }
        if let Some(ref s) = self.sapling {
            return s;
        }
        if let Some(ref t) = self.transparent {
            return t;
        }
        &[]
    }
}

/// zcash transaction (orchard bundle)
#[derive(Clone, Debug)]
pub struct ZcashTransaction {
    /// orchard actions
    pub actions: Vec<OrchardAction>,
    /// anchor (merkle root)
    pub anchor: Hash32,
    /// binding signature
    pub binding_sig: Option<[u8; 64]>,
    /// memo (512 bytes max)
    pub memo: Option<Vec<u8>>,
}

/// orchard action (spend + output combined)
#[derive(Clone, Debug)]
pub struct OrchardAction {
    /// nullifier (spend side)
    pub nullifier: Hash32,
    /// note commitment (output side)
    pub cmx: Hash32,
    /// encrypted note
    pub encrypted_note: Vec<u8>,
}

/// zcash network adapter
#[derive(Clone, Debug)]
pub struct ZcashAdapter {
    /// lightwalletd endpoint
    endpoint: String,
    /// network (mainnet/testnet)
    network: ZcashNetwork,
}

/// zcash network type
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ZcashNetwork {
    Mainnet,
    Testnet,
}

impl ZcashAdapter {
    /// create adapter for mainnet
    pub fn mainnet(endpoint: impl Into<String>) -> Self {
        Self {
            endpoint: endpoint.into(),
            network: ZcashNetwork::Mainnet,
        }
    }

    /// create adapter for testnet
    pub fn testnet(endpoint: impl Into<String>) -> Self {
        Self {
            endpoint: endpoint.into(),
            network: ZcashNetwork::Testnet,
        }
    }

    /// get endpoint
    pub fn endpoint(&self) -> &str {
        &self.endpoint
    }

    /// get network
    pub fn network(&self) -> ZcashNetwork {
        self.network
    }
}

impl NetworkAdapter for ZcashAdapter {
    type Address = ZcashAddress;
    type Transaction = ZcashTransaction;
    type Receipt = ZcashReceipt;
    type Error = ZcashError;

    fn network_id(&self) -> &str {
        match self.network {
            ZcashNetwork::Mainnet => "zcash-mainnet",
            ZcashNetwork::Testnet => "zcash-testnet",
        }
    }

    fn is_connected(&self) -> bool {
        // TODO: check grpc connection to lightwalletd
        true
    }

    fn submit(&self, tx: &Self::Transaction) -> Result<TxHash, Self::Error> {
        if tx.binding_sig.is_none() {
            return Err(ZcashError::NotSigned);
        }
        // TODO: submit via lightwalletd
        Ok([0u8; 32])
    }

    fn tx_status(&self, _hash: &TxHash) -> Result<TxStatus, Self::Error> {
        // TODO: query lightwalletd
        Ok(TxStatus::Unknown)
    }

    fn current_height(&self) -> Result<u64, Self::Error> {
        // TODO: query lightwalletd
        Ok(0)
    }

    fn estimate_fee(&self, _tx: &Self::Transaction) -> Result<u64, Self::Error> {
        // zcash has fixed fee (currently 1000 zatoshi for orchard)
        Ok(1000)
    }
}

/// zcash receipt
#[derive(Clone, Debug)]
pub struct ZcashReceipt {
    pub height: u64,
    pub tx_hash: Hash32,
    pub orchard_nullifiers: Vec<Hash32>,
}

/// zcash errors
#[derive(Clone, Debug)]
pub enum ZcashError {
    NotConnected,
    NotSigned,
    Grpc(String),
    InvalidTransaction(String),
    InsufficientFunds,
}

/// zcash action types
#[derive(Clone, Debug)]
pub enum ZcashAction {
    /// shielded send (orchard)
    ShieldedSend {
        to: ZcashAddress,
        amount: u64,
        memo: Option<Vec<u8>>,
    },
    /// shielded receive (claim incoming note)
    ShieldedReceive {
        note_commitment: Hash32,
    },
    /// shield transparent funds
    Shield {
        transparent_input: Hash32,
        amount: u64,
    },
    /// unshield to transparent
    Unshield {
        to: [u8; 20],
        amount: u64,
    },
}

/// zcash action builder
pub struct ZcashActionBuilder;

impl ActionBuilder for ZcashActionBuilder {
    type Action = ZcashAction;
    type Error = &'static str;

    fn action_kind(&self) -> &str {
        "zcash"
    }

    fn build_from_bytes(&self, data: &[u8]) -> Result<Self::Action, Self::Error> {
        if data.is_empty() {
            return Err("empty data");
        }

        match data[0] {
            0 => {
                // shielded send
                if data.len() < 1 + 8 + 43 {
                    return Err("insufficient data for shielded send");
                }
                let amount = u64::from_le_bytes(data[1..9].try_into().unwrap());
                let mut orchard = [0u8; 43];
                orchard.copy_from_slice(&data[9..52]);

                let memo = if data.len() > 52 {
                    let memo_len = u16::from_le_bytes(data[52..54].try_into().unwrap()) as usize;
                    if data.len() >= 54 + memo_len {
                        Some(data[54..54 + memo_len].to_vec())
                    } else {
                        None
                    }
                } else {
                    None
                };

                Ok(ZcashAction::ShieldedSend {
                    to: ZcashAddress::orchard(orchard),
                    amount,
                    memo,
                })
            }
            1 => {
                // shielded receive
                if data.len() < 1 + 32 {
                    return Err("insufficient data for shielded receive");
                }
                let note_commitment: [u8; 32] = data[1..33].try_into().unwrap();
                Ok(ZcashAction::ShieldedReceive { note_commitment })
            }
            2 => {
                // shield
                if data.len() < 1 + 32 + 8 {
                    return Err("insufficient data for shield");
                }
                let transparent_input: [u8; 32] = data[1..33].try_into().unwrap();
                let amount = u64::from_le_bytes(data[33..41].try_into().unwrap());
                Ok(ZcashAction::Shield {
                    transparent_input,
                    amount,
                })
            }
            3 => {
                // unshield
                if data.len() < 1 + 20 + 8 {
                    return Err("insufficient data for unshield");
                }
                let to: [u8; 20] = data[1..21].try_into().unwrap();
                let amount = u64::from_le_bytes(data[21..29].try_into().unwrap());
                Ok(ZcashAction::Unshield { to, amount })
            }
            _ => Err("unknown action type"),
        }
    }

    fn to_bytes(&self, action: &Self::Action) -> Vec<u8> {
        let mut buf = Vec::new();
        match action {
            ZcashAction::ShieldedSend { to, amount, memo } => {
                buf.push(0);
                buf.extend_from_slice(&amount.to_le_bytes());
                if let Some(orchard) = &to.orchard {
                    buf.extend_from_slice(orchard);
                } else {
                    buf.extend_from_slice(&[0u8; 43]);
                }
                if let Some(m) = memo {
                    buf.extend_from_slice(&(m.len() as u16).to_le_bytes());
                    buf.extend_from_slice(m);
                }
            }
            ZcashAction::ShieldedReceive { note_commitment } => {
                buf.push(1);
                buf.extend_from_slice(note_commitment);
            }
            ZcashAction::Shield {
                transparent_input,
                amount,
            } => {
                buf.push(2);
                buf.extend_from_slice(transparent_input);
                buf.extend_from_slice(&amount.to_le_bytes());
            }
            ZcashAction::Unshield { to, amount } => {
                buf.push(3);
                buf.extend_from_slice(to);
                buf.extend_from_slice(&amount.to_le_bytes());
            }
        }
        buf
    }

    fn validate(&self, action: &Self::Action) -> Result<(), Self::Error> {
        match action {
            ZcashAction::ShieldedSend { amount, memo, .. } => {
                if *amount == 0 {
                    return Err("amount must be > 0");
                }
                if let Some(m) = memo {
                    if m.len() > 512 {
                        return Err("memo too long (max 512 bytes)");
                    }
                }
            }
            ZcashAction::Shield { amount, .. } => {
                if *amount == 0 {
                    return Err("amount must be > 0");
                }
            }
            ZcashAction::Unshield { amount, .. } => {
                if *amount == 0 {
                    return Err("amount must be > 0");
                }
            }
            ZcashAction::ShieldedReceive { .. } => {}
        }
        Ok(())
    }

    fn describe(&self, action: &Self::Action) -> String {
        match action {
            ZcashAction::ShieldedSend { amount, memo, .. } => {
                let memo_info = if memo.is_some() { " (with memo)" } else { "" };
                format!("shielded send {} zats{}", amount, memo_info)
            }
            ZcashAction::ShieldedReceive { .. } => "claim shielded note".into(),
            ZcashAction::Shield { amount, .. } => {
                format!("shield {} zats", amount)
            }
            ZcashAction::Unshield { amount, .. } => {
                format!("unshield {} zats", amount)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_adapter_creation() {
        let adapter = ZcashAdapter::mainnet("https://lightwalletd.zcash.co");
        assert_eq!(adapter.network_id(), "zcash-mainnet");
        assert_eq!(adapter.network(), ZcashNetwork::Mainnet);
    }

    #[test]
    fn test_action_roundtrip() {
        let builder = ZcashActionBuilder;

        let action = ZcashAction::ShieldedSend {
            to: ZcashAddress::orchard([1u8; 43]),
            amount: 100_000,
            memo: Some(b"test memo".to_vec()),
        };

        let bytes = builder.to_bytes(&action);
        let recovered = builder.build_from_bytes(&bytes).unwrap();

        if let ZcashAction::ShieldedSend { amount, memo, .. } = recovered {
            assert_eq!(amount, 100_000);
            assert_eq!(memo, Some(b"test memo".to_vec()));
        } else {
            panic!("wrong action type");
        }
    }

    #[test]
    fn test_action_validation() {
        let builder = ZcashActionBuilder;

        let valid = ZcashAction::ShieldedSend {
            to: ZcashAddress::orchard([1u8; 43]),
            amount: 1000,
            memo: None,
        };
        assert!(builder.validate(&valid).is_ok());

        let invalid = ZcashAction::ShieldedSend {
            to: ZcashAddress::orchard([1u8; 43]),
            amount: 0,
            memo: None,
        };
        assert!(builder.validate(&invalid).is_err());

        // memo too long
        let long_memo = ZcashAction::ShieldedSend {
            to: ZcashAddress::orchard([1u8; 43]),
            amount: 1000,
            memo: Some(vec![0u8; 600]),
        };
        assert!(builder.validate(&long_memo).is_err());
    }

    #[test]
    fn test_unified_address() {
        let addr = ZcashAddress::orchard([2u8; 43]);
        assert!(addr.orchard.is_some());
        assert!(addr.sapling.is_none());
        assert!(addr.transparent.is_none());
        assert_eq!(addr.as_ref().len(), 43);

        let taddr = ZcashAddress::transparent([3u8; 20]);
        assert!(taddr.transparent.is_some());
        assert_eq!(taddr.as_ref().len(), 20);
    }
}
