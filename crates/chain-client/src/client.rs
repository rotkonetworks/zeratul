//! chain client implementation
//!
//! supports RPC and light client modes

use crate::{
    config::{Asset, ChainConfig, ClientConfig, ConnectionMode, TransferDestination},
    error::{ChainError, Result},
};

use parity_scale_codec::{Decode, Encode};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tokio::sync::RwLock;

/// connection state
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ConnectionState {
    Disconnected,
    Connecting,
    Syncing { current: u32, target: u32 },
    Connected { finalized: u32 },
    Error(String),
}

impl Default for ConnectionState {
    fn default() -> Self {
        Self::Disconnected
    }
}

/// account balance info
#[derive(Clone, Debug, Default, Encode, Decode, Serialize, Deserialize)]
pub struct AccountBalance {
    pub free: u128,
    pub reserved: u128,
    pub frozen: u128,
}

impl AccountBalance {
    pub fn transferable(&self) -> u128 {
        self.free.saturating_sub(self.frozen)
    }

    /// format balance with decimals
    pub fn format(&self, decimals: u8) -> String {
        let divisor = 10u128.pow(decimals as u32);
        let whole = self.free / divisor;
        let frac = self.free % divisor;
        format!("{}.{:0>width$}", whole, frac, width = decimals as usize)
    }
}

/// transaction status
#[derive(Clone, Debug)]
pub enum TxStatus {
    Pending,
    InBlock { block_hash: [u8; 32], extrinsic_index: u32 },
    Finalized { block_hash: [u8; 32] },
    Failed { error: String },
}

/// chain client
pub struct ChainClient {
    config: ClientConfig,
    state: Arc<RwLock<ConnectionState>>,
    // in real impl: would hold subxt::OnlineClient or LightClient
}

impl ChainClient {
    /// create new client with config
    pub fn new(config: ClientConfig) -> Self {
        Self {
            config,
            state: Arc::new(RwLock::new(ConnectionState::Disconnected)),
        }
    }

    /// connect using RPC (default)
    pub async fn connect_rpc(endpoint: &str) -> Result<Self> {
        let config = ClientConfig {
            chain: ChainConfig {
                rpc_endpoint: endpoint.to_string(),
                ..ChainConfig::ghettobox()
            },
            mode: ConnectionMode::Rpc,
            ..Default::default()
        };

        let client = Self::new(config);
        client.connect().await?;
        Ok(client)
    }

    /// connect using light client
    #[cfg(feature = "light-client")]
    pub async fn connect_light(chain_name: &str) -> Result<Self> {
        let chain = match chain_name {
            "ghettobox" => ChainConfig::ghettobox(),
            "asset-hub" | "asset-hub-polkadot" => ChainConfig::asset_hub_polkadot(),
            "polkadot" => ChainConfig::polkadot(),
            _ => return Err(ChainError::ConnectionFailed(format!("unknown chain: {}", chain_name))),
        };

        let config = ClientConfig {
            chain,
            mode: ConnectionMode::LightClient,
            ..Default::default()
        };

        let client = Self::new(config);
        client.connect().await?;
        Ok(client)
    }

    /// connect to chain
    pub async fn connect(&self) -> Result<()> {
        *self.state.write().await = ConnectionState::Connecting;

        match self.config.mode {
            ConnectionMode::Rpc => self.connect_rpc_internal().await,
            ConnectionMode::LightClient => self.connect_light_internal().await,
        }
    }

    async fn connect_rpc_internal(&self) -> Result<()> {
        // in real impl: use subxt::OnlineClient::from_url
        tracing::info!("connecting to {} via RPC", self.config.chain.rpc_endpoint);

        // simulate connection
        *self.state.write().await = ConnectionState::Connected { finalized: 1000 };
        Ok(())
    }

    async fn connect_light_internal(&self) -> Result<()> {
        #[cfg(feature = "light-client")]
        {
            // in real impl: use subxt_lightclient
            tracing::info!("connecting to {} via light client", self.config.chain.name);
            *self.state.write().await = ConnectionState::Syncing { current: 0, target: 1000 };
            // simulate sync
            *self.state.write().await = ConnectionState::Connected { finalized: 1000 };
            Ok(())
        }

        #[cfg(not(feature = "light-client"))]
        {
            Err(ChainError::ConnectionFailed("light client feature not enabled".into()))
        }
    }

    /// get connection state
    pub async fn state(&self) -> ConnectionState {
        self.state.read().await.clone()
    }

    /// check if connected
    pub async fn is_connected(&self) -> bool {
        matches!(*self.state.read().await, ConnectionState::Connected { .. })
    }

    /// get account balance
    pub async fn get_balance(&self, address: &str) -> Result<AccountBalance> {
        if !self.is_connected().await {
            return Err(ChainError::NotConnected);
        }

        // in real impl: query System.Account storage
        tracing::debug!("querying balance for {}", address);

        // mock balance for testing
        Ok(AccountBalance {
            free: 1_000_000_000_000, // 1.0 token
            reserved: 0,
            frozen: 0,
        })
    }

