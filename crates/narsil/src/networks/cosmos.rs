//! cosmos/osmosis/noble adapter
//!
//! multi-chain support for cosmos ecosystem via ibc.
//! uses secp256k1 curve (standard cosmos signing).
//!
//! # features
//!
//! - native token transfers
//! - ibc cross-chain transfers
//! - osmosis dex integration
//! - noble usdc support
//!
//! # address derivation
//!
//! syndicate address = bech32(osst_group_pubkey)
//! appears as normal cosmos account on chain.

use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;

use crate::traits::{NetworkAdapter, ActionBuilder, TxHash, TxStatus};
use crate::wire::Hash32;

/// cosmos address (bech32 decoded, 20 bytes)
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CosmosAddress {
    /// raw address bytes (20 bytes for secp256k1)
    pub bytes: [u8; 20],
    /// bech32 prefix (e.g., "osmo", "noble", "cosmos")
    pub prefix: String,
}

impl CosmosAddress {
    /// create address with prefix
    pub fn new(bytes: [u8; 20], prefix: impl Into<String>) -> Self {
        Self {
            bytes,
            prefix: prefix.into(),
        }
    }

    /// create osmosis address
    pub fn osmosis(bytes: [u8; 20]) -> Self {
        Self::new(bytes, "osmo")
    }

    /// create noble address
    pub fn noble(bytes: [u8; 20]) -> Self {
        Self::new(bytes, "noble")
    }

    /// encode to bech32 (simplified)
    pub fn to_bech32(&self) -> String {
        // simplified - real impl uses bech32 crate
        format!("{}1{}", self.prefix, hex::encode(&self.bytes[..10]))
    }
}

impl AsRef<[u8]> for CosmosAddress {
    fn as_ref(&self) -> &[u8] {
        &self.bytes
    }
}

/// cosmos transaction
#[derive(Clone, Debug)]
pub struct CosmosTransaction {
    /// chain id
    pub chain_id: String,
    /// account number
    pub account_number: u64,
    /// sequence
    pub sequence: u64,
    /// fee
    pub fee: CosmosFee,
    /// messages
    pub messages: Vec<CosmosMsg>,
    /// signature (filled after signing)
    pub signature: Option<[u8; 64]>,
    /// memo
    pub memo: String,
}

/// cosmos fee
#[derive(Clone, Debug)]
pub struct CosmosFee {
    pub amount: Vec<Coin>,
    pub gas_limit: u64,
}

/// coin amount
#[derive(Clone, Debug)]
pub struct Coin {
    pub denom: String,
    pub amount: u128,
}

impl Coin {
    pub fn new(denom: impl Into<String>, amount: u128) -> Self {
        Self {
            denom: denom.into(),
            amount,
        }
    }

    /// osmosis native token
    pub fn osmo(amount: u128) -> Self {
        Self::new("uosmo", amount)
    }

    /// noble usdc
    pub fn usdc(amount: u128) -> Self {
        Self::new("uusdc", amount)
    }
}

/// cosmos message types
#[derive(Clone, Debug)]
pub enum CosmosMsg {
    /// bank send
    Send {
        from: CosmosAddress,
        to: CosmosAddress,
        amount: Vec<Coin>,
    },
    /// ibc transfer
    IbcTransfer {
        source_port: String,
        source_channel: String,
        token: Coin,
        sender: CosmosAddress,
        receiver: String, // bech32 on destination chain
        timeout_height: u64,
    },
    /// osmosis swap
    OsmosisSwap {
        sender: CosmosAddress,
        pool_id: u64,
        token_in: Coin,
        token_out_denom: String,
        min_amount_out: u128,
    },
}

/// cosmos network adapter
#[derive(Clone, Debug)]
pub struct CosmosAdapter {
    /// rpc endpoint
    endpoint: String,
    /// chain id
    chain_id: String,
    /// bech32 prefix
    prefix: String,
}

/// known cosmos chains
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CosmosChain {
    Osmosis,
    Noble,
    CosmosHub,
}

impl CosmosAdapter {
    /// create adapter for osmosis mainnet
    pub fn osmosis(endpoint: impl Into<String>) -> Self {
        Self {
            endpoint: endpoint.into(),
            chain_id: "osmosis-1".into(),
            prefix: "osmo".into(),
        }
    }

