//! Core types for the lending pool
//!
//! This module defines the privacy-preserving lending pool that enables
//! margin trading on Penumbra assets.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap; // SECURITY FIX: Use BTreeMap for deterministic iteration

/// Asset identifier (maps to Penumbra asset IDs)
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct AssetId(pub [u8; 32]);

/// Amount of an asset (encrypted in practice)
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Amount(pub u128);

impl Amount {
    pub const ZERO: Self = Amount(0);

    pub fn checked_add(self, other: Amount) -> Option<Amount> {
        self.0.checked_add(other.0).map(Amount)
    }

    pub fn checked_sub(self, other: Amount) -> Option<Amount> {
        self.0.checked_sub(other.0).map(Amount)
    }

    pub fn checked_mul(self, ratio: Ratio) -> Option<Amount> {
        let numerator = self.0.checked_mul(ratio.numerator)?;
        Some(Amount(numerator / ratio.denominator))
    }
}

/// Ratio type for percentages, rates, etc.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Ratio {
    pub numerator: u128,
    pub denominator: u128,
}

impl Ratio {
    pub const ZERO: Self = Ratio {
        numerator: 0,
        denominator: 1,
    };

    pub const ONE: Self = Ratio {
        numerator: 1,
        denominator: 1,
    };

    /// Create a ratio from a percentage (e.g., 75 for 75%)
    pub fn from_percent(percent: u128) -> Self {
        Ratio {
            numerator: percent,
            denominator: 100,
        }
    }

    /// Create a ratio from basis points (e.g., 7500 for 75%)
    pub fn from_bps(bps: u128) -> Self {
        Ratio {
            numerator: bps,
            denominator: 10_000,
        }
    }

    pub fn checked_add(self, other: Ratio) -> Option<Ratio> {
        let common_denom = self.denominator.checked_mul(other.denominator)?;
        let self_num = self.numerator.checked_mul(other.denominator)?;
        let other_num = other.numerator.checked_mul(self.denominator)?;
        let total_num = self_num.checked_add(other_num)?;

        Some(Ratio {
            numerator: total_num,
            denominator: common_denom,
        })
    }

    /// Check if this ratio is less than another
    pub fn lt(&self, other: &Ratio) -> bool {
        self.numerator * other.denominator < other.numerator * self.denominator
    }

    /// Check if this ratio is greater than or equal to another
    pub fn ge(&self, other: &Ratio) -> bool {
        self.numerator * other.denominator >= other.numerator * self.denominator
    }
}

/// Price (quote per base asset)
///
/// Represents exchange rate between two assets as a rational number.
/// Using rational arithmetic ensures consensus-critical determinism.
///
/// Example: Price { numerator: 105, denominator: 100 } means 1.05 (1 base = 1.05 quote)
///
/// SECURITY FIX: Replaced f64 with rational arithmetic to prevent consensus divergence.
/// f64 arithmetic is non-deterministic across architectures and can cause validator forks.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Price {
    pub numerator: u128,
    pub denominator: u128,
}

impl Price {
    /// Create a price from integer and fractional parts
    /// Example: Price::from_parts(1, 5, 100) = 1.05
    pub fn from_parts(whole: u128, frac_num: u128, frac_denom: u128) -> Self {
        let numerator = whole
            .checked_mul(frac_denom)
            .and_then(|w| w.checked_add(frac_num))
            .expect("overflow in price construction");
        Self {
            numerator,
            denominator: frac_denom,
        }
    }

    /// Create a price from a ratio (1:1 conversion)
    pub fn from_ratio(ratio: Ratio) -> Self {
        Self {
            numerator: ratio.numerator,
            denominator: ratio.denominator,
        }
    }

    /// Create a price representing 1:1 exchange rate
    pub const ONE: Self = Self {
        numerator: 1,
        denominator: 1,
    };

    /// Multiply by a scalar value (checked)
    pub fn checked_mul_amount(&self, amount: u128) -> Option<u128> {
        amount
            .checked_mul(self.numerator)?
            .checked_div(self.denominator)
    }

