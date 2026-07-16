//! poker-chain-wasm: high-performance chain client for 240hz poker
//!
//! optimized for real-time games:
//! - frame budget: 4.16ms (240fps)
//! - optimistic state updates
//! - delta-based sync
//! - zero-copy where possible
//! - non-blocking async
//!
//! wire protocol (the server must implement this):
//!
//! server -> client:
//! - [0x01][scale(StateUpdate)]  channel state update (see StateUpdate;
//!   participants, app_data and the GameSnapshot drive the game ui —
//!   pot, phase, current_bet, acting_seat, action_deadline_ms)
//! - [0x02][tx_hash: 32]         tx confirmation
//! - [0x21][balance: 8 le]       balance query response
//!
//! client -> server:
//! - [0x10][channel_id: 32]      subscribe to channel updates
//! - [0x20][account: 32]         balance query
//! - [0x30][len: 4 le][tx]       submit tx (tx is an action, see src/action.rs)

use wasm_bindgen::prelude::*;
use parity_scale_codec::{Decode, Encode};
use std::collections::VecDeque;

mod action;
mod state;
mod websocket;
mod prediction;

pub use action::*;
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

        // apply optimistic update (single optimistic-apply path)
        self.predictor.apply_optimistic_tx(tx_bytes);

        // queue pending tx
        self.pending_txs.push_back(PendingTx {
            hash: *tx_hash.as_bytes(),
            submitted_at: now(),
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

        // prune timed-out pending txs (confirmed ones are removed on receipt)
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

                    // remove confirmed tx; unknown hashes are not a state change
                    if let Some(i) = self.pending_txs.iter().position(|tx| tx.hash == hash) {
                        self.pending_txs.remove(i);

                        // fire callback
                        if let Some(cb) = &self.on_tx_confirmed {
                            let _ = cb.call1(&JsValue::NULL, &JsValue::from(hex_encode(&hash)));
                        }
                        return true;
                    }
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
}

/// acting_seat value in GameSnapshot when no seat is to act
pub const NO_ACTING_SEAT: u8 = 0xff;

/// state update from chain, scale-encoded after the 0x01 opcode
#[derive(Clone, Debug, Encode, Decode)]
pub struct StateUpdate {
    pub channel_id: [u8; 32],
    pub nonce: u64,
    pub state_hash: [u8; 32],
    pub balances: Vec<(u32, u64)>, // (participant_idx, balance)
    pub app_data_hash: [u8; 32],
    /// participant public keys, indexed by seat
    pub participants: Vec<[u8; 32]>,
    /// opaque application state blob (must hash to app_data_hash)
    pub app_data: Vec<u8>,
    /// game view for the ui, none if the channel runs no game
    pub game: Option<GameSnapshot>,
}

/// per-update game view sent by the server
#[derive(Clone, Debug, Encode, Decode)]
pub struct GameSnapshot {
    pub phase: u8,
    pub pot: u64,
    pub current_bet: u64,
    /// seat currently to act, NO_ACTING_SEAT if none
    pub acting_seat: u8,
    /// remaining time to act in ms at send time, 0 if none
    pub action_deadline_ms: u64,
}

/// get current time in ms (worker-safe: falls back to Date.now when
/// there is no window, e.g. in a web worker)
#[cfg(target_arch = "wasm32")]
pub(crate) fn now() -> f64 {
    web_sys::window()
        .and_then(|w| w.performance())
        .map(|p| p.now())
        .unwrap_or_else(js_sys::Date::now)
}

/// get current time in ms (native, for tests)
#[cfg(not(target_arch = "wasm32"))]
pub(crate) fn now() -> f64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs_f64() * 1000.0)
        .unwrap_or(0.0)
}

/// hex encode
pub(crate) fn hex_encode(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{:02x}", b)).collect()
}

// ============================================================================
// high-level game api
// ============================================================================

/// poker game client - high-level api for game ui
///
/// all game state (pot, phase, turn, deadline) is derived from the
/// predictor: confirmed chain updates rebased with our optimistic
/// actions. there is exactly one apply path.
#[wasm_bindgen]
pub struct PokerGameClient {
    chain: PokerChainClient,
    /// our account
    account: [u8; 32],
    /// current channel
    channel_id: Option<[u8; 32]>,
    /// local game state (derived view, refreshed on poll/submit)
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

