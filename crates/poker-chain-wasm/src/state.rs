//! state cache for fast lookups
//!
//! optimized for 240hz access patterns:
//! - small fixed-size cache for hot data
//! - no allocations in hot path
//! - delta updates

use std::collections::HashMap;
use wasm_bindgen::prelude::*;

use crate::StateUpdate;

/// maximum cached channels
const MAX_CHANNELS: usize = 8;
/// maximum cached balances
const MAX_BALANCES: usize = 32;

/// state cache with LRU-ish eviction
pub struct StateCache {
    /// channel states (hot cache)
    channels: Vec<CachedChannel>,
    /// balance cache
    balances: HashMap<[u8; 32], CachedBalance>,
    /// confirmed state nonce
    confirmed_nonce: u64,
    /// last update time
    last_update: f64,
}

impl StateCache {
    pub fn new() -> Self {
        Self {
            channels: Vec::with_capacity(MAX_CHANNELS),
            balances: HashMap::with_capacity(MAX_BALANCES),
            confirmed_nonce: 0,
            last_update: 0.0,
        }
    }

    /// get balance from cache
    pub fn get_balance(&self, account: &[u8; 32]) -> Option<u64> {
        self.balances.get(account).map(|b| b.balance)
    }

    /// set balance in cache
    pub fn set_balance(&mut self, account: [u8; 32], balance: u64) {
        let now = crate::now();
        self.balances.insert(account, CachedBalance {
            balance,
            updated_at: now,
        });

        // evict old entries if over limit
        if self.balances.len() > MAX_BALANCES {
            self.evict_old_balances();
        }
    }

    /// get channel state (app_data) from cache
    pub fn get_channel(&self, channel_id: &[u8; 32]) -> Option<Vec<u8>> {
        self.channels
            .iter()
            .find(|c| &c.channel_id == channel_id)
            .map(|c| c.state_data.clone())
    }

    /// seat index of account in channel participants
    pub fn participant_seat(&self, channel_id: &[u8; 32], account: &[u8; 32]) -> Option<u8> {
        self.channels
            .iter()
            .find(|c| &c.channel_id == channel_id)
            .and_then(|c| c.participants.iter().position(|p| p == account))
            .map(|i| i as u8)
    }

    /// apply state update; stale nonces (<= cached) are ignored
    pub fn apply_update(&mut self, update: &StateUpdate) {
        let now = crate::now();

        let idx = self.channels.iter().position(|c| c.channel_id == update.channel_id);

        match idx {
            Some(i) => {
                if update.nonce > self.channels[i].nonce {
                    // map (participant_idx, balance) to pubkeys, preferring
                    // the fresh participant list from the update
                    let balance_updates: Vec<_> = {
                        let participants = if update.participants.is_empty() {
                            &self.channels[i].participants
                        } else {
                            &update.participants
                        };
                        update.balances.iter()
                            .filter_map(|(idx, balance)| {
                                participants.get(*idx as usize).map(|pk| (*pk, *balance))
                            })
                            .collect()
                    };

                    let channel = &mut self.channels[i];
                    channel.nonce = update.nonce;
                    channel.state_hash = update.state_hash;
                    channel.state_data = update.app_data.clone();
                    if !update.participants.is_empty() {
                        channel.participants = update.participants.clone();
                    }
                    channel.updated_at = now;

                    for (pk, balance) in balance_updates {
                        self.set_balance(pk, balance);
                    }
                }
            }
            None => {
                // add new channel
                if self.channels.len() >= MAX_CHANNELS {
                    // evict oldest
                    let oldest = self.channels
                        .iter()
                        .enumerate()
                        .min_by(|a, b| a.1.updated_at.partial_cmp(&b.1.updated_at).unwrap())
                        .map(|(i, _)| i)
                        .unwrap_or(0);
                    self.channels.remove(oldest);
                }

                self.channels.push(CachedChannel {
                    channel_id: update.channel_id,
                    nonce: update.nonce,
                    state_hash: update.state_hash,
                    state_data: update.app_data.clone(),
                    participants: update.participants.clone(),
                    updated_at: now,
                });

                for (idx, balance) in &update.balances {
                    if let Some(pk) = update.participants.get(*idx as usize) {
                        self.set_balance(*pk, *balance);
                    }
                }
            }
        }

        // only advance the global confirmed nonce
        if update.nonce > self.confirmed_nonce {
            self.confirmed_nonce = update.nonce;
        }
        self.last_update = now;
    }

