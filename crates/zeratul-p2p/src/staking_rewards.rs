//! Staking rewards with target ratio (like Penumbra)
//!
//! ## Design:
//!
//! - **Total supply**: 100M ZT initially
//! - **Base inflation**: 2% per year
//! - **Target staking ratio**: 50%
//! - **Reward adjustment**: APY varies based on actual staking ratio
//!
//! ## How it works:
//!
//! ```text
//! If staking ratio = 10%:
//!   - Only 10M ZT staked
//!   - 2M ZT/year inflation → all goes to 10M staked
//!   - APY = 2M / 10M = 20% (high rewards, encourages more staking)
//!
//! If staking ratio = 50% (target):
//!   - 50M ZT staked
//!   - 2M ZT/year inflation → all goes to 50M staked
//!   - APY = 2M / 50M = 4% (equilibrium)
//!
//! If staking ratio = 100%:
//!   - 100M ZT staked
//!   - 2M ZT/year inflation → all goes to 100M staked
//!   - APY = 2M / 100M = 2% (low rewards, encourages unstaking for trading)
//! ```
//!
//! ## Why this prevents centralization:
//!
//! - All validators have same APY (based on global staking ratio, not validator)
//! - No competition for "best validator"
//! - Delegation pools don't offer better rates
//! - Self-balancing: too much staking → lower APY → people unstake

use crate::consensus::BlockNumber;
use serde::{Deserialize, Serialize};

/// Base inflation rate: 2% per year
pub const BASE_INFLATION_BPS: u64 = 200; // 2% = 200 basis points

/// Target staking ratio: 50%
pub const TARGET_STAKING_RATIO_BPS: u64 = 5000; // 50% = 5000 basis points

/// Blocks per year (1 second blocks)
pub const BLOCKS_PER_YEAR: u64 = 31_536_000;

/// Staking rewards state
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StakingRewards {
    /// Current total supply (grows with inflation)
    pub total_supply: u64,

    /// Total staked (bonded) ZT
    pub total_staked: u64,

    /// Last block when rewards were distributed
    pub last_reward_block: BlockNumber,

    /// Accumulated rewards to distribute
    pub pending_rewards: u64,
}

impl StakingRewards {
    /// Create at genesis
    pub fn genesis(initial_supply: u64) -> Self {
        Self {
            total_supply: initial_supply,
            total_staked: 0,
            last_reward_block: 0,
            pending_rewards: 0,
        }
    }

    /// Calculate staking ratio (basis points)
    pub fn staking_ratio_bps(&self) -> u64 {
        if self.total_supply == 0 {
            return 0;
        }

        (self.total_staked * 10000) / self.total_supply
    }

    /// Calculate current APY for stakers (basis points)
    ///
    /// APY = (inflation / staking_ratio)
    ///
    /// Examples:
    /// - 10% staked: APY = 2% / 10% = 20%
    /// - 50% staked: APY = 2% / 50% = 4%
    /// - 100% staked: APY = 2% / 100% = 2%
    pub fn current_apy_bps(&self) -> u64 {
        let staking_ratio = self.staking_ratio_bps();

        if staking_ratio == 0 {
            return 0;
        }

        // APY = base_inflation / staking_ratio
        // = (200 bps) / (staking_ratio / 10000)
        // = 200 * 10000 / staking_ratio
        (BASE_INFLATION_BPS * 10000) / staking_ratio
    }

    /// Update staked amount
    pub fn set_total_staked(&mut self, total_staked: u64) {
        self.total_staked = total_staked;
    }

    /// Mint new tokens (inflation) and add to rewards pool
    ///
    /// Returns amount minted
    pub fn mint_inflation(
        &mut self,
        current_block: BlockNumber,
    ) -> u64 {
        if current_block <= self.last_reward_block {
            return 0;
        }

        let blocks_elapsed = current_block - self.last_reward_block;

        // Calculate inflation for this period
        // new_tokens = total_supply * (2% * blocks_elapsed / blocks_per_year)
        let inflation_numerator = self.total_supply
            .saturating_mul(BASE_INFLATION_BPS)
            .saturating_mul(blocks_elapsed);

        let new_tokens = inflation_numerator / (10000 * BLOCKS_PER_YEAR);

        if new_tokens == 0 {
            self.last_reward_block = current_block;
            return 0;
        }

        // Mint new tokens
        self.total_supply = self.total_supply.saturating_add(new_tokens);
        self.pending_rewards = self.pending_rewards.saturating_add(new_tokens);
        self.last_reward_block = current_block;

        new_tokens
    }