    /// create adapter for noble mainnet
    pub fn noble(endpoint: impl Into<String>) -> Self {
        Self {
            endpoint: endpoint.into(),
            chain_id: "noble-1".into(),
            prefix: "noble".into(),
        }
    }

    /// create adapter for cosmos hub
    pub fn cosmos_hub(endpoint: impl Into<String>) -> Self {
        Self {
            endpoint: endpoint.into(),
            chain_id: "cosmoshub-4".into(),
            prefix: "cosmos".into(),
        }
    }

    /// create custom adapter
    pub fn custom(
        endpoint: impl Into<String>,
        chain_id: impl Into<String>,
        prefix: impl Into<String>,
    ) -> Self {
        Self {
            endpoint: endpoint.into(),
            chain_id: chain_id.into(),
            prefix: prefix.into(),
        }
    }

    /// get endpoint
    pub fn endpoint(&self) -> &str {
        &self.endpoint
    }

    /// get bech32 prefix
    pub fn prefix(&self) -> &str {
        &self.prefix
    }

    /// create address for this chain
    pub fn address(&self, bytes: [u8; 20]) -> CosmosAddress {
        CosmosAddress::new(bytes, &self.prefix)
    }
}

impl NetworkAdapter for CosmosAdapter {
    type Address = CosmosAddress;
    type Transaction = CosmosTransaction;
    type Receipt = CosmosReceipt;
    type Error = CosmosError;

    fn network_id(&self) -> &str {
        &self.chain_id
    }

    fn is_connected(&self) -> bool {
        // TODO: check rpc connection
        true
    }

    fn submit(&self, tx: &Self::Transaction) -> Result<TxHash, Self::Error> {
        if tx.signature.is_none() {
            return Err(CosmosError::NotSigned);
        }
        // TODO: broadcast via rpc
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

    fn estimate_fee(&self, tx: &Self::Transaction) -> Result<u64, Self::Error> {
        // simplified fee estimation based on message count
        let base_gas = 100_000u64;
        let per_msg = 50_000u64;
        Ok(base_gas + (tx.messages.len() as u64) * per_msg)
    }
}

/// cosmos receipt
#[derive(Clone, Debug)]
pub struct CosmosReceipt {
    pub height: u64,
    pub tx_hash: Hash32,
    pub gas_used: u64,
    pub logs: Vec<String>,
}

/// cosmos errors
#[derive(Clone, Debug)]
pub enum CosmosError {
    NotConnected,
    NotSigned,
    Rpc(String),
    InvalidTransaction(String),
    InsufficientFunds,
    SequenceMismatch,
}

/// cosmos/osmosis action types
#[derive(Clone, Debug)]
pub enum CosmosAction {
    /// send tokens
    Send {
        to: CosmosAddress,
        amount: Vec<Coin>,
    },
    /// ibc transfer
    IbcTransfer {
        channel: String,
        to: String, // destination chain address
        token: Coin,
    },
    /// osmosis swap
    Swap {
        pool_id: u64,
        token_in: Coin,
        token_out_denom: String,
        min_amount_out: u128,
    },
    /// provide liquidity
    ProvideLiquidity {
        pool_id: u64,
        tokens: Vec<Coin>,
    },
    /// withdraw liquidity
    WithdrawLiquidity {
        pool_id: u64,
        shares: u128,
    },
}

/// cosmos action builder
pub struct CosmosActionBuilder {
    prefix: String,
}

impl CosmosActionBuilder {
    /// create builder for chain
    pub fn new(prefix: impl Into<String>) -> Self {
        Self {
            prefix: prefix.into(),
        }
    }

    /// create for osmosis
    pub fn osmosis() -> Self {
        Self::new("osmo")
    }

    /// create for noble
    pub fn noble() -> Self {
        Self::new("noble")
    }
}

impl ActionBuilder for CosmosActionBuilder {
    type Action = CosmosAction;
    type Error = &'static str;

    fn action_kind(&self) -> &str {
        "cosmos"
    }