    /// get confirmed state as js value
    pub fn get_confirmed_state(&self) -> JsValue {
        #[derive(serde::Serialize)]
        struct ConfirmedState {
            nonce: u64,
            channels: Vec<ChannelSummary>,
        }

        #[derive(serde::Serialize)]
        struct ChannelSummary {
            id: String,
            nonce: u64,
            state_hash: String,
        }

        let state = ConfirmedState {
            nonce: self.confirmed_nonce,
            channels: self.channels.iter().map(|c| ChannelSummary {
                id: crate::hex_encode(&c.channel_id),
                nonce: c.nonce,
                state_hash: crate::hex_encode(&c.state_hash),
            }).collect(),
        };

        serde_wasm_bindgen::to_value(&state).unwrap_or(JsValue::NULL)
    }

    fn evict_old_balances(&mut self) {
        // remove oldest entries until under limit
        let mut entries: Vec<_> = self.balances.iter()
            .map(|(k, v)| (*k, v.updated_at))
            .collect();
        entries.sort_by(|a, b| a.1.partial_cmp(&b.1).unwrap());

        let to_remove = entries.len() - MAX_BALANCES;
        for (key, _) in entries.into_iter().take(to_remove) {
            self.balances.remove(&key);
        }
    }
}

impl Default for StateCache {
    fn default() -> Self {
        Self::new()
    }
}

/// cached channel state
struct CachedChannel {
    channel_id: [u8; 32],
    nonce: u64,
    state_hash: [u8; 32],
    state_data: Vec<u8>,
    participants: Vec<[u8; 32]>,
    updated_at: f64,
}

/// cached balance
struct CachedBalance {
    balance: u64,
    updated_at: f64,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn update(channel: u8, nonce: u64) -> StateUpdate {
        StateUpdate {
            channel_id: [channel; 32],
            nonce,
            state_hash: [nonce as u8; 32],
            balances: Vec::new(),
            app_data_hash: [0u8; 32],
            participants: Vec::new(),
            app_data: Vec::new(),
            game: None,
        }
    }

    #[test]
    fn test_cache_balance() {
        let mut cache = StateCache::new();
        let account = [1u8; 32];

        assert!(cache.get_balance(&account).is_none());

        cache.set_balance(account, 1000);
        assert_eq!(cache.get_balance(&account), Some(1000));
    }

    #[test]
    fn test_balance_eviction() {
        let mut cache = StateCache::new();
        for i in 0..(MAX_BALANCES + 4) {
            cache.set_balance([i as u8; 32], i as u64);
        }
        assert!(cache.balances.len() <= MAX_BALANCES);
    }

    #[test]
    fn test_channel_eviction() {
        let mut cache = StateCache::new();
        for i in 0..(MAX_CHANNELS + 2) {
            cache.apply_update(&update(i as u8, 1));
        }
        assert_eq!(cache.channels.len(), MAX_CHANNELS);
    }

    #[test]
    fn test_stale_update_ignored() {
        let mut cache = StateCache::new();
        cache.apply_update(&update(1, 5));
        cache.apply_update(&update(1, 3));

        assert_eq!(cache.confirmed_nonce, 5);
        let channel = &cache.channels[0];
        assert_eq!(channel.nonce, 5);
        assert_eq!(channel.state_hash, [5u8; 32]);
    }

    #[test]
    fn test_participants_and_balances_from_update() {
        let mut cache = StateCache::new();
        let alice = [7u8; 32];
        let bob = [8u8; 32];

        let mut upd = update(1, 1);
        upd.participants = vec![alice, bob];
        upd.balances = vec![(0, 500), (1, 600)];
        upd.app_data = vec![1, 2, 3];
        cache.apply_update(&upd);

        assert_eq!(cache.get_balance(&alice), Some(500));
        assert_eq!(cache.get_balance(&bob), Some(600));
        assert_eq!(cache.get_channel(&[1u8; 32]), Some(vec![1, 2, 3]));
        assert_eq!(cache.participant_seat(&[1u8; 32], &bob), Some(1));

        // newer update without participants keeps the cached list
        let mut upd2 = update(1, 2);
        upd2.balances = vec![(0, 450)];
        cache.apply_update(&upd2);
        assert_eq!(cache.get_balance(&alice), Some(450));
        assert_eq!(cache.participant_seat(&[1u8; 32], &bob), Some(1));
    }
}