    /// get asset balance (for non-native assets)
    pub async fn get_asset_balance(&self, address: &str, asset: &Asset) -> Result<u128> {
        if !self.is_connected().await {
            return Err(ChainError::NotConnected);
        }

        match asset {
            Asset::Native => Ok(self.get_balance(address).await?.free),
            Asset::AssetHub { asset_id } => {
                // query Assets.Account
                tracing::debug!("querying asset {} balance for {}", asset_id, address);
                Ok(0)
            }
            Asset::Usdt => {
                // usdt is asset 1984 on asset hub
                tracing::debug!("querying USDT balance for {}", address);
                Ok(0)
            }
            Asset::Usdc => {
                // usdc is asset 1337 on asset hub
                tracing::debug!("querying USDC balance for {}", address);
                Ok(0)
            }
            Asset::Weth => {
                // hyperbridge wrapped eth
                tracing::debug!("querying WETH balance for {}", address);
                Ok(0)
            }
            Asset::Ibc { channel, denom } => {
                // ibc token balance
                tracing::debug!("querying IBC {}/{} balance for {}", channel, denom, address);
                Ok(0)
            }
        }
    }

    /// transfer native tokens
    pub async fn transfer(
        &self,
        from_keypair: &[u8; 64], // ed25519 keypair
        to: &str,
        amount: u128,
    ) -> Result<[u8; 32]> {
        if !self.is_connected().await {
            return Err(ChainError::NotConnected);
        }

        // in real impl: construct and submit Balances.transfer_allow_death
        tracing::info!("transferring {} to {}", amount, to);

        // return mock tx hash
        Ok([0u8; 32])
    }

    /// deposit to poker pool
    pub async fn deposit_to_pool(
        &self,
        keypair: &[u8; 64],
        amount: u128,
    ) -> Result<[u8; 32]> {
        if !self.is_connected().await {
            return Err(ChainError::NotConnected);
        }

        // in real impl: call PokerPool.deposit
        tracing::info!("depositing {} to poker pool", amount);
        Ok([0u8; 32])
    }

    /// withdraw from poker pool
    pub async fn withdraw_from_pool(
        &self,
        keypair: &[u8; 64],
        amount: u128,
    ) -> Result<[u8; 32]> {
        if !self.is_connected().await {
            return Err(ChainError::NotConnected);
        }

        // in real impl: call PokerPool.withdraw
        tracing::info!("withdrawing {} from poker pool", amount);
        Ok([0u8; 32])
    }

    /// get poker pool balance
    pub async fn get_pool_balance(&self, address: &str) -> Result<u128> {
        if !self.is_connected().await {
            return Err(ChainError::NotConnected);
        }

        // in real impl: query PokerPool.Balances
        tracing::debug!("querying pool balance for {}", address);
        Ok(500_000_000_000) // 0.5 token
    }

    /// get chain config
    pub fn chain(&self) -> &ChainConfig {
        &self.config.chain
    }

    /// get connection mode
    pub fn mode(&self) -> ConnectionMode {
        self.config.mode
    }
}

/// multi-chain client for managing connections to multiple chains
pub struct MultiChainClient {
    clients: std::collections::HashMap<String, ChainClient>,
}

impl MultiChainClient {
    pub fn new() -> Self {
        Self {
            clients: std::collections::HashMap::new(),
        }
    }

    /// add a chain connection
    pub async fn add_chain(&mut self, name: &str, config: ClientConfig) -> Result<()> {
        let client = ChainClient::new(config);
        client.connect().await?;
        self.clients.insert(name.to_string(), client);
        Ok(())
    }

    /// get client for chain
    pub fn get(&self, name: &str) -> Option<&ChainClient> {
        self.clients.get(name)
    }

    /// get mutable client for chain
    pub fn get_mut(&mut self, name: &str) -> Option<&mut ChainClient> {
        self.clients.get_mut(name)
    }

    /// list connected chains
    pub fn chains(&self) -> Vec<&str> {
        self.clients.keys().map(|s| s.as_str()).collect()
    }
}

impl Default for MultiChainClient {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_client_creation() {
        let client = ChainClient::new(ClientConfig::ghettobox_rpc());
        assert!(matches!(client.state().await, ConnectionState::Disconnected));
    }

    #[tokio::test]
    async fn test_client_connect() {
        let client = ChainClient::new(ClientConfig::ghettobox_rpc());
        client.connect().await.unwrap();
        assert!(client.is_connected().await);
    }

    #[tokio::test]
    async fn test_balance_query() {
        let client = ChainClient::new(ClientConfig::ghettobox_rpc());
        client.connect().await.unwrap();

        let balance = client.get_balance("5GrwvaEF5zXb26Fz9rcQpDWS57CtERHpNehXCPcNoHGKutQY").await.unwrap();
        assert!(balance.free > 0);
    }

    #[tokio::test]
    async fn test_multi_chain() {
        let mut multi = MultiChainClient::new();
        multi.add_chain("ghettobox", ClientConfig::ghettobox_rpc()).await.unwrap();
        multi.add_chain("asset-hub", ClientConfig::asset_hub_rpc()).await.unwrap();

        assert_eq!(multi.chains().len(), 2);
        assert!(multi.get("ghettobox").is_some());
    }
}
