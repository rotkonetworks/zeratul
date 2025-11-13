//! Reward Distribution System
//!
//! Calculates and distributes block rewards to validators and their nominators.

use super::{AccountId, Balance, EraIndex, ValidatorIndex};
use super::validator_selection::ValidatorSet;
use anyhow::{bail, Result};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// Reward pool for an era
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RewardPool {
    /// Era number
    pub era: EraIndex,

    /// Total rewards generated this era
    pub total_rewards: Balance,

    /// Rewards per validator
    pub validator_rewards: BTreeMap<ValidatorIndex, ValidatorReward>,

    /// Pending payouts (not yet distributed)
    pub pending_payouts: Vec<PayoutInfo>,
}

impl RewardPool {
    /// Create new reward pool
    pub fn new(era: EraIndex) -> Self {
        Self {
            era,
            total_rewards: 0,
            validator_rewards: BTreeMap::new(),
            pending_payouts: Vec::new(),
        }
    }

    /// Add block reward
    pub fn add_block_reward(&mut self, validator_index: ValidatorIndex, amount: Balance) {
        self.total_rewards += amount;

        let reward = self
            .validator_rewards
            .entry(validator_index)
            .or_insert_with(|| ValidatorReward::new(validator_index));

        reward.total_earned += amount;
    }

    /// Calculate payouts for entire era
    pub fn calculate_payouts(&mut self, validator_set: &ValidatorSet) -> Result<Vec<PayoutInfo>> {
        let mut payouts = Vec::new();

        for (index, validator_reward) in &self.validator_rewards {
            let validator = validator_set
                .get_by_index(*index)
                .ok_or_else(|| anyhow::anyhow!("Validator not found for index {}", index))?;

            // Calculate commission
            let commission_amount =
                (validator_reward.total_earned * validator.commission as u128) / 100;

            // Remaining for nominators
            let nominator_pool = validator_reward.total_earned - commission_amount;

            // Validator gets commission + their share of nominator pool
            let validator_share_of_pool = if validator.total_backing > 0 {
                (nominator_pool * validator.self_stake) / validator.total_backing
            } else {
                0
            };

            let validator_total = commission_amount + validator_share_of_pool;

            // Add validator payout
            payouts.push(PayoutInfo {
                era: self.era,
                recipient: validator.account,
                amount: validator_total,
                payout_type: PayoutType::ValidatorReward,
            });

            // Calculate nominator payouts
            for (nominator, backing) in &validator.nominators {
                if *backing == 0 || validator.total_backing == 0 {
                    continue;
                }

                let nominator_share = (nominator_pool * backing) / validator.total_backing;

                if nominator_share > 0 {
                    payouts.push(PayoutInfo {
                        era: self.era,
                        recipient: *nominator,
                        amount: nominator_share,
                        payout_type: PayoutType::NominatorReward,
                    });
                }
            }
        }

        self.pending_payouts = payouts.clone();
        Ok(payouts)
    }

    /// Mark payouts as distributed
    pub fn mark_distributed(&mut self) {
        self.pending_payouts.clear();
    }
}

/// Validator reward info
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ValidatorReward {
    /// Validator index
    pub validator_index: ValidatorIndex,

    /// Total earned this era
    pub total_earned: Balance,

    /// Number of blocks produced
    pub blocks_produced: u64,
}

impl ValidatorReward {
    fn new(validator_index: ValidatorIndex) -> Self {
        Self {
            validator_index,
            total_earned: 0,
            blocks_produced: 0,
        }
    }
}

/// Payout information
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PayoutInfo {
    /// Era number
    pub era: EraIndex,

    /// Recipient account
    pub recipient: AccountId,

    /// Payout amount
    pub amount: Balance,

    /// Type of payout
    pub payout_type: PayoutType,
}

/// Type of payout
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum PayoutType {
    /// Validator commission + share
    ValidatorReward,

    /// Nominator share
    NominatorReward,
}

/// Reward distribution manager
pub struct RewardDistributor {
    /// Reward pools by era
    pools: BTreeMap<EraIndex, RewardPool>,

    /// Configuration
    config: RewardConfig,
}

#[derive(Debug, Clone)]
pub struct RewardConfig {
    /// Block reward amount (in base units)
    pub block_reward: Balance,

    /// Blocks per era (for calculating era rewards)
    pub blocks_per_era: u64,
}

