//! ibc integration for cosmos chains
//!
//! handles ibc transfers between ghettobox and cosmos ecosystem:
//! - osmosis: osmo, atom, and other ibc tokens
//! - penumbra-style shielded integration
//!
//! based on composable-ibc types (/steam/rotko/composable-ibc)
//!
//! ibc channels:
//! - ghettobox <-> osmosis: channel-0 (ghettobox side) / channel-XXX (osmosis side)
//!
//! references:
//! - MsgTransfer: ibc/modules/src/applications/transfer/msgs/transfer.rs
//! - PacketData: ibc/modules/src/applications/transfer/packet.rs
//! - Denom: ibc/modules/src/applications/transfer/denom.rs

use crate::error::{ChainError, Result};
use parity_scale_codec::{Decode, Encode};
use serde::{Deserialize, Serialize};

/// ibc port identifier
pub const TRANSFER_PORT: &str = "transfer";

/// known cosmos chain ids
pub mod chains {
    pub const OSMOSIS: &str = "osmosis-1";
    pub const COSMOSHUB: &str = "cosmoshub-4";
    pub const PENUMBRA: &str = "penumbra-1";
}

/// known ibc channels from ghettobox
pub mod channels {
    /// ghettobox -> osmosis
    pub const TO_OSMOSIS: &str = "channel-0";
    /// ghettobox -> cosmos hub (via osmosis relay)
    pub const TO_COSMOSHUB: &str = "channel-1";
}

/// ibc denom trace
#[derive(Clone, Debug, Serialize, Deserialize, Encode, Decode)]
pub struct DenomTrace {
    /// path (port/channel/port/channel/...)
    pub path: String,
    /// base denom on origin chain
    pub base_denom: String,
}

impl DenomTrace {
    /// create native denom (no path)
    pub fn native(denom: &str) -> Self {
        Self {
            path: String::new(),
            base_denom: denom.to_string(),
        }
    }

    /// create ibc denom from channel
    pub fn ibc(channel: &str, base_denom: &str) -> Self {
        Self {
            path: format!("{}/{}", TRANSFER_PORT, channel),
            base_denom: base_denom.to_string(),
        }
    }

    /// get full ibc denom (ibc/HASH)
    pub fn ibc_denom(&self) -> String {
        if self.path.is_empty() {
            self.base_denom.clone()
        } else {
            let full_path = format!("{}/{}", self.path, self.base_denom);
            let hash = blake3::hash(full_path.as_bytes());
            format!("ibc/{}", hex::encode_upper(&hash.as_bytes()[..32]))
        }
    }
}

/// common cosmos denoms
pub mod denoms {
    pub const OSMO: &str = "uosmo";
    pub const ATOM: &str = "uatom";
    pub const USDC_NOBLE: &str = "uusdc";
}

/// ibc transfer message (MsgTransfer)
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct MsgTransfer {
    /// source port
    pub source_port: String,
    /// source channel
    pub source_channel: String,
    /// token to transfer
    pub token: IbcCoin,
    /// sender address (bech32)
    pub sender: String,
    /// receiver address
    pub receiver: String,
    /// timeout height (0 = no height timeout)
    pub timeout_height: TimeoutHeight,
    /// timeout timestamp in nanoseconds
    pub timeout_timestamp: u64,
    /// optional memo (for receiver)
    pub memo: String,
}

/// ibc coin
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct IbcCoin {
    pub denom: String,
    pub amount: String,
}

impl IbcCoin {
    pub fn new(denom: &str, amount: u128) -> Self {
        Self {
            denom: denom.to_string(),
            amount: amount.to_string(),
        }
    }
}

/// timeout height
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct TimeoutHeight {
    pub revision_number: u64,
    pub revision_height: u64,
}

impl TimeoutHeight {
    pub fn none() -> Self {
        Self::default()
    }

    pub fn at(revision: u64, height: u64) -> Self {
        Self {
            revision_number: revision,
            revision_height: height,
        }
    }
}

/// ibc packet data (fungible token transfer)
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct FungibleTokenPacketData {
    /// denom being transferred
    pub denom: String,
    /// amount as string
    pub amount: String,
    /// sender on source chain
    pub sender: String,
    /// receiver on destination chain
    pub receiver: String,
    /// optional memo
    pub memo: String,
}

