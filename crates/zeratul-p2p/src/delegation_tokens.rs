//! Delegation tokens: delZT(v) per validator
//!
//! ## How it works (same as Penumbra):
//!
//! 1. **Delegate ZT to validator v**:
//!    - Burn X ZT
//!    - Mint Y delZT(v) where Y = X / ψ_v(epoch)
//!
//! 2. **Exchange rate ψ_v grows over time**:
//!    - Your delZT(v) amount stays constant
//!    - But its value in ZT increases
//!    - Exchange rate factors in supply growth
//!
//! 3. **Undelegate**:
//!    - Burn Y delZT(v)
//!    - Mint X ZT where X = Y * ψ_v(epoch)
//!
//! ## Supply dynamics:
//!
//! - Total ZT supply grows 10% per year (from fee burns being less than issuance)
//! - Unbonded ZT: Loses value (% of total supply decreases)
//! - Bonded delZT(v): Maintains value (exchange rate ψ_v tracks supply growth)
//!
//! ## Example:
//!
//! ```text
//! Epoch 0: Total supply = 100M ZT
//!   Alice: Delegate 10M ZT to validator V
//!          Receive 10M delZT(V) at ψ_v(0) = 1.0
//!   Bob:   Hold 10M ZT unbonded
//!
//! Epoch 1: Total supply = 110M ZT (10% growth)
//!   Alice: Still has 10M delZT(V), now ψ_v(1) = 1.1
//!          Can undelegate to get 11M ZT ✅ Maintained 10% of supply
//!   Bob:   Still has 10M ZT
//!          Now only 9.09% of supply ❌ Lost value
//! ```

use crate::{
    consensus::BlockNumber,
    zswap::{DexState, TOTAL_SUPPLY_ZT},
};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Validator identifier
pub type ValidatorId = [u8; 32];

/// Exchange rate for delegation tokens
///
/// ψ_v(e) = exchange rate at epoch e for validator v
///
/// Grows as: ψ_v(e+1) = ψ_v(e) * (1 + supply_growth_rate)
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct ExchangeRate {
    /// Current rate (amount of ZT per 1 delZT)
    /// Starts at 1.0, grows over time
    pub rate: f64,
}

impl ExchangeRate {
    /// Genesis exchange rate (1:1)
    pub fn genesis() -> Self {
        Self { rate: 1.0 }
    }

    /// Convert ZT → delZT at current rate
    pub fn zt_to_delzt(&self, zt_amount: u64) -> u64 {
        ((zt_amount as f64) / self.rate) as u64
    }

    /// Convert delZT → ZT at current rate
    pub fn delzt_to_zt(&self, delzt_amount: u64) -> u64 {
        ((delzt_amount as f64) * self.rate) as u64
    }

    /// Update rate based on supply growth
    ///
    /// new_rate = old_rate * (new_supply / old_supply)
    pub fn update(&mut self, old_supply: u64, new_supply: u64) {
        if old_supply == 0 {
            return;
        }

        let growth_factor = new_supply as f64 / old_supply as f64;
        self.rate *= growth_factor;
    }
}

/// Delegation pool for a validator
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DelegationPool {
    /// Validator ID
    pub validator_id: ValidatorId,

    /// Total delZT(v) in existence for this validator
    pub total_delzt: u64,

    /// Exchange rate ψ_v(epoch)
    /// This is validator-specific and can differ based on:
    /// - Validator uptime (missed blocks = lower ψ_v)
    /// - Validator commission (commission reduces delegator rewards)
    pub exchange_rate: ExchangeRate,

    /// Validator commission rate (basis points)
    /// e.g., 500 = 5% commission
    pub commission_bps: u64,

    /// Epoch when last updated
    pub last_update_epoch: u64,
}

impl DelegationPool {
    /// Create new delegation pool at genesis
    pub fn new(validator_id: ValidatorId) -> Self {
        Self::with_commission(validator_id, 0)
    }

    /// Create pool with commission rate
    pub fn with_commission(validator_id: ValidatorId, commission_bps: u64) -> Self {
        Self {
            validator_id,
            total_delzt: 0,
            exchange_rate: ExchangeRate::genesis(),
            commission_bps,
            last_update_epoch: 0,
        }
    }

    /// Delegate ZT → get delZT
    pub fn delegate(&mut self, zt_amount: u64) -> u64 {
        let delzt_amount = self.exchange_rate.zt_to_delzt(zt_amount);
        self.total_delzt += delzt_amount;
        delzt_amount
    }

