//! poker-chain-wasm: high-performance chain client for 240hz poker
//!
//! optimized for real-time games:
//! - frame budget: 4.16ms (240fps)
//! - optimistic state updates
//! - delta-based sync
//! - zero-copy where possible
//! - non-blocking async

use wasm_bindgen::prelude::*;
use parity_scale_codec::{Decode, Encode};
use std::collections::VecDeque;

mod state;
mod websocket;
mod prediction;

pub use state::*;
pub use prediction::*;

/// initialize panic hook for better error messages
#[wasm_bindgen(start)]
pub fn init() {
    console_error_panic_hook::set_once();
}

/// connection state
#[wasm_bindgen]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ConnectionState {
    Disconnected,
    Connecting,
    Connected,
    Syncing,
    Ready,
    Error,
}

/// chain client for poker game
/// optimized for 240hz updates
#[wasm_bindgen]
pub struct PokerChainClient {
    /// connection state
    state: ConnectionState,
    /// websocket connection
    ws: Option<websocket::WsConnection>,
    /// local state cache
    cache: StateCache,
    /// pending transactions
    pending_txs: VecDeque<PendingTx>,
    /// optimistic state predictor
    predictor: StatePrediction,
    /// last update timestamp (performance.now())
    last_update_ms: f64,
    /// frame time budget (4.16ms for 240hz)
    frame_budget_ms: f64,
    /// callback for state updates
    on_state_update: Option<js_sys::Function>,
    /// callback for tx confirmations
    on_tx_confirmed: Option<js_sys::Function>,
}

#[wasm_bindgen]
impl PokerChainClient {
    /// create new client
    #[wasm_bindgen(constructor)]
    pub fn new() -> Self {
        Self {
            state: ConnectionState::Disconnected,
            ws: None,
            cache: StateCache::new(),
            pending_txs: VecDeque::new(),
            predictor: StatePrediction::new(),
            last_update_ms: 0.0,
            frame_budget_ms: 4.16, // 240hz
            on_state_update: None,
            on_tx_confirmed: None,
        }
    }

    /// set target fps (adjusts frame budget)
    #[wasm_bindgen]
    pub fn set_target_fps(&mut self, fps: u32) {
        self.frame_budget_ms = 1000.0 / fps as f64;
    }

    /// connect to chain endpoint
    #[wasm_bindgen]
    pub async fn connect(&mut self, endpoint: &str) -> Result<(), JsError> {
        self.state = ConnectionState::Connecting;

        let ws = websocket::WsConnection::connect(endpoint).await
            .map_err(|e| JsError::new(&format!("connect failed: {}", e)))?;

        self.ws = Some(ws);
        self.state = ConnectionState::Connected;

        Ok(())
    }

    /// disconnect
    #[wasm_bindgen]
    pub fn disconnect(&mut self) {
        if let Some(ws) = self.ws.take() {
            ws.close();
        }
        self.state = ConnectionState::Disconnected;
    }

    /// get connection state
    #[wasm_bindgen]
    pub fn connection_state(&self) -> ConnectionState {
        self.state
    }

    /// set callback for state updates
    #[wasm_bindgen]
    pub fn on_state_update(&mut self, callback: js_sys::Function) {
        self.on_state_update = Some(callback);
    }

    /// set callback for tx confirmations
    #[wasm_bindgen]
    pub fn on_tx_confirmed(&mut self, callback: js_sys::Function) {
        self.on_tx_confirmed = Some(callback);
    }

    /// subscribe to channel state updates
    #[wasm_bindgen]
    pub async fn subscribe_channel(&mut self, channel_id: &[u8]) -> Result<(), JsError> {
        if channel_id.len() != 32 {
            return Err(JsError::new("channel_id must be 32 bytes"));
        }

        let ws = self.ws.as_ref()
            .ok_or_else(|| JsError::new("not connected"))?;

        // subscribe via rpc
        let mut id = [0u8; 32];
        id.copy_from_slice(channel_id);
        ws.subscribe_channel(&id).await
            .map_err(|e| JsError::new(&format!("subscribe failed: {}", e)))?;

        Ok(())
    }

