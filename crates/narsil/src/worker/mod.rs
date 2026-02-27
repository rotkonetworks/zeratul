//! offchain worker interface
//!
//! security boundary between wallet (keys) and network (untrusted).
//! the worker handles all network I/O but never sees key material.
//!
//! ```text
//! ┌──────────────────┐
//! │  WALLET (keys)   │  signs locally, never touches network
//! └────────┬─────────┘
//!          │ signed bytes only
//!          ▼
//! ┌──────────────────┐
//! │  NARSIL CORE     │  proposals, osst, coordination
//! └────────┬─────────┘
//!          │
//!          ▼
//! ┌──────────────────┐
//! │  WORKER (sandbox)│  network I/O, no keys
//! └──────────────────┘
//! ```
//!
//! if worker is compromised via malicious RPC/network, attacker
//! cannot steal keys - they never cross this boundary.
//!
//! # implementations
//!
//! - `grpc` - penumbra, zcash (zidecar), cosmos chains
//! - `subxt` - polkadot/substrate chains via json-rpc

#[cfg(feature = "std")]
pub mod grpc;
#[cfg(feature = "std")]
pub mod subxt;

#[cfg(feature = "std")]
pub use grpc::{GrpcConfig, ChainType};
#[cfg(feature = "std")]
pub use subxt::SubxtConfig;

use alloc::boxed::Box;
use alloc::string::String;
use alloc::vec::Vec;

use crate::wire::Hash32;

/// query types the worker can execute
#[derive(Clone, Debug)]
pub enum ChainQuery {
    /// get current block height
    Height,
    /// get account balance
    Balance {
        address: Vec<u8>,
        asset: Option<Vec<u8>>,
    },
    /// get transaction status
    TxStatus { hash: Hash32 },
    /// get merkle proof for state
    StateProof {
        key: Vec<u8>,
        height: Option<u64>,
    },
    /// get block header
    BlockHeader { height: u64 },
    /// custom RPC call
    Rpc {
        method: String,
        params: Vec<u8>,
    },
}

/// response from chain query
#[derive(Clone, Debug)]
pub enum ChainResponse {
    /// block height
    Height(u64),
    /// balance (raw bytes, chain-specific encoding)
    Balance(Vec<u8>),
    /// transaction status
    TxStatus(TxStatusResponse),
    /// state proof
    StateProof {
        value: Option<Vec<u8>>,
        proof: Vec<u8>,
    },
    /// block header
    BlockHeader(Vec<u8>),
    /// raw RPC response
    Rpc(Vec<u8>),
    /// error
    Error(WorkerError),
}

/// transaction status from worker
#[derive(Clone, Debug)]
pub enum TxStatusResponse {
    Unknown,
    Pending,
    Confirmed { height: u64, index: u32 },
    Failed { reason: String },
}

/// worker errors (safe to expose - no key info)
#[derive(Clone, Debug)]
pub enum WorkerError {
    /// network unreachable
    NetworkError(String),
    /// rpc returned error
    RpcError { code: i32, message: String },
    /// timeout
    Timeout,
    /// invalid response format
    InvalidResponse,
    /// rate limited
    RateLimited,
    /// chain not supported
    UnsupportedChain(String),
}

/// submit request - only signed bytes, no keys
#[derive(Clone, Debug)]
pub struct SubmitRequest {
    /// chain identifier
    pub chain: String,
    /// fully signed transaction bytes
    pub signed_tx: Vec<u8>,
    /// wait for confirmation?
    pub wait: bool,
}

/// submit response
#[derive(Clone, Debug)]
pub enum SubmitResponse {
    /// accepted into mempool
    Accepted { hash: Hash32 },
    /// confirmed in block
    Confirmed { hash: Hash32, height: u64 },
    /// rejected
    Rejected { reason: String },
    /// error submitting
    Error(WorkerError),
}

/// event subscription filter
#[derive(Clone, Debug)]
pub struct EventFilter {
    /// chain to watch
    pub chain: String,
    /// address to watch (optional)
    pub address: Option<Vec<u8>>,
    /// event types
    pub event_types: Vec<String>,
}

