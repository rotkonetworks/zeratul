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

    /// get channel state from cache
    pub fn get_channel(&self, channel_id: &[u8; 32]) -> Option<Vec<u8>> {
        self.channels
            .iter()
            .find(|c| &c.channel_id == channel_id)
            .map(|c| c.state_data.clone())
    }

    /// apply state update
    pub fn apply_update(&mut self, update: &StateUpdate) {
        let now = crate::now();

        // find or create channel entry
        let idx = self.channels.iter().position(|c| c.channel_id == update.channel_id);

        match idx {
            Some(i) => {
                // collect balance updates first
                let balance_updates: Vec<_> = {
                    let channel = &self.channels[i];
                    if update.nonce > channel.nonce {
                        update.balances.iter()
                            .filter_map(|(idx, balance)| {
                                channel.participants.get(*idx as usize)
                                    .map(|pk| (*pk, *balance))
                            })
                            .collect()
                    } else {
                        Vec::new()
                    }
                };

                // update channel
                let channel = &mut self.channels[i];
                if update.nonce > channel.nonce {
                    channel.nonce = update.nonce;
                    channel.state_hash = update.state_hash;
                    channel.updated_at = now;
                }

                // apply balance updates
                for (pk, balance) in balance_updates {
                    self.set_balance(pk, balance);
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
                    state_data: Vec::new(),
                    participants: Vec::new(),
                    updated_at: now,
                });
            }
        }

        self.confirmed_nonce = update.nonce;
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
                id: hex_encode(&c.channel_id),
                nonce: c.nonce,
                state_hash: hex_encode(&c.state_hash),
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

fn hex_encode(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{:02x}", b)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_cache_balance() {
        let mut cache = StateCache::new();
        let account = [1u8; 32];

        assert!(cache.get_balance(&account).is_none());

        cache.set_balance(account, 1000);
        assert_eq!(cache.get_balance(&account), Some(1000));
    }
}