    /// Undelegate delZT → get ZT
    pub fn undelegate(&mut self, delzt_amount: u64) -> Result<u64, DelegationError> {
        if delzt_amount > self.total_delzt {
            return Err(DelegationError::InsufficientDelegation);
        }

        self.total_delzt -= delzt_amount;
        let zt_amount = self.exchange_rate.delzt_to_zt(delzt_amount);

        Ok(zt_amount)
    }

    /// Update exchange rate based on inflation distribution
    ///
    /// Like Penumbra:
    /// - Inflation minted (2% per year)
    /// - Validator takes commission first
    /// - Remaining distributed to delegators via exchange rate
    /// - Supply grows over time
    pub fn update_exchange_rate_from_inflation(
        &mut self,
        inflation_share: u64,
        epoch: u64,
    ) -> u64 {
        if self.total_delzt == 0 {
            return 0;
        }

        // Validator commission (taken first)
        let commission = (inflation_share * self.commission_bps) / 10000;
        let delegator_rewards = inflation_share - commission;

        // Current ZT value of pool
        let current_zt_value = self.exchange_rate.delzt_to_zt(self.total_delzt);

        // New ZT value after inflation rewards (minus commission)
        let new_zt_value = current_zt_value + delegator_rewards;

        // Update exchange rate
        self.exchange_rate.rate = new_zt_value as f64 / self.total_delzt as f64;
        self.last_update_epoch = epoch;

        // Return commission (validator keeps this)
        commission
    }

    /// Get total ZT value of pool
    pub fn total_zt_value(&self) -> u64 {
        self.exchange_rate.delzt_to_zt(self.total_delzt)
    }

    /// Apply slashing to this validator
    ///
    /// Reduces the exchange rate ψ_v by the penalty percentage
    /// This affects ALL delegators to this validator
    pub fn slash(&mut self, penalty_bps: u64, epoch: u64) {
        // Reduce exchange rate by penalty %
        // Example: 10% slash (1000 bps) → rate *= 0.9
        let penalty_fraction = penalty_bps as f64 / 10000.0;
        self.exchange_rate.rate *= 1.0 - penalty_fraction;
        self.last_update_epoch = epoch;
    }
}

/// Global delegation state
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DelegationState {
    /// All delegation pools by validator
    pools: HashMap<ValidatorId, DelegationPool>,

    /// Current epoch
    current_epoch: u64,

    /// Total ZT supply at last epoch
    last_supply: u64,

    /// Current total ZT supply
    current_supply: u64,
}

impl DelegationState {
    /// Create at genesis
    pub fn genesis(initial_supply: u64) -> Self {
        Self {
            pools: HashMap::new(),
            current_epoch: 0,
            last_supply: initial_supply,
            current_supply: initial_supply,
        }
    }

    /// Get or create delegation pool for validator
    pub fn get_pool_mut(&mut self, validator_id: ValidatorId) -> &mut DelegationPool {
        self.pools
            .entry(validator_id)
            .or_insert_with(|| DelegationPool::new(validator_id))
    }

    /// Get delegation pool
    pub fn get_pool(&self, validator_id: &ValidatorId) -> Option<&DelegationPool> {
        self.pools.get(validator_id)
    }

    /// Update to new epoch with inflation rewards
    ///
    /// Like Penumbra:
    /// - Mint new tokens (2% per year inflation)
    /// - Distribute proportionally to all bonded holders
    /// - Updates exchange rate ψ_v for each validator pool
    pub fn new_epoch_with_inflation(&mut self, inflation_minted: u64, epoch: u64) {
        self.current_epoch = epoch;

        if inflation_minted == 0 || self.pools.is_empty() {
            return;
        }

        // Total bonded ZT across all validators
        let total_bonded_zt: u64 = self.pools.values().map(|p| p.total_zt_value()).sum();

        if total_bonded_zt == 0 {
            return;
        }

        // Distribute inflation proportionally to each pool
        for pool in self.pools.values_mut() {
            let pool_zt_value = pool.total_zt_value();
            let pool_share = (pool_zt_value as f64 / total_bonded_zt as f64) * inflation_minted as f64;

            pool.update_exchange_rate_from_inflation(pool_share as u64, epoch);
        }
    }

    /// Delegate ZT to validator
    pub fn delegate(
        &mut self,
        validator_id: ValidatorId,
        zt_amount: u64,
    ) -> Result<u64, DelegationError> {
        let pool = self.get_pool_mut(validator_id);
        let delzt_amount = pool.delegate(zt_amount);

        Ok(delzt_amount)
    }

    /// Undelegate delZT from validator
    pub fn undelegate(
        &mut self,
        validator_id: ValidatorId,
        delzt_amount: u64,
    ) -> Result<u64, DelegationError> {
        let pool = self
            .pools
            .get_mut(&validator_id)
            .ok_or(DelegationError::ValidatorNotFound)?;

        pool.undelegate(delzt_amount)
    }

