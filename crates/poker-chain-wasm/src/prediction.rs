//! optimistic state prediction for 240hz ui
//!
//! single optimistic-apply path: txs are decoded via the canonical
//! action codec (src/action.rs), applied as deltas on top of the last
//! confirmed chain state, and rebased when confirmations arrive.

use wasm_bindgen::prelude::*;
use std::collections::VecDeque;

use crate::action::{Action, ActionKind};
use crate::{StateUpdate, NO_ACTING_SEAT};

/// maximum pending predictions
const MAX_PENDING: usize = 16;

/// optimistic state prediction
pub struct StatePrediction {
    /// base confirmed state
    confirmed: Option<PredictedState>,
    /// pending optimistic deltas, oldest first
    pending: VecDeque<StateDelta>,
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

    /// current predicted state
    pub fn predicted(&self) -> &PredictedState {
        &self.predicted
    }

    /// apply optimistic transaction
    pub fn apply_optimistic_tx(&mut self, tx_bytes: &[u8]) {
        if let Some(delta) = Self::parse_tx(tx_bytes) {
            self.apply_delta(&delta);
            self.pending.push_back(delta);

            // prune old pending
            while self.pending.len() > MAX_PENDING {
                self.pending.pop_front();
            }
        }
    }

    /// confirm state from chain, rebasing remaining pending deltas
    pub fn confirm_state(&mut self, update: &StateUpdate) {
        let mut new_confirmed = PredictedState {
            nonce: update.nonce,
            channel_id: update.channel_id,
            balances: update.balances.iter().map(|(i, b)| (*i as u8, *b)).collect(),
            pot: 0,
            phase: 0,
            current_bet: 0,
            acting_seat: None,
            action_deadline: None,
        };
        if let Some(game) = &update.game {
            new_confirmed.pot = game.pot;
            new_confirmed.phase = game.phase;
            new_confirmed.current_bet = game.current_bet;
            if game.acting_seat != NO_ACTING_SEAT {
                new_confirmed.acting_seat = Some(game.acting_seat);
                if game.action_deadline_ms > 0 {
                    new_confirmed.action_deadline =
                        Some(crate::now() + game.action_deadline_ms as f64);
                }
            }
        }

        // split pending into consumed (covered by this update) and remaining
        let mut consumed = Vec::new();
        let mut remaining = VecDeque::with_capacity(self.pending.len());
        for delta in self.pending.drain(..) {
            if delta.nonce > update.nonce {
                remaining.push_back(delta);
            } else {
                consumed.push(delta);
            }
        }
        self.pending = remaining;

        // score consumed predictions: replay them on the previous confirmed
        // base and compare against the confirmed game state; without a
        // baseline or game snapshot there is nothing to contradict, count hit
        if !consumed.is_empty() {
            match (&self.confirmed, &update.game) {
                (Some(base), Some(game)) => {
                    let mut sim = base.clone();
                    for delta in &consumed {
                        Self::apply_delta_to(&mut sim, delta);
                    }
                    if sim.pot == game.pot && sim.current_bet == game.current_bet {
                        self.hits += consumed.len() as u64;
                    } else {
                        self.misses += consumed.len() as u64;
                    }
                }
                _ => self.hits += consumed.len() as u64,
            }
        }

        // rebase: confirmed + remaining pending
        self.confirmed = Some(new_confirmed.clone());
        self.predicted = new_confirmed;
        let deltas: Vec<_> = self.pending.iter().cloned().collect();
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
            acting_seat: Option<u8>,
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
            channel_id: crate::hex_encode(&self.predicted.channel_id),
            balances: self.predicted.balances.clone(),
            pot: self.predicted.pot,
            phase: self.predicted.phase,
            current_bet: self.predicted.current_bet,
            acting_seat: self.predicted.acting_seat,
            pending_count: self.pending.len(),
            prediction_accuracy: accuracy,
        };

