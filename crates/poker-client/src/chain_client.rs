//! chain client resource for bevy
//!
//! wraps the chain module and provides async connection management

use bevy::prelude::*;
use std::sync::{Arc, Mutex};

use crate::chain::{ChainState, ChainClient, MockChainClient, AccountBalance, ChainError};

pub struct ChainClientPlugin;

impl Plugin for ChainClientPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<ChainConnection>()
            .add_systems(Update, (poll_chain_connection, poll_balance_queries));
    }
}

/// wrapper for mpsc receiver to make it Sync
struct SyncReceiver<T>(Mutex<Option<std::sync::mpsc::Receiver<T>>>);

impl<T> SyncReceiver<T> {
    fn new(rx: std::sync::mpsc::Receiver<T>) -> Self {
        Self(Mutex::new(Some(rx)))
    }

    fn try_recv(&self) -> Option<T> {
        let guard = self.0.lock().ok()?;
        guard.as_ref()?.try_recv().ok()
    }

    fn take(&self) -> Option<std::sync::mpsc::Receiver<T>> {
        self.0.lock().ok()?.take()
    }

    fn is_some(&self) -> bool {
        self.0.lock().map(|g| g.is_some()).unwrap_or(false)
    }
}

/// chain connection state
#[derive(Resource)]
pub struct ChainConnection {
    /// connection state
    pub state: ChainState,
    /// our account pubkey (from auth)
    pub account: Option<[u8; 32]>,
    /// cached balance
    pub balance: AccountBalance,
    /// balance last updated block
    pub balance_block: u32,
    /// pending connection
    pending_connect: Option<Arc<SyncReceiver<Result<(), ChainError>>>>,
    /// pending balance query
    pending_balance: Option<Arc<SyncReceiver<Result<AccountBalance, ChainError>>>>,
    /// internal client (mock for now, real impl uses subxt/smoldot)
    client: Arc<Mutex<MockChainClient>>,
}

impl Default for ChainConnection {
    fn default() -> Self {
        Self {
            state: ChainState::Disconnected,
            account: None,
            balance: AccountBalance::default(),
            balance_block: 0,
            pending_connect: None,
            pending_balance: None,
            client: Arc::new(Mutex::new(MockChainClient::new())),
        }
    }
}

impl ChainConnection {
    /// start connecting to the chain
    pub fn connect(&mut self, endpoint: &str, account: [u8; 32]) {
        if !matches!(self.state, ChainState::Disconnected | ChainState::Error(_)) {
            return;
        }

        self.state = ChainState::Connecting;
        self.account = Some(account);

        let (tx, rx) = std::sync::mpsc::channel();
        self.pending_connect = Some(Arc::new(SyncReceiver::new(rx)));

        let client = self.client.clone();
        let endpoint = endpoint.to_string();

        std::thread::spawn(move || {
            // use tokio runtime for async chain client
            let rt = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .unwrap();

            let result = rt.block_on(async {
                let mut c = client.lock().unwrap();
                c.connect(&endpoint).await
            });

            let _ = tx.send(result);
        });
    }

    /// disconnect from chain
    pub fn disconnect(&mut self) {
        self.state = ChainState::Disconnected;
        self.account = None;
        self.balance = AccountBalance::default();
        self.pending_connect = None;
        self.pending_balance = None;
    }

    /// request balance refresh
    pub fn refresh_balance(&mut self) {
        let account = match self.account {
            Some(a) => a,
            None => return,
        };

        if !matches!(self.state, ChainState::Connected { .. }) {
            return;
        }

        if self.pending_balance.as_ref().map(|r| r.is_some()).unwrap_or(false) {
            return; // already pending
        }

        let (tx, rx) = std::sync::mpsc::channel();
        self.pending_balance = Some(Arc::new(SyncReceiver::new(rx)));

        let client = self.client.clone();

        std::thread::spawn(move || {
            let rt = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .unwrap();

            let result = rt.block_on(async {
                let c = client.lock().unwrap();
                c.get_balance(&account).await
            });

            let _ = tx.send(result);
        });
    }

    /// get transferable balance in display units (divide by 10^12)
    pub fn transferable_balance(&self) -> u64 {
        (self.balance.transferable() / 1_000_000) as u64 // convert to chip units
    }

    /// is connected?
    pub fn is_connected(&self) -> bool {
        matches!(self.state, ChainState::Connected { .. })
    }

    /// add balance (for mock faucet)
    pub fn add_mock_balance(&mut self, amount: u128) {
        self.balance.free = self.balance.free.saturating_add(amount);
    }
}

/// poll for connection result
fn poll_chain_connection(
    mut chain: ResMut<ChainConnection>,
) {
    let result = chain.pending_connect.as_ref().and_then(|rx| rx.try_recv());

    if let Some(result) = result {
        chain.pending_connect = None;
        match result {
            Ok(()) => {
                chain.state = ChainState::Connected { finalized: 0 };
                info!("chain: connected to ghettobox network");

                // set initial mock balance (1000 chips = 1_000_000_000_000_000 base units)
                let initial_balance: u128 = 1_000_000_000_000_000;
                if let Some(account) = chain.account {
                    let mut client = chain.client.lock().unwrap();
                    *client = MockChainClient::new().with_balance(account, initial_balance);
                }

                // also set the cached balance immediately
                chain.balance = AccountBalance {
                    free: initial_balance,
                    reserved: 0,
                    frozen: 0,
                };
                info!("chain: initial balance {} chips", chain.transferable_balance());
            }
            Err(e) => {
                chain.state = ChainState::Error(e.to_string());
                warn!("chain: connection failed - {}", e);
            }
        }
    }
}

/// poll for balance query result
fn poll_balance_queries(
    mut chain: ResMut<ChainConnection>,
) {
    let result = chain.pending_balance.as_ref().and_then(|rx| rx.try_recv());

    if let Some(result) = result {
        chain.pending_balance = None;
        match result {
            Ok(balance) => {
                chain.balance = balance;
                if let ChainState::Connected { finalized } = chain.state {
                    chain.balance_block = finalized;
                }
                info!("chain: balance updated - {} chips", chain.transferable_balance());
            }
            Err(e) => {
                warn!("chain: balance query failed - {}", e);
            }
        }
    }
}

/// event to trigger chain connection after login
#[derive(Event)]
pub struct ChainConnectEvent {
    pub account: [u8; 32],
}