    /// Apply slippage percentage to price
    /// positive_slippage: if true, increase price; if false, decrease price
    /// slippage_percent: e.g., 1 for 1%, 5 for 5%
    pub fn apply_slippage(&self, slippage_percent: u8, positive_slippage: bool) -> Self {
        // Calculate new_price = price * (1 +/- slippage/100)
        let slippage_factor = if positive_slippage {
            // (100 + slippage) / 100
            Ratio {
                numerator: 100 + slippage_percent as u128,
                denominator: 100,
            }
        } else {
            // (100 - slippage) / 100
            Ratio {
                numerator: 100u128.saturating_sub(slippage_percent as u128),
                denominator: 100,
            }
        };

        // new_price = price * factor = (num/denom) * (factor_num/factor_denom)
        // = (num * factor_num) / (denom * factor_denom)
        Self {
            numerator: self
                .numerator
                .checked_mul(slippage_factor.numerator)
                .expect("overflow in slippage calculation"),
            denominator: self
                .denominator
                .checked_mul(slippage_factor.denominator)
                .expect("overflow in slippage calculation"),
        }
    }

    /// Calculate percentage difference between two prices
    /// Returns basis points (10000 = 100%)
    pub fn percent_diff(&self, other: &Price) -> u128 {
        // |self - other| / other * 10000
        // = |self.num/self.denom - other.num/other.denom| / (other.num/other.denom) * 10000
        // = |self.num * other.denom - other.num * self.denom| / (other.num * self.denom) * 10000

        let self_scaled = self
            .numerator
            .checked_mul(other.denominator)
            .expect("overflow in price comparison");
        let other_scaled = other
            .numerator
            .checked_mul(self.denominator)
            .expect("overflow in price comparison");

        let diff = if self_scaled > other_scaled {
            self_scaled - other_scaled
        } else {
            other_scaled - self_scaled
        };

        let denominator = other
            .numerator
            .checked_mul(self.denominator)
            .expect("overflow in price comparison");

        // diff / denominator * 10000
        diff.checked_mul(10000)
            .expect("overflow in percent calculation")
            .checked_div(denominator)
            .unwrap_or(u128::MAX)
    }

    /// Check if this price is less than another
    pub fn lt(&self, other: &Price) -> bool {
        // self.num/self.denom < other.num/other.denom
        // self.num * other.denom < other.num * self.denom
        self.numerator
            .checked_mul(other.denominator)
            .expect("overflow in price comparison")
            < other
                .numerator
                .checked_mul(self.denominator)
                .expect("overflow in price comparison")
    }

    /// Check if this price is greater than another
    pub fn gt(&self, other: &Price) -> bool {
        other.lt(self)
    }

    /// Normalize the price by dividing both numerator and denominator by their GCD
    pub fn normalize(&self) -> Self {
        fn gcd(mut a: u128, mut b: u128) -> u128 {
            while b != 0 {
                let temp = b;
                b = a % b;
                a = temp;
            }
            a
        }

        let divisor = gcd(self.numerator, self.denominator);
        Self {
            numerator: self.numerator / divisor,
            denominator: self.denominator / divisor,
        }
    }
}

/// Interest rate (annual percentage rate)
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct InterestRate {
    /// APR in basis points (e.g., 500 = 5%)
    pub bps: u128,
}

impl InterestRate {
    pub const ZERO: Self = InterestRate { bps: 0 };

    /// Calculate interest for a given amount over a number of blocks
    pub fn calculate_interest(&self, principal: Amount, blocks: u64) -> Amount {
        // Assume ~6 second blocks, so ~5,256,000 blocks per year
        const BLOCKS_PER_YEAR: u128 = 5_256_000;

        let interest_per_block = Ratio {
            numerator: self.bps * principal.0,
            denominator: 10_000 * BLOCKS_PER_YEAR,
        };

        let total_interest = interest_per_block
            .numerator
            .checked_mul(blocks as u128)
            .expect("overflow in interest calculation")
            / interest_per_block.denominator;

        Amount(total_interest)
    }
}

/// Per-asset lending pool state
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PoolState {
    /// Asset being pooled
    pub asset_id: AssetId,

    /// Total amount supplied to the pool (encrypted commitment)
    pub total_supplied: Amount,

    /// Total amount borrowed from the pool (encrypted commitment)
    pub total_borrowed: Amount,

    /// Last block when interest was accrued
    pub last_update_block: u64,

    /// Interest rate parameters
    pub params: PoolParams,
}