        serde_wasm_bindgen::to_value(&state).unwrap_or(JsValue::NULL)
    }

    /// parse tx into state delta via the canonical action codec
    fn parse_tx(tx_bytes: &[u8]) -> Option<StateDelta> {
        let action = Action::decode(tx_bytes)?;
        let (balance_changes, pot_change, current_bet_change) = match action.kind {
            ActionKind::Bet { amount } | ActionKind::Call { amount } => {
                (vec![(action.seat, -(amount as i64))], amount as i64, None)
            }
            ActionKind::Raise { amount, new_bet } => (
                vec![(action.seat, -(amount as i64))],
                amount as i64,
                Some(new_bet),
            ),
            ActionKind::Fold | ActionKind::Check => (Vec::new(), 0, None),
        };
        Some(StateDelta {
            channel_id: action.channel_id,
            nonce: action.nonce,
            balance_changes,
            pot_change,
            phase_change: None,
            current_bet_change,
            // after our action it is no longer our turn until the chain says so
            clears_turn: true,
        })
    }

    /// apply delta to predicted state
    fn apply_delta(&mut self, delta: &StateDelta) {
        Self::apply_delta_to(&mut self.predicted, delta);
    }

    fn apply_delta_to(state: &mut PredictedState, delta: &StateDelta) {
        state.nonce = delta.nonce;
        state.channel_id = delta.channel_id;

        // apply balance changes
        for (seat, change) in &delta.balance_changes {
            if let Some((_, balance)) = state.balances.iter_mut().find(|(s, _)| *s == *seat) {
                if *change < 0 {
                    *balance = balance.saturating_sub((-*change) as u64);
                } else {
                    *balance = balance.saturating_add(*change as u64);
                }
            }
        }

        // apply pot change
        if delta.pot_change < 0 {
            state.pot = state.pot.saturating_sub((-delta.pot_change) as u64);
        } else {
            state.pot = state.pot.saturating_add(delta.pot_change as u64);
        }

        // apply phase change
        if let Some(phase) = delta.phase_change {
            state.phase = phase;
        }

        // apply current bet change
        if let Some(bet) = delta.current_bet_change {
            state.current_bet = bet;
        }

        if delta.clears_turn {
            state.acting_seat = None;
            state.action_deadline = None;
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
    /// seat currently to act (confirmed info)
    pub acting_seat: Option<u8>,
    /// local absolute deadline (ms, crate::now() clock)
    pub action_deadline: Option<f64>,
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
    clears_turn: bool,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::GameSnapshot;

    fn call_tx(amount: u64, nonce: u64) -> Vec<u8> {
        Action {
            channel_id: [1u8; 32],
            nonce,
            seat: 0,
            kind: ActionKind::Call { amount },
        }
        .encode()
    }

    fn update(nonce: u64, pot: u64, current_bet: u64) -> StateUpdate {
        StateUpdate {
            channel_id: [1u8; 32],
            nonce,
            state_hash: [0u8; 32],
            balances: vec![(0, 1000)],
            app_data_hash: [0u8; 32],
            participants: vec![[9u8; 32]],
            app_data: Vec::new(),
            game: Some(GameSnapshot {
                phase: 1,
                pot,
                current_bet,
                acting_seat: NO_ACTING_SEAT,
                action_deadline_ms: 0,
            }),
        }
    }

    #[test]
    fn test_prediction_new() {
        let pred = StatePrediction::new();
        assert!(pred.confirmed.is_none());
        assert!(pred.pending.is_empty());
    }

    #[test]
    fn test_call_applied_exactly_once() {
        let mut pred = StatePrediction::new();
        pred.confirm_state(&update(1, 100, 50));
        pred.apply_optimistic_tx(&call_tx(50, 2));
        assert_eq!(pred.predicted().pot, 150);
        assert_eq!(pred.pending.len(), 1);
        // balance debited once
        assert_eq!(pred.predicted().balances[0], (0, 950));
    }

    #[test]
    fn test_confirm_rebase_no_double_count() {
        let mut pred = StatePrediction::new();
        pred.confirm_state(&update(1, 100, 50));
        pred.apply_optimistic_tx(&call_tx(50, 2));
        pred.apply_optimistic_tx(&call_tx(50, 3));
        assert_eq!(pred.predicted().pot, 200);

        // confirm covers nonce 2 and matches the prediction
        pred.confirm_state(&update(2, 150, 50));
        assert_eq!(pred.hits, 1);
        assert_eq!(pred.misses, 0);
        assert_eq!(pred.pending.len(), 1);
        // confirmed 150 + remaining pending call = 200, not 250
        assert_eq!(pred.predicted().pot, 200);
    }

    #[test]
    fn test_contradicted_prediction_counts_miss() {
        let mut pred = StatePrediction::new();
        pred.confirm_state(&update(1, 100, 50));
        pred.apply_optimistic_tx(&call_tx(50, 2));

        // chain says the pot went somewhere else
        pred.confirm_state(&update(2, 999, 50));
        assert_eq!(pred.misses, 1);
        assert_eq!(pred.hits, 0);
        // predicted snaps to confirmed
        assert_eq!(pred.predicted().pot, 999);
    }

    #[test]
    fn test_turn_and_deadline_from_confirmed() {
        let mut pred = StatePrediction::new();
        let mut upd = update(1, 100, 50);
        upd.game = Some(GameSnapshot {
            phase: 1,
            pot: 100,
            current_bet: 50,
            acting_seat: 2,
            action_deadline_ms: 30_000,
        });
        pred.confirm_state(&upd);
        assert_eq!(pred.predicted().acting_seat, Some(2));
        assert!(pred.predicted().action_deadline.is_some());

        // our optimistic action clears the turn
        pred.apply_optimistic_tx(&call_tx(50, 2));
        assert_eq!(pred.predicted().acting_seat, None);
        assert!(pred.predicted().action_deadline.is_none());
    }
}
