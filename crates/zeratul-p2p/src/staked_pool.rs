//! Staked pool token: sZT
//!
//! ## Design:
//!
//! Instead of individual delZT(v) per validator, pool all validators together:
//!
//! - **sZT**: Staked ZT (pooled across all validators)
//! - **Single exchange rate**: ψ (not per-validator)
//! - **Risk sharing**: If any validator slashed, ALL sZT holders share the loss
//! - **Simplicity**: Users don't pick validators, just stake
//!
//! ## How it works:
//!
//! 1. **Stake ZT → get sZT**:
//!    - Burn X ZT
//!    - Mint Y sZT where Y = X / ψ(epoch)
//!
//! 2. **Pool manages validators**:
//!    - Distributes stake across all active validators
//!    - Automatically rebalances
//!    - Collects rewards from all validators
//!
//! 3. **Slashing affects exchange rate**:
//!    - If validator V slashed (for misbehavior)
//!    - Pool loses X% of stake delegated to V
//!    - Exchange rate ψ decreases proportionally
//!    - ALL sZT holders share the loss
//!
//! ## Slashing conditions:
//!
//! Validators are slashed for:
//!
//! 1. **Invalid batch proof** (10% slash):
//!    - Validator signs a batch with incorrect PolkaVM proof
//!    - Other validators verify and detect invalid proof
//!    - Slashed validator loses 10% of bonded stake
//!
//! 2. **Double-signing** (20% slash):
//!    - Validator signs two conflicting batches for same block
//!    - Provable misbehavior (submit both signatures)
//!    - Slashed 20% for attempting to fork
//!
//! 3. **Liveness failure** (1% slash):
//!    - Validator offline for extended period (e.g., 1000 blocks)
//!    - Doesn't participate in consensus
//!    - Small slash to encourage uptime
//!
//! 4. **Unstake sZT → get ZT**:
//!    - Burn Y sZT
//!    - Mint X ZT where X = Y * ψ(epoch)
//!
//! ## Example:
//!
//! ```text
//! Day 0:
//!   Alice stakes 10M ZT → gets 10M sZT
//!   Pool distributes:
//!     - 5M ZT to Validator A
//!     - 5M ZT to Validator B
//!
//! Day 100:
//!   Validator B is slashed 10%
//!     - Loses 500K ZT
//!     - Pool total: 10M - 500K = 9.5M ZT
//!     - Exchange rate: ψ = 9.5M / 10M = 0.95
//!
//!   Alice unstakes:
//!     - Burns 10M sZT
//!     - Gets 10M * 0.95 = 9.5M ZT
//!     - Lost 500K ZT (shared slashing risk)
//! ```
//!
//! ## Benefits vs individual delZT(v):
//!
//! - ✅ Simpler for users (no validator selection)
//! - ✅ Automatic diversification (spread across validators)
//! - ✅ Single token (easier for DeFi integration)
//! - ❌ Share slashing risk (can't avoid bad validators)
//! - ❌ Can't optimize commission (pool average)

use crate::{
    consensus::BlockNumber,
    delegation_tokens::{ValidatorId, DelegationState},
    staking_rewards::StakingRewards,
};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Exchange rate for sZT (pooled staking token)
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct PoolExchangeRate {
    /// Current rate (amount of ZT per 1 sZT)
    pub rate: f64,
}

impl PoolExchangeRate {
    /// Genesis rate (1:1)
    pub fn genesis() -> Self {
        Self { rate: 1.0 }
    }

    /// Convert ZT → sZT
    pub fn zt_to_szt(&self, zt_amount: u64) -> u64 {
        ((zt_amount as f64) / self.rate) as u64
    }

    /// Convert sZT → ZT
    pub fn szt_to_zt(&self, szt_amount: u64) -> u64 {
        ((szt_amount as f64) * self.rate) as u64
    }

    /// Apply slashing penalty
    ///
    /// When a validator is slashed, the pool loses value
    pub fn apply_slashing(&mut self, penalty_bps: u64, validator_share: f64) {
        // penalty_bps = slashing penalty (e.g., 1000 = 10%)
        // validator_share = % of pool delegated to slashed validator

        let penalty_fraction = (penalty_bps as f64 / 10000.0) * validator_share;
        self.rate *= 1.0 - penalty_fraction;
    }

    /// Apply inflation rewards
    pub fn apply_inflation(&mut self, total_szt: u64, inflation_received: u64) {
        if total_szt == 0 {
            return;
        }

        // Current ZT value
        let current_zt = self.szt_to_zt(total_szt);

        // New ZT value after inflation
        let new_zt = current_zt + inflation_received;

        // Update rate
        self.rate = new_zt as f64 / total_szt as f64;
    }
}

/// Staked pool (sZT)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StakedPool {
    /// Total sZT in circulation
    pub total_szt: u64,

    /// Exchange rate (ZT per sZT)
    pub exchange_rate: PoolExchangeRate,

    /// How much delegated to each validator (in ZT)
    pub validator_allocations: HashMap<ValidatorId, u64>,

    /// Last epoch when updated
    pub last_update_epoch: u64,
}