impl PoolState {
    /// Calculate current utilization ratio
    pub fn utilization(&self) -> Ratio {
        if self.total_supplied == Amount::ZERO {
            return Ratio::ZERO;
        }

        Ratio {
            numerator: self.total_borrowed.0,
            denominator: self.total_supplied.0,
        }
    }

    /// Calculate current borrow rate based on utilization
    pub fn borrow_rate(&self) -> InterestRate {
        let util = self.utilization();

        // Two-slope interest rate model (like Aave)
        let optimal_util = self.params.optimal_utilization;

        if util.lt(&optimal_util) {
            // Below optimal: linear interpolation from base to optimal rate
            let slope = Ratio {
                numerator: util.numerator * self.params.optimal_rate.bps,
                denominator: util.denominator * optimal_util.numerator,
            };

            InterestRate {
                bps: self.params.base_rate.bps
                    + (slope.numerator / slope.denominator),
            }
        } else {
            // Above optimal: steep slope to discourage over-utilization
            let excess_util = Ratio {
                numerator: util.numerator - optimal_util.numerator,
                denominator: util.denominator,
            };

            let excess_slope = Ratio {
                numerator: excess_util.numerator * self.params.max_rate.bps,
                denominator: (Ratio::ONE.numerator - optimal_util.numerator),
            };

            InterestRate {
                bps: self.params.optimal_rate.bps + (excess_slope.numerator / excess_slope.denominator),
            }
        }
    }

    /// Calculate current supply rate (what lenders earn)
    pub fn supply_rate(&self) -> InterestRate {
        let borrow_rate = self.borrow_rate();
        let util = self.utilization();

        // Supply rate = utilization * borrow_rate * (1 - reserve_factor)
        let rate_after_reserves = Ratio {
            numerator: borrow_rate.bps * (10_000 - self.params.reserve_factor.bps as u128),
            denominator: 10_000,
        };

        let supply_rate = Ratio {
            numerator: util.numerator * rate_after_reserves.numerator,
            denominator: util.denominator * rate_after_reserves.denominator,
        };

        InterestRate {
            bps: supply_rate.numerator / supply_rate.denominator,
        }
    }

    /// Accrue interest for the time elapsed since last update
    pub fn accrue_interest(&mut self, current_block: u64) {
        let blocks_elapsed = current_block.saturating_sub(self.last_update_block);
        if blocks_elapsed == 0 {
            return;
        }

        let borrow_rate = self.borrow_rate();
        let interest = borrow_rate.calculate_interest(self.total_borrowed, blocks_elapsed);

        // Add interest to total borrowed
        self.total_borrowed = self
            .total_borrowed
            .checked_add(interest)
            .expect("overflow in interest accrual");

        self.last_update_block = current_block;
    }
}

/// Parameters for a lending pool
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PoolParams {
    /// Base interest rate when utilization is 0
    pub base_rate: InterestRate,

    /// Optimal utilization ratio (e.g., 80%)
    pub optimal_utilization: Ratio,

    /// Interest rate at optimal utilization
    pub optimal_rate: InterestRate,

    /// Maximum interest rate (when utilization is 100%)
    pub max_rate: InterestRate,

    /// Reserve factor (portion of interest going to protocol reserves)
    pub reserve_factor: InterestRate,

    /// Collateral factor (how much can be borrowed against collateral)
    /// e.g., 75% means 1000 UM collateral can borrow 750 UM worth
    pub collateral_factor: Ratio,

    /// Liquidation threshold (when position becomes liquidatable)
    /// e.g., 80% means health factor must stay above 0.8
    pub liquidation_threshold: Ratio,

    /// Liquidation penalty (bonus for liquidators)
    /// e.g., 5% means liquidator gets 5% discount
    pub liquidation_penalty: Ratio,
}

impl Default for PoolParams {
    fn default() -> Self {
        Self {
            // Conservative defaults
            base_rate: InterestRate { bps: 200 },      // 2% base rate
            optimal_utilization: Ratio::from_percent(80), // 80% optimal
            optimal_rate: InterestRate { bps: 1000 },  // 10% at optimal
            max_rate: InterestRate { bps: 5000 },      // 50% at 100% util
            reserve_factor: InterestRate { bps: 1000 }, // 10% to reserves
            collateral_factor: Ratio::from_percent(75), // 75% LTV
            liquidation_threshold: Ratio::from_percent(80), // 80% liquidation
            liquidation_penalty: Ratio::from_percent(5), // 5% penalty
        }
    }
}