/// ibc channel config
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ChannelConfig {
    /// channel id on ghettobox
    pub channel_id: String,
    /// counterparty chain id
    pub chain_id: String,
    /// counterparty channel id
    pub counterparty_channel: String,
    /// connection id
    pub connection_id: String,
    /// supported tokens
    pub tokens: Vec<String>,
    /// estimated transfer time in seconds
    pub estimated_time_secs: u32,
}

/// osmosis channel configuration
pub fn osmosis_channel() -> ChannelConfig {
    ChannelConfig {
        channel_id: channels::TO_OSMOSIS.into(),
        chain_id: chains::OSMOSIS.into(),
        counterparty_channel: "channel-XXX".into(), // would be real channel
        connection_id: "connection-0".into(),
        tokens: vec![
            denoms::OSMO.into(),
            denoms::ATOM.into(),
            denoms::USDC_NOBLE.into(),
        ],
        estimated_time_secs: 120,
    }
}

/// build ibc transfer to osmosis
pub fn build_transfer_to_osmosis(
    denom: &str,
    amount: u128,
    sender: &str,
    receiver: &str,
    timeout_mins: u64,
) -> MsgTransfer {
    // timeout is current time + timeout_mins in nanoseconds
    let timeout_ns = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos() as u64
        + (timeout_mins * 60 * 1_000_000_000);

    MsgTransfer {
        source_port: TRANSFER_PORT.into(),
        source_channel: channels::TO_OSMOSIS.into(),
        token: IbcCoin::new(denom, amount),
        sender: sender.into(),
        receiver: receiver.into(),
        timeout_height: TimeoutHeight::none(),
        timeout_timestamp: timeout_ns,
        memo: String::new(),
    }
}

/// build ibc transfer from osmosis to ghettobox
pub fn build_transfer_from_osmosis(
    denom: &str,
    amount: u128,
    sender: &str,
    receiver: &str,
    timeout_mins: u64,
) -> MsgTransfer {
    let timeout_ns = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos() as u64
        + (timeout_mins * 60 * 1_000_000_000);

    MsgTransfer {
        source_port: TRANSFER_PORT.into(),
        // this would be the osmosis->ghettobox channel
        source_channel: "channel-XXX".into(),
        token: IbcCoin::new(denom, amount),
        sender: sender.into(),
        receiver: receiver.into(),
        timeout_height: TimeoutHeight::none(),
        timeout_timestamp: timeout_ns,
        memo: format!("deposit to ghettobox poker: {}", receiver),
    }
}

/// ibc client for cosmos rpc
pub struct IbcClient {
    /// cosmos rpc endpoint
    rpc_endpoint: String,
    /// chain id
    chain_id: String,
}

impl IbcClient {
    /// create new ibc client
    pub fn new(rpc_endpoint: &str, chain_id: &str) -> Self {
        Self {
            rpc_endpoint: rpc_endpoint.to_string(),
            chain_id: chain_id.to_string(),
        }
    }

    /// connect to osmosis
    pub fn osmosis() -> Self {
        Self::new("https://rpc.osmosis.zone", chains::OSMOSIS)
    }

    /// get ibc denom trace
    pub async fn denom_trace(&self, ibc_hash: &str) -> Result<DenomTrace> {
        // in real impl: query /ibc/apps/transfer/v1/denom_traces/{hash}
        // for now return mock
        Ok(DenomTrace::native(denoms::OSMO))
    }

    /// query balance of ibc token
    pub async fn ibc_balance(&self, address: &str, denom: &str) -> Result<u128> {
        // in real impl: query /cosmos/bank/v1beta1/balances/{address}/by_denom?denom={denom}
        Ok(0)
    }

    /// estimate transfer fee
    pub async fn estimate_fee(&self, msg: &MsgTransfer) -> Result<u128> {
        // osmosis typical fee is ~0.0025 OSMO
        Ok(2500)
    }