    fn build_from_bytes(&self, data: &[u8]) -> Result<Self::Action, Self::Error> {
        if data.is_empty() {
            return Err("empty data");
        }

        match data[0] {
            0 => {
                // send
                if data.len() < 1 + 20 + 4 {
                    return Err("insufficient data for send");
                }
                let to: [u8; 20] = data[1..21].try_into().unwrap();
                let coin_count = u32::from_le_bytes(data[21..25].try_into().unwrap()) as usize;

                let mut offset = 25;
                let mut amount = Vec::with_capacity(coin_count);

                for _ in 0..coin_count {
                    if data.len() < offset + 4 {
                        return Err("insufficient data for coin denom length");
                    }
                    let denom_len = u32::from_le_bytes(data[offset..offset+4].try_into().unwrap()) as usize;
                    offset += 4;

                    if data.len() < offset + denom_len + 16 {
                        return Err("insufficient data for coin");
                    }
                    let denom = core::str::from_utf8(&data[offset..offset+denom_len])
                        .map_err(|_| "invalid denom utf8")?
                        .to_string();
                    offset += denom_len;

                    let amt = u128::from_le_bytes(data[offset..offset+16].try_into().unwrap());
                    offset += 16;

                    amount.push(Coin::new(denom, amt));
                }

                Ok(CosmosAction::Send {
                    to: CosmosAddress::new(to, &self.prefix),
                    amount,
                })
            }
            1 => {
                // ibc transfer
                if data.len() < 1 + 4 {
                    return Err("insufficient data for ibc transfer");
                }
                let channel_len = u32::from_le_bytes(data[1..5].try_into().unwrap()) as usize;
                if data.len() < 5 + channel_len + 4 {
                    return Err("insufficient data for channel");
                }
                let channel = core::str::from_utf8(&data[5..5+channel_len])
                    .map_err(|_| "invalid channel utf8")?
                    .to_string();

                let mut offset = 5 + channel_len;
                let to_len = u32::from_le_bytes(data[offset..offset+4].try_into().unwrap()) as usize;
                offset += 4;

                if data.len() < offset + to_len + 4 + 16 {
                    return Err("insufficient data for destination");
                }
                let to = core::str::from_utf8(&data[offset..offset+to_len])
                    .map_err(|_| "invalid to utf8")?
                    .to_string();
                offset += to_len;

                let denom_len = u32::from_le_bytes(data[offset..offset+4].try_into().unwrap()) as usize;
                offset += 4;

                if data.len() < offset + denom_len + 16 {
                    return Err("insufficient data for token");
                }
                let denom = core::str::from_utf8(&data[offset..offset+denom_len])
                    .map_err(|_| "invalid denom utf8")?
                    .to_string();
                offset += denom_len;

                let amount = u128::from_le_bytes(data[offset..offset+16].try_into().unwrap());

                Ok(CosmosAction::IbcTransfer {
                    channel,
                    to,
                    token: Coin::new(denom, amount),
                })
            }
            2 => {
                // swap
                if data.len() < 1 + 8 + 4 + 16 + 4 + 16 {
                    return Err("insufficient data for swap");
                }
                let pool_id = u64::from_le_bytes(data[1..9].try_into().unwrap());
                let in_denom_len = u32::from_le_bytes(data[9..13].try_into().unwrap()) as usize;

                if data.len() < 13 + in_denom_len + 16 + 4 {
                    return Err("insufficient data for swap token_in");
                }
                let in_denom = core::str::from_utf8(&data[13..13+in_denom_len])
                    .map_err(|_| "invalid in_denom utf8")?
                    .to_string();
                let mut offset = 13 + in_denom_len;

                let in_amount = u128::from_le_bytes(data[offset..offset+16].try_into().unwrap());
                offset += 16;

                let out_denom_len = u32::from_le_bytes(data[offset..offset+4].try_into().unwrap()) as usize;
                offset += 4;

                if data.len() < offset + out_denom_len + 16 {
                    return Err("insufficient data for swap token_out");
                }
                let out_denom = core::str::from_utf8(&data[offset..offset+out_denom_len])
                    .map_err(|_| "invalid out_denom utf8")?
                    .to_string();
                offset += out_denom_len;

                let min_out = u128::from_le_bytes(data[offset..offset+16].try_into().unwrap());

                Ok(CosmosAction::Swap {
                    pool_id,
                    token_in: Coin::new(in_denom, in_amount),
                    token_out_denom: out_denom,
                    min_amount_out: min_out,
                })
            }
            _ => Err("unknown action type"),
        }
    }