/// Multi-asset lending pool
/// SECURITY FIX: Use BTreeMap for deterministic consensus
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LendingPool {
    /// Per-asset pool states (BTreeMap for deterministic iteration)
    pub pools: BTreeMap<AssetId, PoolState>,

    /// Global reserves (from liquidation penalties and fees)
    pub reserves: BTreeMap<AssetId, Amount>,
}

impl LendingPool {
    pub fn new() -> Self {
        Self {
            pools: BTreeMap::new(),
            reserves: BTreeMap::new(),
        }
    }

    /// Initialize a new asset pool
    pub fn add_pool(&mut self, asset_id: AssetId, params: PoolParams) {
        self.pools.insert(
            asset_id,
            PoolState {
                asset_id,
                total_supplied: Amount::ZERO,
                total_borrowed: Amount::ZERO,
                last_update_block: 0,
                params,
            },
        );
        self.reserves.insert(asset_id, Amount::ZERO);
    }

    /// Get a pool by asset ID
    pub fn get_pool(&self, asset_id: &AssetId) -> Option<&PoolState> {
        self.pools.get(asset_id)
    }

    /// Get a mutable pool by asset ID
    pub fn get_pool_mut(&mut self, asset_id: &AssetId) -> Option<&mut PoolState> {
        self.pools.get_mut(asset_id)
    }

    /// Accrue interest for all pools
    pub fn accrue_all(&mut self, current_block: u64) {
        for pool in self.pools.values_mut() {
            pool.accrue_interest(current_block);
        }
    }

    /// Get total value locked (TVL) in pool for a specific asset
    pub fn get_tvl(&self, asset_id: &AssetId) -> Amount {
        self.pools
            .get(asset_id)
            .map(|p| p.total_supplied)
            .unwrap_or(Amount::ZERO)
    }

    /// Get total borrowed for a specific asset
    pub fn get_total_borrowed(&self, asset_id: &AssetId) -> Amount {
        self.pools
            .get(asset_id)
            .map(|p| p.total_borrowed)
            .unwrap_or(Amount::ZERO)
    }
}

/// Risk management configuration
///
/// HARDENING: Protects against position size concentration and systemic risk
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RiskManagementConfig {
    /// Maximum position size as percentage of pool TVL
    /// HARDENING: Prevents whale positions from destabilizing protocol
    /// Default: 10% (position can't exceed 10% of pool size)
    pub max_position_tvl_percent: u8,

    /// Maximum notional exposure per user across all positions
    /// HARDENING: Limits total risk from single user
    /// Default: 1,000,000 (in base currency units)
    pub max_notional_per_user: u128,

    /// Leverage tiers: higher leverage = lower max size
    /// HARDENING: Prevents large highly-leveraged positions
    pub leverage_tiers: Vec<LeverageTier>,

    /// Circuit breaker: max liquidations per block (as % of active positions)
    /// HARDENING: Pause trading if too many liquidations (potential cascade)
    /// Default: 20% (if >20% of positions liquidated, pause)
    pub circuit_breaker_liquidation_threshold_percent: u8,

    /// Circuit breaker: cooldown period after trigger (blocks)
    /// HARDENING: Time to assess systemic risk before resuming
    /// Default: 10 blocks (20 seconds)
    pub circuit_breaker_cooldown_blocks: u64,

    /// Maximum borrow per asset as % of pool size
    /// HARDENING: Prevents pool drain
    /// Default: 80% (can't borrow >80% of pool)
    pub max_borrow_utilization_percent: u8,
}

/// Leverage tier with size limits
///
/// HARDENING: Higher leverage positions have lower max sizes
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LeverageTier {
    /// Maximum leverage for this tier (e.g., 5 = 5x)
    pub max_leverage: u8,

    /// Maximum position size for this tier (% of TVL)
    pub max_size_tvl_percent: u8,
}