    /// submit poker action encoded with the canonical codec (src/action.rs)
    /// the predictor applies it optimistically exactly once
    #[wasm_bindgen]
    pub async fn submit_action(&mut self, action_bytes: &[u8]) -> Result<Vec<u8>, JsError> {
        let hash = self.chain.submit_tx(action_bytes).await?;
        self.sync_game_state();
        Ok(hash)
    }

    /// poll for updates - call every frame
    #[wasm_bindgen]
    pub fn poll(&mut self) -> bool {
        let changed = self.chain.poll();
        if changed {
            self.sync_game_state();
        }
        changed
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

    /// refresh the derived game view from predictor + cache
    fn sync_game_state(&mut self) {
        let Some(channel_id) = self.channel_id else { return };
        let Some(state) = &mut self.game_state else { return };

        let predicted = self.chain.predictor.predicted();
        state.pot = predicted.pot;
        state.phase = predicted.phase;
        state.current_bet = predicted.current_bet;

        let our_seat = self.chain.cache.participant_seat(&channel_id, &self.account);
        state.is_our_turn = match (predicted.acting_seat, our_seat) {
            (Some(acting), Some(ours)) => acting == ours,
            _ => false,
        };
        state.action_deadline = if state.is_our_turn {
            predicted.action_deadline
        } else {
            None
        };
    }
}

/// derived game view for the ui, updated from the predictor
pub struct LocalGameState {
    pub phase: u8,
    pub pot: u64,
    pub current_bet: u64,
    pub is_our_turn: bool,
    pub action_deadline: Option<f64>,
}

impl LocalGameState {
    pub fn new() -> Self {
        Self {
            phase: 0,
            pot: 0,
            current_bet: 0,
            is_our_turn: false,
            action_deadline: None,
        }
    }
}

impl Default for LocalGameState {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_client_creation() {
        let client = PokerChainClient::new();
        assert_eq!(client.connection_state(), ConnectionState::Disconnected);
    }

    #[test]
    fn test_frame_budget() {
        let mut client = PokerChainClient::new();
        assert!((client.frame_budget_ms - 4.16).abs() < 0.01);

        client.set_target_fps(60);
        assert!((client.frame_budget_ms - 16.67).abs() < 0.01);
    }

    #[test]
    fn test_tx_confirm_removes_pending() {
        let mut client = PokerChainClient::new();
        client.pending_txs.push_back(PendingTx {
            hash: [7u8; 32],
            submitted_at: 0.0,
        });

        // unknown hash: not a state change, pending untouched
        let mut msg = vec![0x02];
        msg.extend_from_slice(&[9u8; 32]);
        assert!(!client.process_message(&msg));
        assert_eq!(client.pending_txs.len(), 1);

        // matching hash: confirmed and removed
        let mut msg = vec![0x02];
        msg.extend_from_slice(&[7u8; 32]);
        assert!(client.process_message(&msg));
        assert!(client.pending_txs.is_empty());
    }

    #[test]
    fn test_state_update_drives_game_view() {
        let account = [3u8; 32];
        let mut game = PokerGameClient::new(&account).unwrap();
        game.channel_id = Some([1u8; 32]);
        game.game_state = Some(LocalGameState::new());

        let update = StateUpdate {
            channel_id: [1u8; 32],
            nonce: 1,
            state_hash: [0u8; 32],
            balances: vec![(0, 1000), (1, 900)],
            app_data_hash: [0u8; 32],
            participants: vec![[2u8; 32], account],
            app_data: vec![0xaa],
            game: Some(GameSnapshot {
                phase: 2,
                pot: 100,
                current_bet: 50,
                acting_seat: 1,
                action_deadline_ms: 30_000,
            }),
        };
        let mut msg = vec![0x01];
        msg.extend_from_slice(&update.encode());

        assert!(game.chain.process_message(&msg));
        game.sync_game_state();

        assert!(game.is_our_turn());
        assert_eq!(game.get_pot(), 100);
        assert_eq!(game.get_phase(), 2);
        assert!(game.action_timeout_ms() > 0.0);
        assert_eq!(game.get_balance(), 900);
        assert_eq!(
            game.chain.get_channel_cached(&[1u8; 32]),
            Some(vec![0xaa])
        );
    }
}