    /// get balance (cached, non-blocking)
    #[wasm_bindgen]
    pub fn get_balance_cached(&self, account: &[u8]) -> Option<u64> {
        if account.len() != 32 {
            return None;
        }
        let mut key = [0u8; 32];
        key.copy_from_slice(account);
        self.cache.get_balance(&key)
    }

    /// get balance (async, fetches from chain)
    #[wasm_bindgen]
    pub async fn get_balance(&mut self, account: &[u8]) -> Result<u64, JsError> {
        if account.len() != 32 {
            return Err(JsError::new("account must be 32 bytes"));
        }

        let ws = self.ws.as_ref()
            .ok_or_else(|| JsError::new("not connected"))?;

        let mut key = [0u8; 32];
        key.copy_from_slice(account);

        let balance = ws.query_balance(&key).await
            .map_err(|e| JsError::new(&format!("query failed: {}", e)))?;

        // update cache
        self.cache.set_balance(key, balance);

        Ok(balance)
    }

    /// get channel state (cached)
    #[wasm_bindgen]
    pub fn get_channel_cached(&self, channel_id: &[u8]) -> Option<Vec<u8>> {
        if channel_id.len() != 32 {
            return None;
        }
        let mut id = [0u8; 32];
        id.copy_from_slice(channel_id);
        self.cache.get_channel(&id)
    }

    /// submit signed transaction
    /// returns tx hash immediately (optimistic)
    #[wasm_bindgen]
    pub async fn submit_tx(&mut self, tx_bytes: &[u8]) -> Result<Vec<u8>, JsError> {
        let ws = self.ws.as_ref()
            .ok_or_else(|| JsError::new("not connected"))?;

        // compute tx hash
        let tx_hash = blake3::hash(tx_bytes);
        let hash_bytes = tx_hash.as_bytes().to_vec();

        // apply optimistic update
        self.predictor.apply_optimistic_tx(tx_bytes);

        // queue pending tx
        self.pending_txs.push_back(PendingTx {
            hash: *tx_hash.as_bytes(),
            submitted_at: now(),
            confirmed: false,
        });

        // submit async (non-blocking)
        ws.submit_tx(tx_bytes).await
            .map_err(|e| JsError::new(&format!("submit failed: {}", e)))?;

        Ok(hash_bytes)
    }

    /// poll for updates - call this every frame
    /// returns true if state changed
    #[wasm_bindgen]
    pub fn poll(&mut self) -> bool {
        let now = now();
        let elapsed = now - self.last_update_ms;

        // skip if within frame budget (avoid over-polling)
        if elapsed < self.frame_budget_ms * 0.5 {
            return false;
        }

        self.last_update_ms = now;

        let mut changed = false;

        // process websocket messages - collect first to avoid borrow conflict
        let messages: Vec<Vec<u8>> = self.ws.as_ref()
            .map(|ws| {
                let mut msgs = Vec::new();
                while let Some(msg) = ws.try_recv() {
                    msgs.push(msg);
                }
                msgs
            })
            .unwrap_or_default();

        for msg in messages {
            changed |= self.process_message(&msg);
        }

        // prune old pending txs
        self.pending_txs.retain(|tx| {
            now - tx.submitted_at < 60_000.0 // 60s timeout
        });

        changed
    }

    /// get predicted state (optimistic)
    /// use this for immediate UI updates
    #[wasm_bindgen]
    pub fn get_predicted_state(&self) -> JsValue {
        self.predictor.get_predicted_state()
    }

    /// get confirmed state (on-chain)
    #[wasm_bindgen]
    pub fn get_confirmed_state(&self) -> JsValue {
        self.cache.get_confirmed_state()
    }