impl Default for RiskManagementConfig {
    fn default() -> Self {
        Self {
            max_position_tvl_percent: 10,              // 10% max position size
            max_notional_per_user: 1_000_000_000_000,  // 1M units max per user
            leverage_tiers: vec![
                LeverageTier { max_leverage: 2,  max_size_tvl_percent: 10 },  // 2x: 10% max
                LeverageTier { max_leverage: 5,  max_size_tvl_percent: 5 },   // 5x: 5% max
                LeverageTier { max_leverage: 10, max_size_tvl_percent: 2 },   // 10x: 2% max
                LeverageTier { max_leverage: 20, max_size_tvl_percent: 1 },   // 20x: 1% max
            ],
            circuit_breaker_liquidation_threshold_percent: 20,  // 20% liquidation threshold
            circuit_breaker_cooldown_blocks: 10,                // 10 blocks (20s) cooldown
            max_borrow_utilization_percent: 80,                 // 80% max utilization
        }
    }
}

impl RiskManagementConfig {
    /// Validate position size against limits
    ///
    /// HARDENING: Prevents excessive position concentration
    pub fn validate_position_size(
        &self,
        position_size: Amount,
        leverage: u8,
        pool_tvl: Amount,
    ) -> Result<(), String> {
        // Get max size for this leverage tier
        let max_size_percent = self.get_max_size_for_leverage(leverage);

        // SECURITY FIX: Use checked arithmetic
        let max_allowed_size = pool_tvl
            .0
            .checked_mul(max_size_percent as u128)
            .and_then(|v| v.checked_div(100))
            .ok_or_else(|| {
                "Overflow calculating max position size".to_string()
            })?;

        if position_size.0 > max_allowed_size {
            return Err(format!(
                "Position too large: {} > {} max ({}% of TVL for {}x leverage)",
                position_size.0, max_allowed_size, max_size_percent, leverage
            ));
        }

        Ok(())
    }

    /// Get maximum position size for given leverage
    fn get_max_size_for_leverage(&self, leverage: u8) -> u8 {
        // Find applicable tier
        for tier in &self.leverage_tiers {
            if leverage <= tier.max_leverage {
                return tier.max_size_tvl_percent;
            }
        }

        // If no tier found, use minimum from last tier
        self.leverage_tiers
            .last()
            .map(|t| t.max_size_tvl_percent)
            .unwrap_or(1) // Default to 1% if no tiers configured
    }

    /// Check if borrow would exceed utilization limits
    ///
    /// HARDENING: Prevents pool drain
    /// SECURITY FIX: Use checked arithmetic
    pub fn validate_borrow_utilization(
        &self,
        new_borrow: Amount,
        total_supplied: Amount,
        total_borrowed: Amount,
    ) -> Result<(), String> {
        let new_total_borrowed = total_borrowed
            .0
            .checked_add(new_borrow.0)
            .ok_or_else(|| "Overflow calculating new total borrowed".to_string())?;

        let utilization_percent = if total_supplied.0 > 0 {
            new_total_borrowed
                .checked_mul(100)
                .and_then(|v| v.checked_div(total_supplied.0))
                .ok_or_else(|| "Overflow calculating utilization".to_string())?
        } else {
            100 // No supply = 100% utilization
        };

        if utilization_percent > self.max_borrow_utilization_percent as u128 {
            return Err(format!(
                "Borrow would exceed utilization limit: {}% > {}%",
                utilization_percent, self.max_borrow_utilization_percent
            ));
        }

        Ok(())
    }

    /// Check if circuit breaker should trigger
    ///
    /// HARDENING: Detects liquidation cascades
    pub fn should_trigger_circuit_breaker(
        &self,
        num_liquidated: u32,
        total_positions: u32,
    ) -> bool {
        if total_positions == 0 {
            return false;
        }

        let liquidation_rate = (num_liquidated * 100) / total_positions;
        liquidation_rate > self.circuit_breaker_liquidation_threshold_percent as u32
    }
}

/// A user's position (encrypted in NOMT)
/// SECURITY FIX: Use BTreeMap for deterministic consensus
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Position {
    /// Position owner (commitment hash)
    pub owner: [u8; 32],

    /// Collateral assets and amounts (encrypted, BTreeMap for deterministic iteration)
    pub collateral: BTreeMap<AssetId, Amount>,

    /// Borrowed assets and amounts (encrypted, BTreeMap for deterministic iteration)
    pub debt: BTreeMap<AssetId, Amount>,

    /// Block when position was last updated
    pub last_update_block: u64,
}