    fn to_bytes(&self, action: &Self::Action) -> Vec<u8> {
        let mut buf = Vec::new();
        match action {
            CosmosAction::Send { to, amount } => {
                buf.push(0);
                buf.extend_from_slice(&to.bytes);
                buf.extend_from_slice(&(amount.len() as u32).to_le_bytes());
                for coin in amount {
                    buf.extend_from_slice(&(coin.denom.len() as u32).to_le_bytes());
                    buf.extend_from_slice(coin.denom.as_bytes());
                    buf.extend_from_slice(&coin.amount.to_le_bytes());
                }
            }
            CosmosAction::IbcTransfer { channel, to, token } => {
                buf.push(1);
                buf.extend_from_slice(&(channel.len() as u32).to_le_bytes());
                buf.extend_from_slice(channel.as_bytes());
                buf.extend_from_slice(&(to.len() as u32).to_le_bytes());
                buf.extend_from_slice(to.as_bytes());
                buf.extend_from_slice(&(token.denom.len() as u32).to_le_bytes());
                buf.extend_from_slice(token.denom.as_bytes());
                buf.extend_from_slice(&token.amount.to_le_bytes());
            }
            CosmosAction::Swap {
                pool_id,
                token_in,
                token_out_denom,
                min_amount_out,
            } => {
                buf.push(2);
                buf.extend_from_slice(&pool_id.to_le_bytes());
                buf.extend_from_slice(&(token_in.denom.len() as u32).to_le_bytes());
                buf.extend_from_slice(token_in.denom.as_bytes());
                buf.extend_from_slice(&token_in.amount.to_le_bytes());
                buf.extend_from_slice(&(token_out_denom.len() as u32).to_le_bytes());
                buf.extend_from_slice(token_out_denom.as_bytes());
                buf.extend_from_slice(&min_amount_out.to_le_bytes());
            }
            CosmosAction::ProvideLiquidity { .. } => {
                buf.push(3);
                // TODO: implement
            }
            CosmosAction::WithdrawLiquidity { .. } => {
                buf.push(4);
                // TODO: implement
            }
        }
        buf
    }

    fn validate(&self, action: &Self::Action) -> Result<(), Self::Error> {
        match action {
            CosmosAction::Send { amount, .. } => {
                if amount.is_empty() {
                    return Err("must send at least one coin");
                }
                for coin in amount {
                    if coin.amount == 0 {
                        return Err("amount must be > 0");
                    }
                }
            }
            CosmosAction::IbcTransfer { token, channel, .. } => {
                if token.amount == 0 {
                    return Err("amount must be > 0");
                }
                if channel.is_empty() {
                    return Err("channel required");
                }
            }
            CosmosAction::Swap {
                token_in,
                min_amount_out,
                ..
            } => {
                if token_in.amount == 0 {
                    return Err("token_in amount must be > 0");
                }
                if *min_amount_out == 0 {
                    return Err("min_amount_out must be > 0");
                }
            }
            CosmosAction::ProvideLiquidity { tokens, .. } => {
                if tokens.is_empty() {
                    return Err("must provide at least one token");
                }
            }
            CosmosAction::WithdrawLiquidity { shares, .. } => {
                if *shares == 0 {
                    return Err("shares must be > 0");
                }
            }
        }
        Ok(())
    }