    /// process incoming message
    fn process_message(&mut self, msg: &[u8]) -> bool {
        // decode message type
        if msg.is_empty() {
            return false;
        }

        match msg[0] {
            // state update
            0x01 => {
                if let Ok(update) = StateUpdate::decode(&mut &msg[1..]) {
                    self.cache.apply_update(&update);
                    self.predictor.confirm_state(&update);

                    // fire callback
                    if let Some(cb) = &self.on_state_update {
                        let _ = cb.call1(&JsValue::NULL, &JsValue::from(update.nonce));
                    }
                    return true;
                }
            }
            // tx confirmed
            0x02 => {
                if msg.len() >= 33 {
                    let mut hash = [0u8; 32];
                    hash.copy_from_slice(&msg[1..33]);

                    // mark tx confirmed
                    for tx in &mut self.pending_txs {
                        if tx.hash == hash {
                            tx.confirmed = true;

                            // fire callback
                            if let Some(cb) = &self.on_tx_confirmed {
                                let _ = cb.call1(&JsValue::NULL, &JsValue::from(hex_encode(&hash)));
                            }
                            break;
                        }
                    }
                    return true;
                }
            }
            _ => {}
        }

        false
    }
}

impl Default for PokerChainClient {
    fn default() -> Self {
        Self::new()
    }
}

/// pending transaction
struct PendingTx {
    hash: [u8; 32],
    submitted_at: f64,
    confirmed: bool,
}

/// state update from chain
#[derive(Clone, Debug, Encode, Decode)]
pub struct StateUpdate {
    pub channel_id: [u8; 32],
    pub nonce: u64,
    pub state_hash: [u8; 32],
    pub balances: Vec<(u32, u64)>, // (participant_idx, balance)
    pub app_data_hash: [u8; 32],
}

/// get current time in ms
fn now() -> f64 {
    web_sys::window()
        .and_then(|w| w.performance())
        .map(|p| p.now())
        .unwrap_or(0.0)
}

/// hex encode
fn hex_encode(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{:02x}", b)).collect()
}

// ============================================================================
// high-level game api
// ============================================================================

/// poker game client - high-level api for game ui
#[wasm_bindgen]
pub struct PokerGameClient {
    chain: PokerChainClient,
    /// our account
    account: [u8; 32],
    /// current channel
    channel_id: Option<[u8; 32]>,
    /// local game state
    game_state: Option<LocalGameState>,
}

#[wasm_bindgen]
impl PokerGameClient {
    /// create new game client
    #[wasm_bindgen(constructor)]
    pub fn new(account: &[u8]) -> Result<PokerGameClient, JsError> {
        if account.len() != 32 {
            return Err(JsError::new("account must be 32 bytes"));
        }
        let mut acc = [0u8; 32];
        acc.copy_from_slice(account);

        Ok(Self {
            chain: PokerChainClient::new(),
            account: acc,
            channel_id: None,
            game_state: None,
        })
    }

    /// connect to chain
    #[wasm_bindgen]
    pub async fn connect(&mut self, endpoint: &str) -> Result<(), JsError> {
        self.chain.connect(endpoint).await
    }

    /// join a poker table
    #[wasm_bindgen]
    pub async fn join_table(&mut self, channel_id: &[u8]) -> Result<(), JsError> {
        if channel_id.len() != 32 {
            return Err(JsError::new("channel_id must be 32 bytes"));
        }

        let mut id = [0u8; 32];
        id.copy_from_slice(channel_id);

        self.chain.subscribe_channel(&id).await?;
        self.channel_id = Some(id);
        self.game_state = Some(LocalGameState::new());

        Ok(())
    }

    /// submit poker action (bet, fold, etc)
    #[wasm_bindgen]
    pub async fn submit_action(&mut self, action_bytes: &[u8]) -> Result<Vec<u8>, JsError> {
        // apply local prediction immediately
        if let Some(state) = &mut self.game_state {
            state.apply_action_local(action_bytes);
        }

        // submit to chain
        self.chain.submit_tx(action_bytes).await
    }

