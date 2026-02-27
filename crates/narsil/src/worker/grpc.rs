//! grpc-based offchain workers
//!
//! implementations for penumbra, zcash (zidecar), and cosmos chains.
//! all use tonic for grpc transport.

use alloc::string::String;
use alloc::vec::Vec;

#[cfg(feature = "std")]
use std::sync::Arc;

use crate::worker::{
    ChainQuery, ChainResponse, ChainEvent, EventFilter,
    SubmitRequest, SubmitResponse, OffchainWorker,
    TxStatusResponse, WorkerError,
};
use crate::wire::Hash32;

/// grpc worker configuration
#[derive(Clone, Debug)]
pub struct GrpcConfig {
    /// endpoint url
    pub endpoint: String,
    /// timeout in milliseconds
    pub timeout_ms: u64,
    /// chain type for protocol-specific handling
    pub chain_type: ChainType,
}

/// supported chain types
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ChainType {
    /// penumbra (view service + tendermint proxy)
    Penumbra,
    /// zcash via zidecar
    Zidecar,
    /// cosmos sdk chains (osmosis, noble, etc)
    Cosmos,
}

impl GrpcConfig {
    /// create config for penumbra
    pub fn penumbra(endpoint: impl Into<String>) -> Self {
        Self {
            endpoint: endpoint.into(),
            timeout_ms: 30_000,
            chain_type: ChainType::Penumbra,
        }
    }

    /// create config for zidecar
    pub fn zidecar(endpoint: impl Into<String>) -> Self {
        Self {
            endpoint: endpoint.into(),
            timeout_ms: 30_000,
            chain_type: ChainType::Zidecar,
        }
    }

    /// create config for cosmos
    pub fn cosmos(endpoint: impl Into<String>) -> Self {
        Self {
            endpoint: endpoint.into(),
            timeout_ms: 30_000,
            chain_type: ChainType::Cosmos,
        }
    }

    /// set timeout
    pub fn with_timeout(mut self, ms: u64) -> Self {
        self.timeout_ms = ms;
        self
    }
}

/// grpc-based offchain worker
///
/// handles penumbra, zcash (zidecar), and cosmos chains
#[cfg(feature = "net")]
pub struct GrpcWorker {
    config: GrpcConfig,
    // tonic channel would go here
    // channel: tonic::transport::Channel,
}

#[cfg(feature = "net")]
impl GrpcWorker {
    /// connect to grpc endpoint
    pub async fn connect(config: GrpcConfig) -> Result<Self, WorkerError> {
        // TODO: establish tonic channel
        // let channel = tonic::transport::Channel::from_shared(config.endpoint.clone())
        //     .map_err(|e| WorkerError::NetworkError(e.to_string()))?
        //     .timeout(std::time::Duration::from_millis(config.timeout_ms))
        //     .connect()
        //     .await
        //     .map_err(|e| WorkerError::NetworkError(e.to_string()))?;

        Ok(Self { config })
    }

    /// get chain type
    pub fn chain_type(&self) -> ChainType {
        self.config.chain_type
    }

    /// query via penumbra view service
    async fn query_penumbra(&self, query: ChainQuery) -> ChainResponse {
        match query {
            ChainQuery::Height => {
                // call ViewService.Status()
                // returns StatusResponse with full_sync_height
                ChainResponse::Height(0) // TODO
            }
            ChainQuery::Balance { address, asset } => {
                // call ViewService.Balances()
                // filter by address_index and asset_id
                let _ = (address, asset);
                ChainResponse::Balance(vec![]) // TODO
            }
            ChainQuery::TxStatus { hash } => {
                // call ViewService.TransactionInfoByHash()
                let _ = hash;
                ChainResponse::TxStatus(TxStatusResponse::Unknown) // TODO
            }
            _ => ChainResponse::Error(WorkerError::InvalidResponse),
        }
    }

    /// submit via penumbra tendermint proxy
    async fn submit_penumbra(&self, signed_tx: Vec<u8>) -> SubmitResponse {
        // call TendermintProxyService.BroadcastTxSync()
        // or ViewService.BroadcastTransaction() if we have view access
        let _ = signed_tx;
        SubmitResponse::Error(WorkerError::NetworkError("not implemented".into()))
    }

    /// query via zidecar
    async fn query_zidecar(&self, query: ChainQuery) -> ChainResponse {
        match query {
            ChainQuery::Height => {
                // call Zidecar.GetTip()
                // returns BlockId with height
                ChainResponse::Height(0) // TODO
            }
            ChainQuery::TxStatus { hash } => {
                // call Zidecar.GetTransaction()
                let _ = hash;
                ChainResponse::TxStatus(TxStatusResponse::Unknown) // TODO
            }
            ChainQuery::StateProof { key, height } => {
                // call Zidecar.GetCommitmentProof() or GetNullifierProof()
                let _ = (key, height);
                ChainResponse::StateProof { value: None, proof: vec![] } // TODO
            }
            _ => ChainResponse::Error(WorkerError::InvalidResponse),
        }
    }

