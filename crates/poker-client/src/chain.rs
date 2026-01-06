//! chain interaction module
//!
//! provides light client connection to ghettobox chain for:
//! - account registration
//! - channel operations (open, close, dispute)
//! - table creation/joining
//! - balance queries

#![allow(dead_code)]

use parity_scale_codec::{Decode, Encode};

/// chain connection state
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ChainState {
    Disconnected,
    Connecting,
    Syncing { finalized: u32, target: u32 },
    Connected { finalized: u32 },
    Error(String),
}

impl Default for ChainState {
    fn default() -> Self {
        Self::Disconnected
    }
}

/// account balance
#[derive(Clone, Debug, Default, Encode, Decode)]
pub struct AccountBalance {
    pub free: u128,
    pub reserved: u128,
    pub frozen: u128,
}

impl AccountBalance {
    pub fn transferable(&self) -> u128 {
        self.free.saturating_sub(self.frozen)
    }
}

/// channel info from chain
#[derive(Clone, Debug, Encode, Decode)]
pub struct ChannelInfo {
    pub id: [u8; 32],
    pub participants: Vec<[u8; 32]>,
    pub total_deposit: u128,
    pub state: ChannelState,
    pub nonce: u64,
    pub timeout_block: Option<u32>,
}

#[derive(Clone, Debug, PartialEq, Eq, Encode, Decode)]
pub enum ChannelState {
    Open,
    Closing,
    Disputed,
    Settled,
}

/// table info from chain
#[derive(Clone, Debug, Encode, Decode)]
pub struct TableInfo {
    pub id: u64,
    pub host: [u8; 32],
    pub stakes: Stakes,
    pub max_players: u8,
    pub current_players: u8,
    pub security_tier: u8,
}

#[derive(Clone, Debug, Encode, Decode)]
pub struct Stakes {
    pub small_blind: u128,
    pub big_blind: u128,
    pub min_buy_in: u128,
    pub max_buy_in: u128,
}

/// transaction status
#[derive(Clone, Debug)]
pub enum TxStatus {
    Pending,
    InBlock { block_hash: [u8; 32] },
    Finalized { block_hash: [u8; 32] },
    Failed(String),
}

/// chain client trait - abstraction over subxt/smoldot
pub trait ChainClient: Send + Sync {
    /// connect to chain
    fn connect(&mut self, endpoint: &str) -> impl std::future::Future<Output = Result<(), ChainError>> + Send;

    /// get current state
    fn state(&self) -> ChainState;

    /// get account balance
    fn get_balance(&self, account: &[u8; 32]) -> impl std::future::Future<Output = Result<AccountBalance, ChainError>> + Send;

    /// get channel info
    fn get_channel(&self, id: &[u8; 32]) -> impl std::future::Future<Output = Result<Option<ChannelInfo>, ChainError>> + Send;

    /// get table info
    fn get_table(&self, id: u64) -> impl std::future::Future<Output = Result<Option<TableInfo>, ChainError>> + Send;

    /// submit signed transaction
    fn submit_tx(&self, tx_bytes: &[u8]) -> impl std::future::Future<Output = Result<[u8; 32], ChainError>> + Send;

    /// watch transaction status
    fn watch_tx(&self, tx_hash: &[u8; 32]) -> impl std::future::Future<Output = Result<TxStatus, ChainError>> + Send;
}

/// chain client errors
#[derive(Clone, Debug)]
pub enum ChainError {
    ConnectionFailed(String),
    NotConnected,
    QueryFailed(String),
    SubmitFailed(String),
    DecodingError(String),
    Timeout,
}

impl std::fmt::Display for ChainError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::ConnectionFailed(e) => write!(f, "connection failed: {}", e),
            Self::NotConnected => write!(f, "not connected"),
            Self::QueryFailed(e) => write!(f, "query failed: {}", e),
            Self::SubmitFailed(e) => write!(f, "submit failed: {}", e),
            Self::DecodingError(e) => write!(f, "decoding error: {}", e),
            Self::Timeout => write!(f, "timeout"),
        }
    }
}

impl std::error::Error for ChainError {}

/// mock chain client for testing/development
#[derive(Default)]
pub struct MockChainClient {
    state: ChainState,
    balances: std::collections::HashMap<[u8; 32], AccountBalance>,
}

impl MockChainClient {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_balance(mut self, account: [u8; 32], balance: u128) -> Self {
        self.balances.insert(account, AccountBalance {
            free: balance,
            reserved: 0,
            frozen: 0,
        });
        self
    }
}