    /// poll for updates - call every frame
    #[wasm_bindgen]
    pub fn poll(&mut self) -> bool {
        self.chain.poll()
    }

    /// get our balance
    #[wasm_bindgen]
    pub fn get_balance(&self) -> u64 {
        self.chain.get_balance_cached(&self.account).unwrap_or(0)
    }

    /// get current pot (predicted)
    #[wasm_bindgen]
    pub fn get_pot(&self) -> u64 {
        self.game_state.as_ref().map(|s| s.pot).unwrap_or(0)
    }

    /// get current phase
    #[wasm_bindgen]
    pub fn get_phase(&self) -> u8 {
        self.game_state.as_ref().map(|s| s.phase).unwrap_or(0)
    }

    /// is it our turn?
    #[wasm_bindgen]
    pub fn is_our_turn(&self) -> bool {
        self.game_state.as_ref().map(|s| s.is_our_turn).unwrap_or(false)
    }

    /// get time remaining for action (ms)
    #[wasm_bindgen]
    pub fn action_timeout_ms(&self) -> f64 {
        self.game_state.as_ref()
            .and_then(|s| s.action_deadline)
            .map(|deadline| (deadline - now()).max(0.0))
            .unwrap_or(0.0)
    }
}

/// local game state for prediction
pub struct LocalGameState {
    pub phase: u8,
    pub pot: u64,
    pub current_bet: u64,
    pub our_bet: u64,
    pub is_our_turn: bool,
    pub action_deadline: Option<f64>,
    pub players: Vec<PlayerState>,
}

impl LocalGameState {
    pub fn new() -> Self {
        Self {
            phase: 0,
            pot: 0,
            current_bet: 0,
            our_bet: 0,
            is_our_turn: false,
            action_deadline: None,
            players: Vec::new(),
        }
    }

    pub fn apply_action_local(&mut self, action: &[u8]) {
        // optimistically apply action
        // this will be confirmed/reverted when chain update arrives
        if action.is_empty() {
            return;
        }

        match action[0] {
            // fold
            0x00 => {
                self.is_our_turn = false;
            }
            // check
            0x01 => {
                self.is_our_turn = false;
            }
            // call
            0x02 => {
                let call_amount = self.current_bet.saturating_sub(self.our_bet);
                self.pot += call_amount;
                self.our_bet = self.current_bet;
                self.is_our_turn = false;
            }
            // raise (amount in bytes 1..9)
            0x03 if action.len() >= 9 => {
                let amount = u64::from_le_bytes(action[1..9].try_into().unwrap_or([0; 8]));
                self.current_bet += amount;
                let raise_cost = self.current_bet.saturating_sub(self.our_bet);
                self.pot += raise_cost;
                self.our_bet = self.current_bet;
                self.is_our_turn = false;
            }
            _ => {}
        }
    }
}

impl Default for LocalGameState {
    fn default() -> Self {
        Self::new()
    }
}

/// player state
pub struct PlayerState {
    pub seat: u8,
    pub balance: u64,
    pub current_bet: u64,
    pub folded: bool,
    pub all_in: bool,
}

#[cfg(test)]
mod tests {
    use super::*;
    use wasm_bindgen_test::*;

    #[wasm_bindgen_test]
    fn test_client_creation() {
        let client = PokerChainClient::new();
        assert_eq!(client.connection_state(), ConnectionState::Disconnected);
    }

    #[wasm_bindgen_test]
    fn test_frame_budget() {
        let mut client = PokerChainClient::new();
        assert!((client.frame_budget_ms - 4.16).abs() < 0.01);

        client.set_target_fps(60);
        assert!((client.frame_budget_ms - 16.67).abs() < 0.01);
    }
}