/// chain event from subscription
#[derive(Clone, Debug)]
pub struct ChainEvent {
    /// chain identifier
    pub chain: String,
    /// block height
    pub height: u64,
    /// event type
    pub event_type: String,
    /// event data
    pub data: Vec<u8>,
    /// affected address (if any)
    pub address: Option<Vec<u8>>,
}

/// offchain worker trait - the security boundary
///
/// implementations can be:
/// - separate process (IPC)
/// - wasm sandbox
/// - remote service
/// - in-process (for testing)
#[cfg(feature = "std")]
pub trait OffchainWorker: Send + Sync {
    /// query chain state
    fn query(&self, chain: &str, query: ChainQuery) -> ChainResponse;

    /// submit signed transaction (no keys!)
    fn submit(&self, request: SubmitRequest) -> SubmitResponse;

    /// subscribe to chain events
    fn subscribe(&self, filter: EventFilter) -> Box<dyn Iterator<Item = ChainEvent> + Send>;

    /// check if chain is supported
    fn supports(&self, chain: &str) -> bool;

    /// list supported chains
    fn chains(&self) -> Vec<String>;
}

/// worker handle for async usage
#[cfg(feature = "std")]
pub struct WorkerHandle {
    inner: Box<dyn OffchainWorker>,
}

#[cfg(feature = "std")]
impl WorkerHandle {
    /// create from worker implementation
    pub fn new(worker: impl OffchainWorker + 'static) -> Self {
        Self {
            inner: Box::new(worker),
        }
    }

    /// query chain
    pub fn query(&self, chain: &str, query: ChainQuery) -> ChainResponse {
        self.inner.query(chain, query)
    }

    /// submit signed tx
    pub fn submit(&self, request: SubmitRequest) -> SubmitResponse {
        self.inner.submit(request)
    }

    /// get current height
    pub fn height(&self, chain: &str) -> Result<u64, WorkerError> {
        match self.inner.query(chain, ChainQuery::Height) {
            ChainResponse::Height(h) => Ok(h),
            ChainResponse::Error(e) => Err(e),
            _ => Err(WorkerError::InvalidResponse),
        }
    }

    /// get balance
    pub fn balance(&self, chain: &str, address: Vec<u8>) -> Result<Vec<u8>, WorkerError> {
        match self.inner.query(chain, ChainQuery::Balance { address, asset: None }) {
            ChainResponse::Balance(b) => Ok(b),
            ChainResponse::Error(e) => Err(e),
            _ => Err(WorkerError::InvalidResponse),
        }
    }

    /// check tx status
    pub fn tx_status(&self, chain: &str, hash: Hash32) -> Result<TxStatusResponse, WorkerError> {
        match self.inner.query(chain, ChainQuery::TxStatus { hash }) {
            ChainResponse::TxStatus(s) => Ok(s),
            ChainResponse::Error(e) => Err(e),
            _ => Err(WorkerError::InvalidResponse),
        }
    }
}

/// mock worker for testing
#[cfg(feature = "std")]
pub struct MockWorker {
    chains: Vec<String>,
    height: u64,
}

#[cfg(feature = "std")]
impl MockWorker {
    pub fn new(chains: Vec<String>) -> Self {
        Self { chains, height: 1000 }
    }

    pub fn set_height(&mut self, height: u64) {
        self.height = height;
    }
}

#[cfg(feature = "std")]
impl OffchainWorker for MockWorker {
    fn query(&self, chain: &str, query: ChainQuery) -> ChainResponse {
        if !self.supports(chain) {
            return ChainResponse::Error(WorkerError::UnsupportedChain(chain.into()));
        }

        match query {
            ChainQuery::Height => ChainResponse::Height(self.height),
            ChainQuery::Balance { .. } => ChainResponse::Balance(vec![0; 16]),
            ChainQuery::TxStatus { .. } => {
                ChainResponse::TxStatus(TxStatusResponse::Confirmed { height: self.height, index: 0 })
            }
            ChainQuery::StateProof { .. } => {
                ChainResponse::StateProof { value: Some(vec![]), proof: vec![] }
            }
            ChainQuery::BlockHeader { height } => {
                ChainResponse::BlockHeader(height.to_le_bytes().to_vec())
            }
            ChainQuery::Rpc { .. } => ChainResponse::Rpc(vec![]),
        }
    }