    fn describe(&self, action: &Self::Action) -> String {
        match action {
            CosmosAction::Send { to, amount } => {
                let coins: Vec<String> = amount
                    .iter()
                    .map(|c| format!("{} {}", c.amount, c.denom))
                    .collect();
                format!("send {} to {}", coins.join(", "), to.to_bech32())
            }
            CosmosAction::IbcTransfer { channel, to, token } => {
                format!(
                    "ibc {} {} to {} via {}",
                    token.amount, token.denom, to, channel
                )
            }
            CosmosAction::Swap {
                pool_id,
                token_in,
                token_out_denom,
                ..
            } => {
                format!(
                    "swap {} {} for {} on pool {}",
                    token_in.amount, token_in.denom, token_out_denom, pool_id
                )
            }
            CosmosAction::ProvideLiquidity { pool_id, .. } => {
                format!("provide liquidity to pool {}", pool_id)
            }
            CosmosAction::WithdrawLiquidity { pool_id, shares } => {
                format!("withdraw {} shares from pool {}", shares, pool_id)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_adapter_creation() {
        let adapter = CosmosAdapter::osmosis("https://rpc.osmosis.zone");
        assert_eq!(adapter.network_id(), "osmosis-1");
        assert_eq!(adapter.prefix(), "osmo");

        let noble = CosmosAdapter::noble("https://rpc.noble.xyz");
        assert_eq!(noble.network_id(), "noble-1");
        assert_eq!(noble.prefix(), "noble");
    }

    #[test]
    fn test_address() {
        let addr = CosmosAddress::osmosis([1u8; 20]);
        assert_eq!(addr.prefix, "osmo");
        assert!(addr.to_bech32().starts_with("osmo1"));

        let noble_addr = CosmosAddress::noble([2u8; 20]);
        assert!(noble_addr.to_bech32().starts_with("noble1"));
    }

    #[test]
    fn test_coin() {
        let osmo = Coin::osmo(1_000_000);
        assert_eq!(osmo.denom, "uosmo");
        assert_eq!(osmo.amount, 1_000_000);

        let usdc = Coin::usdc(100_000_000);
        assert_eq!(usdc.denom, "uusdc");
    }

    #[test]
    fn test_action_roundtrip() {
        let builder = CosmosActionBuilder::osmosis();

        let action = CosmosAction::Send {
            to: CosmosAddress::osmosis([1u8; 20]),
            amount: vec![Coin::osmo(1_000_000)],
        };

        let bytes = builder.to_bytes(&action);
        let recovered = builder.build_from_bytes(&bytes).unwrap();

        if let CosmosAction::Send { to, amount } = recovered {
            assert_eq!(to.bytes, [1u8; 20]);
            assert_eq!(amount.len(), 1);
            assert_eq!(amount[0].amount, 1_000_000);
        } else {
            panic!("wrong action type");
        }
    }

    #[test]
    fn test_ibc_roundtrip() {
        let builder = CosmosActionBuilder::osmosis();

        let action = CosmosAction::IbcTransfer {
            channel: "channel-0".into(),
            to: "noble1abc123".into(),
            token: Coin::usdc(100_000_000),
        };

        let bytes = builder.to_bytes(&action);
        let recovered = builder.build_from_bytes(&bytes).unwrap();

        if let CosmosAction::IbcTransfer { channel, to, token } = recovered {
            assert_eq!(channel, "channel-0");
            assert_eq!(to, "noble1abc123");
            assert_eq!(token.denom, "uusdc");
            assert_eq!(token.amount, 100_000_000);
        } else {
            panic!("wrong action type");
        }
    }

    #[test]
    fn test_swap_roundtrip() {
        let builder = CosmosActionBuilder::osmosis();

        let action = CosmosAction::Swap {
            pool_id: 1,
            token_in: Coin::osmo(1_000_000),
            token_out_denom: "uusdc".into(),
            min_amount_out: 900_000,
        };

        let bytes = builder.to_bytes(&action);
        let recovered = builder.build_from_bytes(&bytes).unwrap();

        if let CosmosAction::Swap {
            pool_id,
            token_in,
            token_out_denom,
            min_amount_out,
        } = recovered
        {
            assert_eq!(pool_id, 1);
            assert_eq!(token_in.denom, "uosmo");
            assert_eq!(token_out_denom, "uusdc");
            assert_eq!(min_amount_out, 900_000);
        } else {
            panic!("wrong action type");
        }
    }

    #[test]
    fn test_action_validation() {
        let builder = CosmosActionBuilder::osmosis();

        // valid send
        let valid = CosmosAction::Send {
            to: CosmosAddress::osmosis([1u8; 20]),
            amount: vec![Coin::osmo(100)],
        };
        assert!(builder.validate(&valid).is_ok());

        // invalid: empty amount
        let invalid = CosmosAction::Send {
            to: CosmosAddress::osmosis([1u8; 20]),
            amount: vec![],
        };
        assert!(builder.validate(&invalid).is_err());

        // invalid: zero amount
        let zero = CosmosAction::Send {
            to: CosmosAddress::osmosis([1u8; 20]),
            amount: vec![Coin::osmo(0)],
        };
        assert!(builder.validate(&zero).is_err());
    }
}