impl ChainClient for MockChainClient {
    async fn connect(&mut self, _endpoint: &str) -> Result<(), ChainError> {
        self.state = ChainState::Connected { finalized: 1000 };
        Ok(())
    }

    fn state(&self) -> ChainState {
        self.state.clone()
    }

    async fn get_balance(&self, account: &[u8; 32]) -> Result<AccountBalance, ChainError> {
        Ok(self.balances.get(account).cloned().unwrap_or_default())
    }

    async fn get_channel(&self, _id: &[u8; 32]) -> Result<Option<ChannelInfo>, ChainError> {
        Ok(None)
    }

    async fn get_table(&self, _id: u64) -> Result<Option<TableInfo>, ChainError> {
        Ok(None)
    }

    async fn submit_tx(&self, _tx_bytes: &[u8]) -> Result<[u8; 32], ChainError> {
        Ok([0u8; 32])
    }

    async fn watch_tx(&self, _tx_hash: &[u8; 32]) -> Result<TxStatus, ChainError> {
        Ok(TxStatus::Finalized { block_hash: [0u8; 32] })
    }
}

// ============================================================
// Transaction Builders
// ============================================================

/// poker pool calls
#[derive(Clone, Debug, Encode)]
pub enum PokerPoolCall {
    /// register managed account
    RegisterManaged {
        tier: u8,
        encrypted_shards: Vec<Vec<u8>>,
        registration_proof: Vec<u8>,
    },
    /// register self-custody account
    RegisterSelfCustody {
        pubkey: [u8; 32],
        signature: [u8; 64],
    },
}

/// state channel calls
#[derive(Clone, Debug, Encode)]
pub enum StateChannelCall {
    /// open a new channel
    Open {
        participants: Vec<[u8; 32]>,
        initial_state: Vec<u8>,
    },
    /// update channel state (signed by all)
    Update {
        channel_id: [u8; 32],
        nonce: u64,
        state_hash: [u8; 32],
        signatures: Vec<[u8; 64]>,
    },
    /// close channel cooperatively
    Close {
        channel_id: [u8; 32],
        final_state: Vec<u8>,
        signatures: Vec<[u8; 64]>,
    },
    /// dispute (unilateral close)
    Dispute {
        channel_id: [u8; 32],
        state: Vec<u8>,
        signatures: Vec<[u8; 64]>,
    },
}

/// build unsigned transaction
pub fn build_poker_pool_tx(call: PokerPoolCall, nonce: u32) -> Vec<u8> {
    let mut tx = Vec::new();
    // pallet index for poker-pool (example: 50)
    tx.push(50);
    // call index
    match &call {
        PokerPoolCall::RegisterManaged { .. } => tx.push(0),
        PokerPoolCall::RegisterSelfCustody { .. } => tx.push(1),
    }
    // encode call data
    call.encode_to(&mut tx);
    // add nonce (simplified - real tx needs more fields)
    tx.extend_from_slice(&nonce.to_le_bytes());
    tx
}

pub fn build_state_channel_tx(call: StateChannelCall, nonce: u32) -> Vec<u8> {
    let mut tx = Vec::new();
    // pallet index for state-channel (example: 51)
    tx.push(51);
    // call index
    match &call {
        StateChannelCall::Open { .. } => tx.push(0),
        StateChannelCall::Update { .. } => tx.push(1),
        StateChannelCall::Close { .. } => tx.push(2),
        StateChannelCall::Dispute { .. } => tx.push(3),
    }
    // encode call data
    call.encode_to(&mut tx);
    tx.extend_from_slice(&nonce.to_le_bytes());
    tx
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_mock_client() {
        let account = [1u8; 32];
        let mut client = MockChainClient::new()
            .with_balance(account, 1_000_000_000_000);

        client.connect("ws://localhost:9944").await.unwrap();
        assert!(matches!(client.state(), ChainState::Connected { .. }));

        let balance = client.get_balance(&account).await.unwrap();
        assert_eq!(balance.free, 1_000_000_000_000);
    }

    #[test]
    fn test_tx_building() {
        let call = PokerPoolCall::RegisterSelfCustody {
            pubkey: [0u8; 32],
            signature: [0u8; 64],
        };
        let tx = build_poker_pool_tx(call, 0);
        assert!(!tx.is_empty());
        assert_eq!(tx[0], 50); // pallet index
        assert_eq!(tx[1], 1);  // call index
    }
}