    /// Get total staked ZT across all validators
    pub fn total_staked(&self) -> u64 {
        self.pools.values().map(|p| p.total_zt_value()).sum()
    }

    /// Get exchange rate for validator
    pub fn exchange_rate(&self, validator_id: &ValidatorId) -> Option<ExchangeRate> {
        self.pools.get(validator_id).map(|p| p.exchange_rate)
    }
}

#[derive(Debug, thiserror::Error)]
pub enum DelegationError {
    #[error("Validator not found")]
    ValidatorNotFound,

    #[error("Insufficient delegation")]
    InsufficientDelegation,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_exchange_rate_tracks_supply_growth() {
        let mut rate = ExchangeRate::genesis();
        assert_eq!(rate.rate, 1.0);

        // Supply grows from 100M to 110M (10% growth)
        rate.update(100_000_000, 110_000_000);

        // Rate should be 1.1 (each delZT now worth 1.1 ZT)
        assert!((rate.rate - 1.1).abs() < 0.01);
    }

    #[test]
    fn test_delegation_maintains_value() {
        let mut state = DelegationState::genesis(100_000_000);
        let validator = [1u8; 32];

        // Alice delegates 10M ZT at epoch 0
        let alice_delzt = state.delegate(validator, 10_000_000).unwrap();
        assert_eq!(alice_delzt, 10_000_000); // 1:1 at genesis

        // Epoch 1: Supply grows to 110M
        state.new_epoch(110_000_000, 1);

        // Alice undelegates - should get back 11M ZT
        let alice_zt = state.undelegate(validator, alice_delzt).unwrap();
        assert!(alice_zt >= 10_900_000 && alice_zt <= 11_100_000); // ~11M

        println!("Alice delegated 10M, got back {} (supply grew 10%)", alice_zt);
    }

    #[test]
    fn test_unbonded_loses_value() {
        // Bob holds 10M ZT unbonded
        let bob_zt = 10_000_000u64;
        let initial_supply = 100_000_000u64;
        let new_supply = 110_000_000u64;

        // Bob's % of supply at genesis
        let initial_pct = (bob_zt as f64 / initial_supply as f64) * 100.0;

        // Bob's % of supply after growth
        let final_pct = (bob_zt as f64 / new_supply as f64) * 100.0;

        let lost_pct = initial_pct - final_pct;

        println!("Bob lost {}% of the network by not bonding", lost_pct);

        // Should lose ~0.9% (from 10% to 9.09%)
        assert!(lost_pct > 0.8 && lost_pct < 1.0);
    }

    #[test]
    fn test_slashing_reduces_exchange_rate() {
        let mut pool = DelegationPool::new([1u8; 32]);

        // Alice delegates 10M ZT
        let alice_delzt = pool.delegate(10_000_000);
        assert_eq!(alice_delzt, 10_000_000); // 1:1 at start

        // Pool gets slashed 10% (1000 bps)
        pool.slash(1000, 1);

        // Exchange rate should be 0.9
        assert!((pool.exchange_rate.rate - 0.9).abs() < 0.01);

        // Alice undelegates
        let alice_zt = pool.undelegate(alice_delzt).unwrap();

        // Alice gets 9M ZT (lost 10% to slashing)
        assert!(alice_zt > 8_900_000 && alice_zt < 9_100_000);

        println!("After 10% slash:");
        println!("  Alice delegated: 10M ZT");
        println!("  Alice receives: {} ZT", alice_zt);
        println!("  Lost to slash: {} ZT", 10_000_000 - alice_zt);
    }

    #[test]
    fn test_slashing_affects_all_delegators() {
        let mut pool = DelegationPool::new([1u8; 32]);

        // Alice and Bob both delegate
        let alice_delzt = pool.delegate(10_000_000);
        let bob_delzt = pool.delegate(5_000_000);

        // Validator slashed 20%
        pool.slash(2000, 1);

        // Both lose 20%
        let alice_zt = pool.undelegate(alice_delzt).unwrap();
        let bob_zt = pool.undelegate(bob_delzt).unwrap();

        println!("After 20% slash:");
        println!("  Alice: {} ZT (lost {})", alice_zt, 10_000_000 - alice_zt);
        println!("  Bob: {} ZT (lost {})", bob_zt, 5_000_000 - bob_zt);

        // Alice lost ~2M, Bob lost ~1M
        assert!(alice_zt > 7_900_000 && alice_zt < 8_100_000);
        assert!(bob_zt > 3_900_000 && bob_zt < 4_100_000);
    }
}
