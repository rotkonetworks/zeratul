//! chain configuration and endpoints

use serde::{Deserialize, Serialize};

/// known chain configurations
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ChainConfig {
    /// chain name
    pub name: String,
    /// rpc websocket endpoint
    pub rpc_endpoint: String,
    /// chain spec for light client (if available)
    pub chain_spec: Option<String>,
    /// ss58 address prefix
    pub ss58_prefix: u16,
    /// token decimals
    pub decimals: u8,
    /// token symbol
    pub symbol: String,
    /// para id (for parachains)
    pub para_id: Option<u32>,
}

impl ChainConfig {
    /// ghettobox main chain
    pub fn ghettobox() -> Self {
        Self {
            name: "ghettobox".into(),
            rpc_endpoint: "wss://ghettobox.rotko.net".into(),
            chain_spec: None, // would include embedded chain spec for light client
            ss58_prefix: 42,
            decimals: 12,
            symbol: "GHETTO".into(),
            para_id: None, // standalone or relay
        }
    }

    /// asset hub polkadot
    pub fn asset_hub_polkadot() -> Self {
        Self {
            name: "asset-hub-polkadot".into(),
            rpc_endpoint: "wss://asset-hub-polkadot.rotko.net".into(),
            chain_spec: None,
            ss58_prefix: 0,
            decimals: 10,
            symbol: "DOT".into(),
            para_id: Some(1000),
        }
    }

    /// asset hub kusama
    pub fn asset_hub_kusama() -> Self {
        Self {
            name: "asset-hub-kusama".into(),
            rpc_endpoint: "wss://asset-hub-kusama.rotko.net".into(),
            chain_spec: None,
            ss58_prefix: 2,
            decimals: 12,
            symbol: "KSM".into(),
            para_id: Some(1000),
        }
    }

    /// polkadot relay
    pub fn polkadot() -> Self {
        Self {
            name: "polkadot".into(),
            rpc_endpoint: "wss://polkadot.rotko.net".into(),
            chain_spec: None,
            ss58_prefix: 0,
            decimals: 10,
            symbol: "DOT".into(),
            para_id: None,
        }
    }

    /// kusama relay
    pub fn kusama() -> Self {
        Self {
            name: "kusama".into(),
            rpc_endpoint: "wss://kusama.rotko.net".into(),
            chain_spec: None,
            ss58_prefix: 2,
            decimals: 12,
            symbol: "KSM".into(),
            para_id: None,
        }
    }
}

/// connection mode
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ConnectionMode {
    /// use rpc endpoint (faster, requires trust)
    #[default]
    Rpc,
    /// use light client (trustless, slower startup)
    LightClient,
}

/// client configuration
#[derive(Clone, Debug)]
pub struct ClientConfig {
    /// chain to connect to
    pub chain: ChainConfig,
    /// connection mode
    pub mode: ConnectionMode,
    /// request timeout in seconds
    pub timeout_secs: u64,
    /// retry attempts on failure
    pub retry_attempts: u32,
}

impl Default for ClientConfig {
    fn default() -> Self {
        Self {
            chain: ChainConfig::ghettobox(),
            mode: ConnectionMode::Rpc,
            timeout_secs: 30,
            retry_attempts: 3,
        }
    }
}

impl ClientConfig {
    pub fn ghettobox_rpc() -> Self {
        Self {
            chain: ChainConfig::ghettobox(),
            mode: ConnectionMode::Rpc,
            ..Default::default()
        }
    }

    pub fn asset_hub_rpc() -> Self {
        Self {
            chain: ChainConfig::asset_hub_polkadot(),
            mode: ConnectionMode::Rpc,
            ..Default::default()
        }
    }

    #[cfg(feature = "light-client")]
    pub fn ghettobox_light() -> Self {
        Self {
            chain: ChainConfig::ghettobox(),
            mode: ConnectionMode::LightClient,
            ..Default::default()
        }
    }
}

/// supported assets for deposits/withdrawals
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub enum Asset {
    /// native token of the chain
    Native,
    /// asset from asset hub (asset id)
    AssetHub { asset_id: u32 },
    /// usdt on asset hub
    Usdt,
    /// usdc on asset hub
    Usdc,
    /// wrapped eth via hyperbridge
    Weth,
    /// ibc token from cosmos
    Ibc { channel: String, denom: String },
}

impl Asset {
    pub fn symbol(&self) -> &str {
        match self {
            Asset::Native => "GHETTO",
            Asset::AssetHub { .. } => "ASSET",
            Asset::Usdt => "USDT",
            Asset::Usdc => "USDC",
            Asset::Weth => "WETH",
            Asset::Ibc { denom, .. } => denom,
        }
    }

    pub fn decimals(&self) -> u8 {
        match self {
            Asset::Native => 12,
            Asset::AssetHub { .. } => 10,
            Asset::Usdt => 6,
            Asset::Usdc => 6,
            Asset::Weth => 18,
            Asset::Ibc { .. } => 6, // default
        }
    }
}

/// destination for cross-chain transfers
#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum TransferDestination {
    /// same chain transfer
    Local { address: String },
    /// xcm to another parachain
    Xcm {
        para_id: u32,
        address: String,
    },
    /// xcm to relay chain
    XcmRelay { address: String },
    /// ibc to cosmos chain
    Ibc {
        channel: String,
        address: String,
    },
    /// hyperbridge to ethereum
    Hyperbridge {
        chain_id: u64,
        address: String,
    },
}