    fn submit(&self, request: SubmitRequest) -> SubmitResponse {
        if !self.supports(&request.chain) {
            return SubmitResponse::Error(WorkerError::UnsupportedChain(request.chain));
        }

        // mock: hash is sha256 of tx
        use sha2::{Sha256, Digest};
        let mut hasher = Sha256::new();
        hasher.update(&request.signed_tx);
        let hash: [u8; 32] = hasher.finalize().into();

        if request.wait {
            SubmitResponse::Confirmed { hash, height: self.height }
        } else {
            SubmitResponse::Accepted { hash }
        }
    }

    fn subscribe(&self, _filter: EventFilter) -> Box<dyn Iterator<Item = ChainEvent> + Send> {
        Box::new(core::iter::empty())
    }

    fn supports(&self, chain: &str) -> bool {
        self.chains.iter().any(|c| c == chain)
    }

    fn chains(&self) -> Vec<String> {
        self.chains.clone()
    }
}

/// builder for constructing submit requests
pub struct SubmitBuilder {
    chain: String,
    signed_tx: Vec<u8>,
    wait: bool,
}

impl SubmitBuilder {
    /// start building for chain
    pub fn new(chain: impl Into<String>) -> Self {
        Self {
            chain: chain.into(),
            signed_tx: Vec::new(),
            wait: false,
        }
    }

    /// set signed transaction bytes
    pub fn signed_tx(mut self, tx: Vec<u8>) -> Self {
        self.signed_tx = tx;
        self
    }

    /// wait for confirmation
    pub fn wait(mut self) -> Self {
        self.wait = true;
        self
    }

    /// build the request
    pub fn build(self) -> SubmitRequest {
        SubmitRequest {
            chain: self.chain,
            signed_tx: self.signed_tx,
            wait: self.wait,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_mock_worker() {
        let worker = MockWorker::new(vec!["polkadot".into(), "penumbra".into()]);
        let handle = WorkerHandle::new(worker);

        assert!(handle.inner.supports("polkadot"));
        assert!(!handle.inner.supports("ethereum"));

        let height = handle.height("polkadot").unwrap();
        assert_eq!(height, 1000);
    }

    #[test]
    fn test_submit_builder() {
        let request = SubmitBuilder::new("zcash")
            .signed_tx(vec![1, 2, 3, 4])
            .wait()
            .build();

        assert_eq!(request.chain, "zcash");
        assert_eq!(request.signed_tx, vec![1, 2, 3, 4]);
        assert!(request.wait);
    }

    #[test]
    fn test_worker_submit() {
        let worker = MockWorker::new(vec!["osmosis".into()]);
        let handle = WorkerHandle::new(worker);

        let request = SubmitBuilder::new("osmosis")
            .signed_tx(vec![0xde, 0xad, 0xbe, 0xef])
            .build();

        let response = handle.submit(request);
        assert!(matches!(response, SubmitResponse::Accepted { .. }));
    }

    #[test]
    fn test_unsupported_chain() {
        let worker = MockWorker::new(vec!["polkadot".into()]);
        let handle = WorkerHandle::new(worker);

        let result = handle.height("ethereum");
        assert!(matches!(result, Err(WorkerError::UnsupportedChain(_))));
    }

    #[test]
    fn test_tx_status() {
        let worker = MockWorker::new(vec!["penumbra".into()]);
        let handle = WorkerHandle::new(worker);

        let status = handle.tx_status("penumbra", [0u8; 32]).unwrap();
        assert!(matches!(status, TxStatusResponse::Confirmed { .. }));
    }
}