    /// Distribute pending rewards to bonded holders
    ///
    /// Returns amount distributed
    pub fn distribute_rewards(&mut self) -> u64 {
        let amount = self.pending_rewards;
        self.pending_rewards = 0;
        amount
    }

    /// Get supply stats
    pub fn stats(&self) -> RewardStats {
        RewardStats {
            total_supply: self.total_supply,
            total_staked: self.total_staked,
            staking_ratio_bps: self.staking_ratio_bps(),
            current_apy_bps: self.current_apy_bps(),
            pending_rewards: self.pending_rewards,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RewardStats {
    pub total_supply: u64,
    pub total_staked: u64,
    pub staking_ratio_bps: u64,
    pub current_apy_bps: u64,
    pub pending_rewards: u64,
}

impl RewardStats {
    /// Get staking ratio as percentage
    pub fn staking_ratio_pct(&self) -> f64 {
        self.staking_ratio_bps as f64 / 100.0
    }

    /// Get APY as percentage
    pub fn apy_pct(&self) -> f64 {
        self.current_apy_bps as f64 / 100.0
    }

    /// Get inflation rate as percentage
    pub fn inflation_rate_pct(&self) -> f64 {
        BASE_INFLATION_BPS as f64 / 100.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_target_ratio_gives_4pct_apy() {
        let mut rewards = StakingRewards::genesis(100_000_000);

        // 50% staked (target ratio)
        rewards.set_total_staked(50_000_000);

        let apy = rewards.current_apy_bps();

        // APY should be 2% / 50% = 4%
        assert_eq!(apy, 400); // 4% = 400 bps

        println!("At target 50% staking: APY = {}%", apy as f64 / 100.0);
    }

    #[test]
    fn test_low_staking_gives_high_apy() {
        let mut rewards = StakingRewards::genesis(100_000_000);

        // Only 10% staked (low)
        rewards.set_total_staked(10_000_000);

        let apy = rewards.current_apy_bps();

        // APY should be 2% / 10% = 20%
        assert_eq!(apy, 2000); // 20% = 2000 bps

        println!("At low 10% staking: APY = {}%", apy as f64 / 100.0);
    }

    #[test]
    fn test_high_staking_gives_low_apy() {
        let mut rewards = StakingRewards::genesis(100_000_000);

        // 100% staked (high)
        rewards.set_total_staked(100_000_000);

        let apy = rewards.current_apy_bps();

        // APY should be 2% / 100% = 2%
        assert_eq!(apy, 200); // 2% = 200 bps

        println!("At high 100% staking: APY = {}%", apy as f64 / 100.0);
    }

    #[test]
    fn test_inflation_over_one_year() {
        let mut rewards = StakingRewards::genesis(100_000_000);
        rewards.set_total_staked(50_000_000); // 50% staked

        // Simulate 1 year (31.5M blocks)
        let minted = rewards.mint_inflation(BLOCKS_PER_YEAR);

        // Should mint ~2% of total supply = 2M tokens
        let expected = 2_000_000;
        let tolerance = 20_000; // 1% tolerance

        assert!(
            minted > expected - tolerance && minted < expected + tolerance,
            "Expected ~{}, got {}",
            expected,
            minted
        );

        // Total supply should be ~102M
        assert!(rewards.total_supply > 101_900_000 && rewards.total_supply < 102_100_000);

        println!("After 1 year:");
        println!("  Minted: {} ZT", minted);
        println!("  Total supply: {} ZT", rewards.total_supply);
        println!("  APY: {}%", rewards.current_apy_bps() as f64 / 100.0);
    }

    #[test]
    fn test_stats() {
        let mut rewards = StakingRewards::genesis(100_000_000);
        rewards.set_total_staked(50_000_000);

        let stats = rewards.stats();

        println!("Staking stats:");
        println!("  Total supply: {} ZT", stats.total_supply);
        println!("  Total staked: {} ZT", stats.total_staked);
        println!("  Staking ratio: {}%", stats.staking_ratio_pct());
        println!("  Current APY: {}%", stats.apy_pct());
        println!("  Inflation rate: {}%", stats.inflation_rate_pct());

        assert_eq!(stats.staking_ratio_pct(), 50.0);
        assert_eq!(stats.apy_pct(), 4.0);
        assert_eq!(stats.inflation_rate_pct(), 2.0);
    }
}