impl Default for RewardConfig {
    fn default() -> Self {
        Self {
            block_reward: 10 * 10u128.pow(18),  // 10 ZT per block
            blocks_per_era: 43_200,              // 24 hours at 2-second blocks
        }
    }
}

impl RewardDistributor {
    /// Create new reward distributor
    pub fn new(config: RewardConfig) -> Self {
        Self {
            pools: BTreeMap::new(),
            config,
        }
    }

    /// Record block reward
    pub fn record_block_reward(&mut self, era: EraIndex, validator_index: ValidatorIndex) {
        let pool = self
            .pools
            .entry(era)
            .or_insert_with(|| RewardPool::new(era));

        pool.add_block_reward(validator_index, self.config.block_reward);

        let reward = pool
            .validator_rewards
            .entry(validator_index)
            .or_insert_with(|| ValidatorReward::new(validator_index));

        reward.blocks_produced += 1;
    }

    /// Calculate payouts for era
    pub fn calculate_era_payouts(
        &mut self,
        era: EraIndex,
        validator_set: &ValidatorSet,
    ) -> Result<Vec<PayoutInfo>> {
        let pool = self
            .pools
            .get_mut(&era)
            .ok_or_else(|| anyhow::anyhow!("No reward pool for era {}", era))?;

        pool.calculate_payouts(validator_set)
    }

    /// Mark era payouts as distributed
    pub fn mark_era_distributed(&mut self, era: EraIndex) -> Result<()> {
        let pool = self
            .pools
            .get_mut(&era)
            .ok_or_else(|| anyhow::anyhow!("No reward pool for era {}", era))?;

        pool.mark_distributed();
        Ok(())
    }

    /// Get reward pool for era
    pub fn get_pool(&self, era: EraIndex) -> Option<&RewardPool> {
        self.pools.get(&era)
    }

    /// Get total rewards for validator in era
    pub fn get_validator_rewards(&self, era: EraIndex, validator_index: ValidatorIndex) -> Option<Balance> {
        self.pools
            .get(&era)
            .and_then(|pool| pool.validator_rewards.get(&validator_index))
            .map(|reward| reward.total_earned)
    }

    /// Get expected era rewards
    pub fn expected_era_rewards(&self) -> Balance {
        self.config.block_reward * self.config.blocks_per_era as u128
    }

    /// Calculate APY for a validator
    ///
    /// APY = (era_rewards / validator_backing) * (eras_per_year) * 100
    pub fn calculate_validator_apy(
        &self,
        era_rewards: Balance,
        validator_backing: Balance,
        eras_per_year: u64,
    ) -> f64 {
        if validator_backing == 0 {
            return 0.0;
        }

        let era_return = era_rewards as f64 / validator_backing as f64;
        let annual_return = era_return * eras_per_year as f64;
        annual_return * 100.0
    }
}

