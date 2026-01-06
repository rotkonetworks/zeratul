//! optimistic state prediction for 240hz ui
//!
//! provides immediate state updates before chain confirmation
//! - apply txs optimistically
//! - revert on conflict
//! - interpolate between states

use wasm_bindgen::prelude::*;
use std::collections::VecDeque;

use crate::StateUpdate;

/// maximum pending predictions
const MAX_PENDING: usize = 16;

/// optimistic state prediction
pub struct StatePrediction {
    /// base confirmed state
    confirmed: Option<PredictedState>,
    /// pending optimistic updates
    pending: VecDeque<PendingUpdate>,
    /// current predicted state (confirmed + pending)
    predicted: PredictedState,
    /// prediction accuracy stats
    hits: u64,
    misses: u64,
}

impl StatePrediction {
    pub fn new() -> Self {
        Self {
            confirmed: None,
            pending: VecDeque::with_capacity(MAX_PENDING),
            predicted: PredictedState::default(),
            hits: 0,
            misses: 0,
        }
    }

    /// apply optimistic transaction
    pub fn apply_optimistic_tx(&mut self, tx_bytes: &[u8]) {
        let tx_hash = blake3::hash(tx_bytes);

        // parse and apply optimistically
        if let Some(update) = self.parse_tx(tx_bytes) {
            // store pending
            self.pending.push_back(PendingUpdate {
                tx_hash: *tx_hash.as_bytes(),
                applied_at: crate::now(),
                delta: update.clone(),
            });

            // apply to predicted state
            self.apply_delta(&update);

            // prune old pending
            while self.pending.len() > MAX_PENDING {
                self.pending.pop_front();
            }
        }
    }

    /// confirm state from chain
    pub fn confirm_state(&mut self, update: &StateUpdate) {
        let new_confirmed = PredictedState {
            nonce: update.nonce,
            channel_id: update.channel_id,
            balances: update.balances.iter()
                .map(|(i, b)| (*i as u8, *b))
                .collect(),
            pot: 0, // would be in app_data
            phase: 0,
            current_bet: 0,
        };

        // check predictions
        let old_pending_len = self.pending.len();

        // remove confirmed pending txs
        self.pending.retain(|p| {
            // keep if nonce is still ahead
            p.delta.nonce > update.nonce
        });

        if self.pending.len() < old_pending_len {
            self.hits += (old_pending_len - self.pending.len()) as u64;
        }

        // update confirmed
        self.confirmed = Some(new_confirmed.clone());

        // rebuild predicted from confirmed + remaining pending
        self.predicted = new_confirmed;

        // collect deltas first to avoid borrow conflict
        let deltas: Vec<_> = self.pending.iter().map(|p| p.delta.clone()).collect();
        for delta in &deltas {
            self.apply_delta(delta);
        }
    }

    /// get predicted state as js value
    pub fn get_predicted_state(&self) -> JsValue {
        #[derive(serde::Serialize)]
        struct State {
            nonce: u64,
            channel_id: String,
            balances: Vec<(u8, u64)>,
            pot: u64,
            phase: u8,
            current_bet: u64,
            pending_count: usize,
            prediction_accuracy: f64,
        }

        let accuracy = if self.hits + self.misses > 0 {
            self.hits as f64 / (self.hits + self.misses) as f64
        } else {
            1.0
        };

        let state = State {
            nonce: self.predicted.nonce,
            channel_id: hex_encode(&self.predicted.channel_id),
            balances: self.predicted.balances.clone(),
            pot: self.predicted.pot,
            phase: self.predicted.phase,
            current_bet: self.predicted.current_bet,
            pending_count: self.pending.len(),
            prediction_accuracy: accuracy,
        };

        serde_wasm_bindgen::to_value(&state).unwrap_or(JsValue::NULL)
    }

