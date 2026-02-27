//! subxt-based offchain worker for polkadot/substrate chains
//!
//! uses json-rpc (not libp2p) for simplicity.
//! for light client mode, consider smoldot integration.

use alloc::string::String;
use alloc::vec::Vec;

use crate::worker::{
    ChainQuery, ChainResponse, ChainEvent, EventFilter,
    SubmitRequest, SubmitResponse, OffchainWorker,
    TxStatusResponse, WorkerError,
};

/// subxt worker configuration
#[derive(Clone, Debug)]
pub struct SubxtConfig {
    /// websocket endpoint (wss://...)
    pub endpoint: String,
    /// chain name for identification
    pub chain_name: String,
    /// ss58 prefix for address encoding
    pub ss58_prefix: u16,
}

impl SubxtConfig {
    /// create config for polkadot asset hub
    pub fn asset_hub(endpoint: impl Into<String>) -> Self {
        Self {
            endpoint: endpoint.into(),
            chain_name: "polkadot-asset-hub".into(),
            ss58_prefix: 0,
        }
    }

    /// create config for kusama asset hub
    pub fn kusama_asset_hub(endpoint: impl Into<String>) -> Self {
        Self {
            endpoint: endpoint.into(),
            chain_name: "kusama-asset-hub".into(),
            ss58_prefix: 2,
        }
    }

    /// create config for westend (testnet)
    pub fn westend(endpoint: impl Into<String>) -> Self {
        Self {
            endpoint: endpoint.into(),
            chain_name: "westend".into(),
            ss58_prefix: 42,
        }
    }

    /// custom chain
    pub fn custom(
        endpoint: impl Into<String>,
        chain_name: impl Into<String>,
        ss58_prefix: u16,
    ) -> Self {
        Self {
            endpoint: endpoint.into(),
            chain_name: chain_name.into(),
            ss58_prefix,
        }
    }
}

/// subxt-based offchain worker for polkadot ecosystem
#[cfg(feature = "net")]
pub struct SubxtWorker {
    config: SubxtConfig,
    // subxt client would go here
    // client: subxt::OnlineClient<subxt::PolkadotConfig>,
}

#[cfg(feature = "net")]
impl SubxtWorker {
    /// connect to substrate node via websocket
    pub async fn connect(config: SubxtConfig) -> Result<Self, WorkerError> {
        // TODO: establish subxt connection
        // let client = subxt::OnlineClient::<subxt::PolkadotConfig>::from_url(&config.endpoint)
        //     .await
        //     .map_err(|e| WorkerError::NetworkError(e.to_string()))?;

        Ok(Self { config })
    }

    /// get chain name
    pub fn chain_name(&self) -> &str {
        &self.config.chain_name
    }

    /// query block height
    async fn query_height(&self) -> Result<u64, WorkerError> {
        // client.blocks().at_latest().await?.number()
        Ok(0) // TODO
    }

    /// query account balance
    async fn query_balance(&self, address: &[u8], asset: Option<&[u8]>) -> Result<Vec<u8>, WorkerError> {
        // for native: system.account(address).data.free
        // for assets: assets.account(asset_id, address).balance
        let _ = (address, asset);
        Ok(vec![]) // TODO
    }

    /// submit extrinsic
    async fn submit_extrinsic(&self, signed_tx: Vec<u8>, wait: bool) -> SubmitResponse {
        // client.tx().submit_and_watch(&signed_tx).await
        let _ = (signed_tx, wait);
        SubmitResponse::Error(WorkerError::NetworkError("not implemented".into()))
    }

    /// watch for tx in block
    async fn watch_tx(&self, hash: [u8; 32]) -> TxStatusResponse {
        // query system.events and filter for extrinsic
        let _ = hash;
        TxStatusResponse::Unknown
    }
}

#[cfg(feature = "net")]
impl OffchainWorker for SubxtWorker {
    fn query(&self, chain: &str, query: ChainQuery) -> ChainResponse {
        if chain != self.config.chain_name {
            return ChainResponse::Error(WorkerError::UnsupportedChain(chain.into()));
        }

        // TODO: block on async runtime
        match query {
            ChainQuery::Height => ChainResponse::Height(0),
            ChainQuery::Balance { .. } => ChainResponse::Balance(vec![]),
            ChainQuery::TxStatus { .. } => ChainResponse::TxStatus(TxStatusResponse::Unknown),
            _ => ChainResponse::Error(WorkerError::InvalidResponse),
        }
    }

    fn submit(&self, request: SubmitRequest) -> SubmitResponse {
        if request.chain != self.config.chain_name {
            return SubmitResponse::Error(WorkerError::UnsupportedChain(request.chain));
        }
        SubmitResponse::Error(WorkerError::NetworkError("use async interface".into()))
    }

    fn subscribe(&self, _filter: EventFilter) -> Box<dyn Iterator<Item = ChainEvent> + Send> {
        // TODO: use subxt subscription
        Box::new(core::iter::empty())
    }

    fn supports(&self, chain: &str) -> bool {
        chain == self.config.chain_name
    }

    fn chains(&self) -> Vec<String> {
        vec![self.config.chain_name.clone()]
    }
}

/// async interface for subxt worker
#[cfg(feature = "net")]
#[allow(async_fn_in_trait)]
pub trait AsyncSubxtWorker: Send + Sync {
    /// get current block number
    async fn height(&self) -> Result<u64, WorkerError>;

    /// get account balance (native token)
    async fn balance(&self, address: &[u8]) -> Result<u128, WorkerError>;

    /// get asset balance
    async fn asset_balance(&self, address: &[u8], asset_id: u32) -> Result<u128, WorkerError>;

    /// submit signed extrinsic
    async fn submit(&self, signed_tx: Vec<u8>) -> Result<[u8; 32], WorkerError>;

    /// submit and wait for inclusion
    async fn submit_and_watch(&self, signed_tx: Vec<u8>) -> Result<TxInBlock, WorkerError>;
}

/// transaction included in block
#[derive(Clone, Debug)]
pub struct TxInBlock {
    /// block hash
    pub block_hash: [u8; 32],
    /// block number
    pub block_number: u32,
    /// extrinsic index
    pub extrinsic_index: u32,
    /// success flag
    pub success: bool,
    /// events (encoded)
    pub events: Vec<Vec<u8>>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_config_creation() {
        let ah = SubxtConfig::asset_hub("wss://asset-hub-polkadot-rpc.dwellir.com");
        assert_eq!(ah.chain_name, "polkadot-asset-hub");
        assert_eq!(ah.ss58_prefix, 0);

        let kusama = SubxtConfig::kusama_asset_hub("wss://kusama-asset-hub-rpc.dwellir.com");
        assert_eq!(kusama.ss58_prefix, 2);

        let custom = SubxtConfig::custom("wss://my-node.local", "my-chain", 42);
        assert_eq!(custom.chain_name, "my-chain");
    }
}