impl Position {
    pub fn new(owner: [u8; 32]) -> Self {
        Self {
            owner,
            collateral: BTreeMap::new(),
            debt: BTreeMap::new(),
            last_update_block: 0,
        }
    }

    /// Calculate health factor of this position
    /// health = collateral_value / debt_value
    /// health < 1.0 means position is liquidatable
    pub fn health_factor(
        &self,
        pool: &LendingPool,
        oracle_prices: &BTreeMap<AssetId, Amount>,
    ) -> Option<Ratio> {
        let mut total_collateral_value = 0u128;
        let mut total_debt_value = 0u128;

        // Calculate collateral value (with collateral factor applied)
        for (asset_id, amount) in &self.collateral {
            let price = oracle_prices.get(asset_id)?;
            let pool_state = pool.get_pool(asset_id)?;
            let collateral_factor = pool_state.params.collateral_factor;

            let value = amount.0.checked_mul(price.0)?;
            let adjusted_value = value
                .checked_mul(collateral_factor.numerator)?
                .checked_div(collateral_factor.denominator)?;

            total_collateral_value = total_collateral_value.checked_add(adjusted_value)?;
        }

        // Calculate debt value
        for (asset_id, amount) in &self.debt {
            let price = oracle_prices.get(asset_id)?;
            let value = amount.0.checked_mul(price.0)?;
            total_debt_value = total_debt_value.checked_add(value)?;
        }

        if total_debt_value == 0 {
            return Some(Ratio {
                numerator: u128::MAX,
                denominator: 1,
            });
        }

        Some(Ratio {
            numerator: total_collateral_value,
            denominator: total_debt_value,
        })
    }

    /// Check if position is liquidatable
    pub fn is_liquidatable(
        &self,
        pool: &LendingPool,
        oracle_prices: &BTreeMap<AssetId, Amount>,
    ) -> bool {
        if let Some(health) = self.health_factor(pool, oracle_prices) {
            health.lt(&Ratio::ONE)
        } else {
            false
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_ratio_comparisons() {
        let half = Ratio::from_percent(50);
        let three_quarters = Ratio::from_percent(75);
        let one = Ratio::ONE;

        assert!(half.lt(&three_quarters));
        assert!(three_quarters.lt(&one));
        assert!(one.ge(&three_quarters));
    }

    #[test]
    fn test_utilization() {
        let mut pool = PoolState {
            asset_id: AssetId([0; 32]),
            total_supplied: Amount(1000),
            total_borrowed: Amount(800),
            last_update_block: 0,
            params: PoolParams::default(),
        };

        let util = pool.utilization();
        assert_eq!(util.numerator, 800);
        assert_eq!(util.denominator, 1000);
        // 80% utilization
    }

    #[test]
    fn test_interest_calculation() {
        let rate = InterestRate { bps: 1000 }; // 10% APR
        let principal = Amount(1_000_000);

        // ~1 year worth of blocks
        let interest = rate.calculate_interest(principal, 5_256_000);

        // Should be approximately 10% of principal
        assert!(interest.0 > 90_000 && interest.0 < 110_000);
    }

    #[test]
    fn test_health_factor() {
        let mut pool = LendingPool::new();
        let asset_id = AssetId([1; 32]);

        pool.add_pool(asset_id, PoolParams::default());

        let mut position = Position::new([2; 32]);
        position.collateral.insert(asset_id, Amount(1000));
        position.debt.insert(asset_id, Amount(600));

        let mut prices = BTreeMap::new();
        prices.insert(asset_id, Amount(1)); // 1:1 price

        let health = position.health_factor(&pool, &prices).unwrap();

        // With 75% collateral factor:
        // Adjusted collateral = 1000 * 0.75 = 750
        // Debt = 600
        // Health = 750 / 600 = 1.25
        assert_eq!(health.numerator, 750);
        assert_eq!(health.denominator, 600);
        assert!(!position.is_liquidatable(&pool, &prices));
    }
}