    /// broadcast ibc transfer (returns tx hash)
    pub async fn transfer(&self, msg: MsgTransfer, _keypair: &[u8]) -> Result<String> {
        // in real impl:
        // 1. build cosmos tx with MsgTransfer
        // 2. sign with secp256k1 key
        // 3. broadcast via /cosmos/tx/v1beta1/txs

        // mock: return fake tx hash
        let tx_hash = blake3::hash(msg.memo.as_bytes());
        Ok(hex::encode_upper(&tx_hash.as_bytes()[..32]))
    }

    /// check if ibc packet was received
    pub async fn packet_received(&self, _channel: &str, _sequence: u64) -> Result<bool> {
        // query packet commitment/receipt
        Ok(false)
    }
}

/// ibc relayer status
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RelayerStatus {
    pub active: bool,
    pub pending_packets: u32,
    pub last_relayed_height: u64,
}

/// query relayer status for channel
pub async fn relayer_status(channel: &str) -> Result<RelayerStatus> {
    // in real impl: query relayer metrics
    Ok(RelayerStatus {
        active: true,
        pending_packets: 0,
        last_relayed_height: 0,
    })
}

/// address conversion utilities
pub mod address {
    use super::*;

    /// convert substrate address to cosmos bech32
    /// (this is a simplified mock - real impl needs proper key derivation)
    pub fn substrate_to_cosmos(ss58_address: &str, prefix: &str) -> Result<String> {
        // in reality: decode ss58 -> pubkey -> bech32 encode with prefix
        // for now: mock conversion
        let hash = blake3::hash(ss58_address.as_bytes());
        Ok(format!("{}1{}", prefix, hex::encode(&hash.as_bytes()[..20])))
    }

    /// cosmos address to substrate
    pub fn cosmos_to_substrate(bech32_address: &str) -> Result<String> {
        // decode bech32 -> derive ss58
        let hash = blake3::hash(bech32_address.as_bytes());
        Ok(format!("5{}", hex::encode(&hash.as_bytes()[..31])))
    }

    /// osmosis address prefix
    pub const OSMO_PREFIX: &str = "osmo";

    /// cosmos hub address prefix
    pub const COSMOS_PREFIX: &str = "cosmos";
}

/// packet acknowledgement
#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum Acknowledgement {
    Success(Vec<u8>),
    Error(String),
}

impl Acknowledgement {
    pub fn is_success(&self) -> bool {
        matches!(self, Acknowledgement::Success(_))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_denom_trace() {
        let native = DenomTrace::native("uosmo");
        assert_eq!(native.ibc_denom(), "uosmo");

        let ibc = DenomTrace::ibc("channel-0", "uatom");
        assert!(ibc.ibc_denom().starts_with("ibc/"));
    }

    #[test]
    fn test_msg_transfer() {
        let msg = build_transfer_to_osmosis(
            "uosmo",
            1_000_000,
            "ghettobox1abc...",
            "osmo1xyz...",
            10,
        );

        assert_eq!(msg.source_port, "transfer");
        assert_eq!(msg.source_channel, "channel-0");
        assert_eq!(msg.token.amount, "1000000");
    }

    #[test]
    fn test_ibc_coin() {
        let coin = IbcCoin::new("uosmo", 5_000_000);
        assert_eq!(coin.denom, "uosmo");
        assert_eq!(coin.amount, "5000000");
    }

    #[test]
    fn test_channel_config() {
        let config = osmosis_channel();
        assert_eq!(config.chain_id, "osmosis-1");
        assert!(config.tokens.contains(&"uosmo".to_string()));
    }

    #[tokio::test]
    async fn test_ibc_client() {
        let client = IbcClient::osmosis();

        let msg = build_transfer_to_osmosis(
            "uosmo",
            1_000_000,
            "ghettobox1sender",
            "osmo1receiver",
            10,
        );

        let fee = client.estimate_fee(&msg).await.unwrap();
        assert!(fee > 0);
    }

    #[test]
    fn test_address_conversion() {
        let cosmos = address::substrate_to_cosmos("5GrwvaEF...", "osmo").unwrap();
        assert!(cosmos.starts_with("osmo1"));

        let substrate = address::cosmos_to_substrate("osmo1abc...").unwrap();
        assert!(substrate.starts_with("5"));
    }
}