    /// submit via zidecar
    async fn submit_zidecar(&self, signed_tx: Vec<u8>) -> SubmitResponse {
        // call Zidecar.SendTransaction()
        let _ = signed_tx;
        SubmitResponse::Error(WorkerError::NetworkError("not implemented".into()))
    }

    /// query via cosmos grpc
    async fn query_cosmos(&self, query: ChainQuery) -> ChainResponse {
        match query {
            ChainQuery::Height => {
                // call cosmos.base.tendermint.v1beta1.GetLatestBlock()
                ChainResponse::Height(0) // TODO
            }
            ChainQuery::Balance { address, asset } => {
                // call cosmos.bank.v1beta1.Balance()
                let _ = (address, asset);
                ChainResponse::Balance(vec![]) // TODO
            }
            ChainQuery::TxStatus { hash } => {
                // call cosmos.tx.v1beta1.GetTx()
                let _ = hash;
                ChainResponse::TxStatus(TxStatusResponse::Unknown) // TODO
            }
            _ => ChainResponse::Error(WorkerError::InvalidResponse),
        }
    }

    /// submit via cosmos grpc
    async fn submit_cosmos(&self, signed_tx: Vec<u8>) -> SubmitResponse {
        // call cosmos.tx.v1beta1.BroadcastTx()
        let _ = signed_tx;
        SubmitResponse::Error(WorkerError::NetworkError("not implemented".into()))
    }
}

// sync wrapper for the trait (blocks on async)
#[cfg(feature = "net")]
impl OffchainWorker for GrpcWorker {
    fn query(&self, chain: &str, query: ChainQuery) -> ChainResponse {
        // in real impl, would use tokio runtime
        let _ = chain;
        match self.config.chain_type {
            ChainType::Penumbra => ChainResponse::Height(0), // TODO: block on async
            ChainType::Zidecar => ChainResponse::Height(0),
            ChainType::Cosmos => ChainResponse::Height(0),
        }
    }

    fn submit(&self, request: SubmitRequest) -> SubmitResponse {
        let _ = request;
        SubmitResponse::Error(WorkerError::NetworkError("use async interface".into()))
    }

    fn subscribe(&self, _filter: EventFilter) -> Box<dyn Iterator<Item = ChainEvent> + Send> {
        Box::new(core::iter::empty())
    }

    fn supports(&self, chain: &str) -> bool {
        match self.config.chain_type {
            ChainType::Penumbra => chain == "penumbra",
            ChainType::Zidecar => chain == "zcash",
            ChainType::Cosmos => chain == "osmosis" || chain == "noble" || chain == "cosmos",
        }
    }

    fn chains(&self) -> Vec<String> {
        match self.config.chain_type {
            ChainType::Penumbra => vec!["penumbra".into()],
            ChainType::Zidecar => vec!["zcash".into()],
            ChainType::Cosmos => vec!["osmosis".into(), "noble".into(), "cosmos".into()],
        }
    }
}

/// async worker trait for proper grpc usage
#[cfg(feature = "net")]
#[allow(async_fn_in_trait)]
pub trait AsyncOffchainWorker: Send + Sync {
    /// query chain state (async)
    async fn query(&self, query: ChainQuery) -> ChainResponse;

    /// submit signed transaction (async)
    async fn submit(&self, signed_tx: Vec<u8>, wait: bool) -> SubmitResponse;

    /// get current height
    async fn height(&self) -> Result<u64, WorkerError> {
        match self.query(ChainQuery::Height).await {
            ChainResponse::Height(h) => Ok(h),
            ChainResponse::Error(e) => Err(e),
            _ => Err(WorkerError::InvalidResponse),
        }
    }

    /// check tx status
    async fn tx_status(&self, hash: Hash32) -> Result<TxStatusResponse, WorkerError> {
        match self.query(ChainQuery::TxStatus { hash }).await {
            ChainResponse::TxStatus(s) => Ok(s),
            ChainResponse::Error(e) => Err(e),
            _ => Err(WorkerError::InvalidResponse),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_config_creation() {
        let penumbra = GrpcConfig::penumbra("http://localhost:8080");
        assert_eq!(penumbra.chain_type, ChainType::Penumbra);

        let zidecar = GrpcConfig::zidecar("http://localhost:50051");
        assert_eq!(zidecar.chain_type, ChainType::Zidecar);

        let cosmos = GrpcConfig::cosmos("http://localhost:9090")
            .with_timeout(60_000);
        assert_eq!(cosmos.chain_type, ChainType::Cosmos);
        assert_eq!(cosmos.timeout_ms, 60_000);
    }
}