    /// parse tx into state delta
    fn parse_tx(&self, tx_bytes: &[u8]) -> Option<StateDelta> {
        if tx_bytes.is_empty() {
            return None;
        }

        // simple tx format for poker actions
        // [action_type: 1][channel_id: 32][nonce: 8][data...]
        if tx_bytes.len() < 41 {
            return None;
        }

        let action_type = tx_bytes[0];
        let mut channel_id = [0u8; 32];
        channel_id.copy_from_slice(&tx_bytes[1..33]);
        let nonce = u64::from_le_bytes(tx_bytes[33..41].try_into().ok()?);

        match action_type {
            // bet action
            0x01 => {
                if tx_bytes.len() >= 50 {
                    let seat = tx_bytes[41];
                    let amount = u64::from_le_bytes(tx_bytes[42..50].try_into().ok()?);
                    return Some(StateDelta {
                        channel_id,
                        nonce,
                        balance_changes: vec![(seat, -(amount as i64))],
                        pot_change: amount as i64,
                        phase_change: None,
                        current_bet_change: None,
                    });
                }
            }
            // fold action
            0x02 => {
                return Some(StateDelta {
                    channel_id,
                    nonce,
                    balance_changes: Vec::new(),
                    pot_change: 0,
                    phase_change: None,
                    current_bet_change: None,
                });
            }
            // call action
            0x03 => {
                if tx_bytes.len() >= 50 {
                    let seat = tx_bytes[41];
                    let amount = u64::from_le_bytes(tx_bytes[42..50].try_into().ok()?);
                    return Some(StateDelta {
                        channel_id,
                        nonce,
                        balance_changes: vec![(seat, -(amount as i64))],
                        pot_change: amount as i64,
                        phase_change: None,
                        current_bet_change: None,
                    });
                }
            }
            // raise action
            0x04 => {
                if tx_bytes.len() >= 58 {
                    let seat = tx_bytes[41];
                    let amount = u64::from_le_bytes(tx_bytes[42..50].try_into().ok()?);
                    let new_bet = u64::from_le_bytes(tx_bytes[50..58].try_into().ok()?);
                    return Some(StateDelta {
                        channel_id,
                        nonce,
                        balance_changes: vec![(seat, -(amount as i64))],
                        pot_change: amount as i64,
                        phase_change: None,
                        current_bet_change: Some(new_bet),
                    });
                }
            }
            _ => {}
        }

        None
    }

    /// apply delta to predicted state
    fn apply_delta(&mut self, delta: &StateDelta) {
        self.predicted.nonce = delta.nonce;
        self.predicted.channel_id = delta.channel_id;

        // apply balance changes
        for (seat, change) in &delta.balance_changes {
            if let Some((_, balance)) = self.predicted.balances.iter_mut()
                .find(|(s, _)| *s == *seat)
            {
                if *change < 0 {
                    *balance = balance.saturating_sub((-*change) as u64);
                } else {
                    *balance = balance.saturating_add(*change as u64);
                }
            }
        }

        // apply pot change
        if delta.pot_change < 0 {
            self.predicted.pot = self.predicted.pot.saturating_sub((-delta.pot_change) as u64);
        } else {
            self.predicted.pot = self.predicted.pot.saturating_add(delta.pot_change as u64);
        }

        // apply phase change
        if let Some(phase) = delta.phase_change {
            self.predicted.phase = phase;
        }

        // apply current bet change
        if let Some(bet) = delta.current_bet_change {
            self.predicted.current_bet = bet;
        }
    }
}

impl Default for StatePrediction {
    fn default() -> Self {
        Self::new()
    }
}

/// predicted game state
#[derive(Clone, Default)]
pub struct PredictedState {
    pub nonce: u64,
    pub channel_id: [u8; 32],
    pub balances: Vec<(u8, u64)>, // (seat, balance)
    pub pot: u64,
    pub phase: u8,
    pub current_bet: u64,
}

/// pending optimistic update
struct PendingUpdate {
    tx_hash: [u8; 32],
    applied_at: f64,
    delta: StateDelta,
}

/// state change delta
#[derive(Clone)]
struct StateDelta {
    channel_id: [u8; 32],
    nonce: u64,
    balance_changes: Vec<(u8, i64)>, // (seat, delta)
    pot_change: i64,
    phase_change: Option<u8>,
    current_bet_change: Option<u64>,
}

fn hex_encode(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{:02x}", b)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_prediction_new() {
        let pred = StatePrediction::new();
        assert!(pred.confirmed.is_none());
        assert!(pred.pending.is_empty());
    }
}