impl StakedPool {
    /// Create at genesis
    pub fn new() -> Self {
        Self {
            total_szt: 0,
            exchange_rate: PoolExchangeRate::genesis(),
            validator_allocations: HashMap::new(),
            last_update_epoch: 0,
        }
    }

    /// Stake ZT → get sZT
    pub fn stake(&mut self, zt_amount: u64) -> u64 {
        let szt_amount = self.exchange_rate.zt_to_szt(zt_amount);
        self.total_szt += szt_amount;
        szt_amount
    }

    /// Unstake sZT → get ZT
    pub fn unstake(&mut self, szt_amount: u64) -> Result<u64, StakedPoolError> {
        if szt_amount > self.total_szt {
            return Err(StakedPoolError::InsufficientStake);
        }

        let zt_amount = self.exchange_rate.szt_to_zt(szt_amount);
        self.total_szt -= szt_amount;

        Ok(zt_amount)
    }

    /// Distribute pool stake across validators
    ///
    /// Simple strategy: equal distribution
    pub fn rebalance_validators(&mut self, active_validators: &[ValidatorId]) {
        if active_validators.is_empty() {
            return;
        }

        self.validator_allocations.clear();

        let total_zt = self.exchange_rate.szt_to_zt(self.total_szt);
        let per_validator = total_zt / active_validators.len() as u64;

        for validator in active_validators {
            self.validator_allocations.insert(*validator, per_validator);
        }
    }

    /// Apply slashing to a validator
    ///
    /// Reduces exchange rate proportionally
    pub fn slash_validator(&mut self, validator_id: &ValidatorId, penalty_bps: u64) {
        let validator_stake = self.validator_allocations.get(validator_id).copied().unwrap_or(0);

        if validator_stake == 0 {
            return;
        }

        let total_zt = self.exchange_rate.szt_to_zt(self.total_szt);
        let validator_share = validator_stake as f64 / total_zt as f64;

        // Apply slashing to exchange rate
        self.exchange_rate.apply_slashing(penalty_bps, validator_share);

        // Reduce validator allocation
        let slashed_amount = (validator_stake * penalty_bps) / 10000;
        self.validator_allocations.insert(*validator_id, validator_stake - slashed_amount);
    }

    /// Collect inflation rewards from all validators
    pub fn collect_rewards(&mut self, inflation_received: u64, epoch: u64) {
        self.exchange_rate.apply_inflation(self.total_szt, inflation_received);
        self.last_update_epoch = epoch;
    }

    /// Get total ZT value of pool
    pub fn total_zt_value(&self) -> u64 {
        self.exchange_rate.szt_to_zt(self.total_szt)
    }

    /// Get pool stats
    pub fn stats(&self) -> PoolStats {
        PoolStats {
            total_szt: self.total_szt,
            total_zt_value: self.total_zt_value(),
            exchange_rate: self.exchange_rate.rate,
            num_validators: self.validator_allocations.len(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PoolStats {
    pub total_szt: u64,
    pub total_zt_value: u64,
    pub exchange_rate: f64,
    pub num_validators: usize,
}

#[derive(Debug, thiserror::Error)]
pub enum StakedPoolError {
    #[error("Insufficient stake")]
    InsufficientStake,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_stake_and_unstake() {
        let mut pool = StakedPool::new();

        // Stake 10M ZT
        let szt = pool.stake(10_000_000);
        assert_eq!(szt, 10_000_000); // 1:1 at genesis

        // Unstake
        let zt = pool.unstake(szt).unwrap();
        assert_eq!(zt, 10_000_000);
    }

    #[test]
    fn test_slashing_affects_all_holders() {
        let mut pool = StakedPool::new();

        // Alice and Bob stake
        let alice_szt = pool.stake(10_000_000);
        let bob_szt = pool.stake(10_000_000);

        // Distribute across 2 validators
        let v1 = [1u8; 32];
        let v2 = [2u8; 32];
        pool.rebalance_validators(&[v1, v2]);

        // Validator 1 is slashed 10%
        pool.slash_validator(&v1, 1000);

        // Both Alice and Bob lose 5% (10% slash on 50% of pool)
        let alice_zt = pool.unstake(alice_szt).unwrap();
        let bob_zt = pool.unstake(bob_szt).unwrap();

        // Each should get ~9.5M ZT (lost 500K)
        assert!(alice_zt > 9_400_000 && alice_zt < 9_600_000);
        assert!(bob_zt > 9_400_000 && bob_zt < 9_600_000);

        println!("After slashing:");
        println!("  Alice: {} ZT (lost {})", alice_zt, 10_000_000 - alice_zt);
        println!("  Bob: {} ZT (lost {})", bob_zt, 10_000_000 - bob_zt);
    }

    #[test]
    fn test_inflation_increases_exchange_rate() {
        let mut pool = StakedPool::new();

        let szt = pool.stake(10_000_000);

        // Receive 1M ZT inflation
        pool.collect_rewards(1_000_000, 1);

        // Exchange rate should be 1.1
        assert!((pool.exchange_rate.rate - 1.1).abs() < 0.01);

        // Unstake should get 11M ZT
        let zt = pool.unstake(szt).unwrap();
        assert!(zt > 10_900_000 && zt < 11_100_000);
    }
}