/// Calculate nominator APY
///
/// Takes into account validator commission
pub fn calculate_nominator_apy(
    validator_apy: f64,
    validator_commission: u8,
) -> f64 {
    validator_apy * (1.0 - (validator_commission as f64 / 100.0))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::governance::validator_selection::{ActiveValidator, ValidatorSet};
    use std::net::SocketAddr;

    fn create_test_account(id: u8) -> AccountId {
        let mut account = [0u8; 32];
        account[0] = id;
        account
    }

    fn create_test_validator(index: ValidatorIndex, self_stake: Balance) -> ActiveValidator {
        ActiveValidator {
            account: create_test_account(index as u8),
            index,
            consensus_key: [index as u8; 32],
            frost_key: [index as u8; 32],
            endpoint: "127.0.0.1:8000".parse().unwrap(),
            total_backing: self_stake,
            self_stake,
            commission: 10,
            nominators: BTreeMap::new(),
        }
    }

    fn create_test_validator_set() -> ValidatorSet {
        let mut validators = Vec::new();
        for i in 0..3 {
            validators.push(create_test_validator(i, 50_000 * 10u128.pow(18)));
        }

        ValidatorSet {
            era: 1,
            validators,
            total_stake: 150_000 * 10u128.pow(18),
            frost_public_key: None,
        }
    }

    #[test]
    fn test_record_block_rewards() {
        let mut distributor = RewardDistributor::new(RewardConfig::default());

        // Record rewards for 3 validators
        for _ in 0..10 {
            distributor.record_block_reward(1, 0);
        }
        for _ in 0..8 {
            distributor.record_block_reward(1, 1);
        }
        for _ in 0..12 {
            distributor.record_block_reward(1, 2);
        }

        let pool = distributor.get_pool(1).unwrap();
        assert_eq!(pool.validator_rewards.len(), 3);

        let v0_reward = pool.validator_rewards.get(&0).unwrap();
        assert_eq!(v0_reward.blocks_produced, 10);
        assert_eq!(v0_reward.total_earned, 10 * 10 * 10u128.pow(18));
    }

    #[test]
    fn test_payout_calculation() {
        let mut distributor = RewardDistributor::new(RewardConfig::default());
        let validator_set = create_test_validator_set();

        // Record some rewards
        for _ in 0..10 {
            distributor.record_block_reward(1, 0);
        }

        let payouts = distributor
            .calculate_era_payouts(1, &validator_set)
            .unwrap();

        // Should have exactly 1 payout (validator only, no nominators)
        assert_eq!(payouts.len(), 1);

        let payout = &payouts[0];
        assert_eq!(payout.recipient, create_test_account(0));
        assert_eq!(payout.payout_type, PayoutType::ValidatorReward);

        // Total reward = 10 blocks * 10 ZT = 100 ZT
        // Commission = 10% = 10 ZT
        // Validator share = 90 ZT (100% backing from self-stake)
        // Total = 100 ZT
        let expected = 100 * 10u128.pow(18);
        assert_eq!(payout.amount, expected);
    }

    #[test]
    fn test_payout_with_nominators() {
        let mut distributor = RewardDistributor::new(RewardConfig::default());

        // Create validator with nominators
        let mut validator = create_test_validator(0, 50_000 * 10u128.pow(18));
        validator.nominators.insert(
            create_test_account(100),
            30_000 * 10u128.pow(18),
        );
        validator.nominators.insert(
            create_test_account(101),
            20_000 * 10u128.pow(18),
        );
        validator.total_backing = 100_000 * 10u128.pow(18);

        let validator_set = ValidatorSet {
            era: 1,
            validators: vec![validator],
            total_stake: 100_000 * 10u128.pow(18),
            frost_public_key: None,
        };

        // Record rewards
        for _ in 0..10 {
            distributor.record_block_reward(1, 0);
        }

        let payouts = distributor
            .calculate_era_payouts(1, &validator_set)
            .unwrap();

        // Should have 3 payouts: validator + 2 nominators
        assert_eq!(payouts.len(), 3);

        // Find validator payout
        let validator_payout = payouts
            .iter()
            .find(|p| p.payout_type == PayoutType::ValidatorReward)
            .unwrap();

        // Total reward = 100 ZT
        // Commission = 10 ZT
        // Remaining = 90 ZT
        // Validator share of remaining = 90 * (50K/100K) = 45 ZT
        // Total validator = 10 + 45 = 55 ZT
        let expected_validator = 55 * 10u128.pow(18);
        assert_eq!(validator_payout.amount, expected_validator);

        // Nominator 1: 90 * (30K/100K) = 27 ZT
        let nom1_payout = payouts
            .iter()
            .find(|p| p.recipient == create_test_account(100))
            .unwrap();
        assert_eq!(nom1_payout.amount, 27 * 10u128.pow(18));

        // Nominator 2: 90 * (20K/100K) = 18 ZT
        let nom2_payout = payouts
            .iter()
            .find(|p| p.recipient == create_test_account(101))
            .unwrap();
        assert_eq!(nom2_payout.amount, 18 * 10u128.pow(18));
    }

    #[test]
    fn test_apy_calculation() {
        let distributor = RewardDistributor::new(RewardConfig::default());

        // Era reward = 100 ZT, backing = 1000 ZT
        // Era return = 10%
        // Eras per year = 365
        // APY = 10% * 365 = 3650%
        let apy = distributor.calculate_validator_apy(
            100 * 10u128.pow(18),
            1000 * 10u128.pow(18),
            365,
        );

        assert!((apy - 3650.0).abs() < 0.1);

        // With 10% commission, nominator APY = 3650% * 0.9 = 3285%
        let nominator_apy = calculate_nominator_apy(apy, 10);
        assert!((nominator_apy - 3285.0).abs() < 0.1);
    }
}
