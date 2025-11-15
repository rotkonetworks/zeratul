//! Batch Margin Trading Execution
//!
//! This module implements MEV-resistant margin trading using batch execution.
//! All margin orders in a block are executed together at a fair clearing price,
//! similar to Penumbra's batch swap mechanism.

use super::types::*;
use anyhow::{bail, Result};
use serde::{Deserialize, Serialize};
use zeratul_circuit::AccidentalComputerProof;
use std::collections::BTreeMap; // SECURITY FIX: Use BTreeMap for deterministic iteration

/// Side of a margin trade
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Side {
    Long,
    Short,
}

/// Leverage multiplier
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Leverage(pub u8);

impl Leverage {
    pub const fn new(x: u8) -> Option<Self> {
        match x {
            2 | 3 | 5 | 10 | 20 => Some(Leverage(x)),
            _ => None,
        }
    }

    pub fn multiplier(&self) -> u128 {
        self.0 as u128
    }
}

/// A margin trade order (encrypted)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MarginOrder {
    /// Position owner (commitment)
    pub owner: [u8; 32],

    /// Trading pair (base/quote)
    pub base_asset: AssetId,
    pub quote_asset: AssetId,

    /// Side of the trade
    pub side: Side,

    /// Order size in base asset (encrypted)
    pub size: Amount,

    /// Leverage multiplier
    pub leverage: Leverage,

    /// Maximum slippage tolerance (percentage)
    /// CRITICAL MEV PROTECTION: User-configurable, default 1%
    /// Order rejected if execution price differs from oracle by > this amount
    pub max_slippage_percent: u8,

    /// ZK proof of sufficient collateral
    pub collateral_proof: AccidentalComputerProof,

    /// Block when order was submitted
    pub submitted_block: u64,
}

impl MarginOrder {
    /// Calculate maximum acceptable execution price for this order
    ///
    /// For long orders: willing to pay UP TO (oracle_price * (1 + slippage))
    /// For short orders: willing to receive AT LEAST (oracle_price * (1 - slippage))
    pub fn calculate_max_acceptable_price(&self, oracle_price: Price) -> Price {
        match self.side {
            Side::Long => oracle_price.apply_slippage(self.max_slippage_percent, true),
            Side::Short => oracle_price.apply_slippage(self.max_slippage_percent, false),
        }
    }

    /// Validate that execution price is within slippage tolerance
    ///
    /// CRITICAL MEV PROTECTION: Reject order if execution price outside bounds
    pub fn validate_execution_price(
        &self,
        execution_price: Price,
        oracle_price: Price,
    ) -> Result<()> {
        // Calculate actual slippage in basis points (10000 = 100%)
        let slippage_bps = execution_price.percent_diff(&oracle_price);
        let max_slippage_bps = (self.max_slippage_percent as u128) * 100; // Convert percent to bps

        if slippage_bps > max_slippage_bps {
            bail!(
                "Slippage too high: {} bps > {} bps limit (oracle: {}/{}, execution: {}/{})",
                slippage_bps,
                max_slippage_bps,
                oracle_price.numerator,
                oracle_price.denominator,
                execution_price.numerator,
                execution_price.denominator
            );
        }

        Ok(())
    }
}

/// Aggregated batch of margin orders for a trading pair
#[derive(Debug, Clone)]
pub struct MarginBatch {
    /// Trading pair
    pub base_asset: AssetId,
    pub quote_asset: AssetId,

    /// All long orders
    pub long_orders: Vec<MarginOrder>,

    /// All short orders
    pub short_orders: Vec<MarginOrder>,

    /// Total long size (aggregate)
    pub total_long_size: Amount,

    /// Total short size (aggregate)
    pub total_short_size: Amount,

    /// Block height for this batch
    pub block_height: u64,
}

impl MarginBatch {
    pub fn new(base_asset: AssetId, quote_asset: AssetId, block_height: u64) -> Self {
        Self {
            base_asset,
            quote_asset,
            long_orders: Vec::new(),
            short_orders: Vec::new(),
            total_long_size: Amount::ZERO,
            total_short_size: Amount::ZERO,
            block_height,
        }
    }

    /// Add an order to the batch
    pub fn add_order(&mut self, order: MarginOrder) -> Result<()> {
        // Verify trading pair matches
        if order.base_asset != self.base_asset || order.quote_asset != self.quote_asset {
            bail!("order trading pair mismatch");
        }

        match order.side {
            Side::Long => {
                self.total_long_size = self
                    .total_long_size
                    .checked_add(order.size)
                    .ok_or_else(|| anyhow::anyhow!("overflow in long size"))?;
                self.long_orders.push(order);
            }
            Side::Short => {
                self.total_short_size = self
                    .total_short_size
                    .checked_add(order.size)
                    .ok_or_else(|| anyhow::anyhow!("overflow in short size"))?;
                self.short_orders.push(order);
            }
        }

        Ok(())
    }

    /// Calculate the net imbalance
    /// Positive = more longs, Negative = more shorts
    pub fn net_imbalance(&self) -> i128 {
        (self.total_long_size.0 as i128) - (self.total_short_size.0 as i128)
    }

    /// Check if batch is balanced enough to execute
    /// SECURITY FIX: Use checked arithmetic
    pub fn is_executable(&self, max_imbalance_ratio: Ratio) -> bool {
        let total_size = match self.total_long_size.0.checked_add(self.total_short_size.0) {
            Some(size) => size,
            None => return false, // Overflow = not executable
        };

        if total_size == 0 {
            return false;
        }

        let imbalance = self.net_imbalance().unsigned_abs();
        let imbalance_ratio = Ratio {
            numerator: imbalance,
            denominator: total_size,
        };

        imbalance_ratio.lt(&max_imbalance_ratio)
    }
}

/// Result of executing a margin batch
/// SECURITY FIX: Use BTreeMap for deterministic consensus
#[derive(Debug, Clone)]
pub struct BatchExecutionResult {
    /// Fair clearing price for this batch
    pub clearing_price: Price,

    /// Updated positions
    pub position_updates: Vec<PositionUpdate>,

    /// Amount borrowed from pool for leverage (BTreeMap for deterministic iteration)
    pub borrowed_from_pool: BTreeMap<AssetId, Amount>,

    /// Pool utilization after execution (BTreeMap for deterministic iteration)
    pub new_pool_utilization: BTreeMap<AssetId, Ratio>,

    /// Events generated
    pub events: Vec<MarginEvent>,
}

/// Helper function to calculate clearing price based on oracle price and imbalance
pub fn calculate_clearing_price(
    oracle_price: Price,
    net_imbalance: i128,
    total_size: u128,
    max_slippage: Ratio,
) -> Price {
    // Price impact based on imbalance
    // If more longs: price goes up (slippage)
    // If more shorts: price goes down

    if total_size == 0 {
        return oracle_price;
    }

    // Calculate imbalance_ratio = imbalance / total_size as a Ratio
    let imbalance_abs = net_imbalance.unsigned_abs();
    let imbalance_ratio = Ratio {
        numerator: imbalance_abs,
        denominator: total_size,
    };

    // Calculate slippage = imbalance_ratio * max_slippage
    // = (imb_num / imb_denom) * (max_num / max_denom)
    // = (imb_num * max_num) / (imb_denom * max_denom)
    let slippage_numerator = imbalance_ratio
        .numerator
        .checked_mul(max_slippage.numerator)
        .expect("overflow in slippage calculation");
    let slippage_denominator = imbalance_ratio
        .denominator
        .checked_mul(max_slippage.denominator)
        .expect("overflow in slippage calculation");

    // Apply slippage: new_price = oracle_price * (1 +/- slippage)
    // = (oracle_num/oracle_denom) * ((denom +/- num) / denom)
    let positive_slippage = net_imbalance > 0;

    let factor_numerator = if positive_slippage {
        slippage_denominator
            .checked_add(slippage_numerator)
            .expect("overflow in price adjustment")
    } else {
        slippage_denominator.saturating_sub(slippage_numerator)
    };

    Price {
        numerator: oracle_price
            .numerator
            .checked_mul(factor_numerator)
            .expect("overflow in clearing price"),
        denominator: oracle_price
            .denominator
            .checked_mul(slippage_denominator)
            .expect("overflow in clearing price"),
    }
    .normalize()
}

/// Update to a position after batch execution
#[derive(Debug, Clone)]
pub struct PositionUpdate {
    pub owner: [u8; 32],
    pub base_asset: AssetId,
    pub quote_asset: AssetId,
    pub side: Side,
    pub size: Amount,
    pub entry_price: Price,
    pub leverage: Leverage,
    pub borrowed_amount: Amount,
}

/// Events from margin trading
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum MarginEvent {
    OrderExecuted {
        owner: [u8; 32],
        base_asset: AssetId,
        quote_asset: AssetId,
        side: Side,
        size: Amount,
        price: Price,
        leverage: Leverage,
    },
    OrderRejected {
        owner: [u8; 32],
        reason: String,
    },
    PositionOpened {
        owner: [u8; 32],
        position_id: [u8; 32],
        size: Amount,
        leverage: Leverage,
    },
    PositionIncreased {
        owner: [u8; 32],
        position_id: [u8; 32],
        added_size: Amount,
    },
}

/// Execute a batch of margin orders
pub fn execute_margin_batch(
    batch: MarginBatch,
    oracle_price: Price,
    lending_pool: &mut LendingPool,
    positions: &mut BTreeMap<[u8; 32], Position>,
    max_slippage: Ratio,
    current_block: u64,
) -> Result<BatchExecutionResult> {
    // Check if batch is executable
    if !batch.is_executable(Ratio::from_percent(20)) {
        // Max 20% imbalance
        bail!("batch imbalance too high");
    }

    // Calculate fair clearing price
    // SECURITY FIX: Use checked arithmetic
    let total_size = batch
        .total_long_size
        .0
        .checked_add(batch.total_short_size.0)
        .ok_or_else(|| anyhow::anyhow!("Overflow calculating total batch size"))?;

    let clearing_price = calculate_clearing_price(
        oracle_price,
        batch.net_imbalance(),
        total_size,
        max_slippage,
    );

    let mut position_updates = Vec::new();
    let mut borrowed_from_pool = BTreeMap::new();
    let mut events = Vec::new();

    // Execute all long orders at clearing price
    for order in batch.long_orders {
        // CRITICAL MEV PROTECTION: Validate slippage before execution
        if let Err(e) = order.validate_execution_price(clearing_price, oracle_price) {
            // Order rejected due to slippage - emit rejection event
            events.push(MarginEvent::OrderRejected {
                owner: order.owner,
                reason: format!("Slippage protection: {}", e),
            });
            continue;
        }

        let position_update = execute_single_order(
            order.clone(),
            clearing_price,
            lending_pool,
            positions,
            current_block,
            &mut borrowed_from_pool,
        )?;

        events.push(MarginEvent::OrderExecuted {
            owner: order.owner,
            base_asset: order.base_asset,
            quote_asset: order.quote_asset,
            side: order.side,
            size: order.size,
            price: clearing_price,
            leverage: order.leverage,
        });

        position_updates.push(position_update);
    }

    // Execute all short orders at same clearing price
    for order in batch.short_orders {
        // CRITICAL MEV PROTECTION: Validate slippage before execution
        if let Err(e) = order.validate_execution_price(clearing_price, oracle_price) {
            // Order rejected due to slippage - emit rejection event
            events.push(MarginEvent::OrderRejected {
                owner: order.owner,
                reason: format!("Slippage protection: {}", e),
            });
            continue;
        }

        let position_update = execute_single_order(
            order.clone(),
            clearing_price,
            lending_pool,
            positions,
            current_block,
            &mut borrowed_from_pool,
        )?;

        events.push(MarginEvent::OrderExecuted {
            owner: order.owner,
            base_asset: order.base_asset,
            quote_asset: order.quote_asset,
            side: order.side,
            size: order.size,
            price: clearing_price,
            leverage: order.leverage,
        });

        position_updates.push(position_update);
    }

    // Calculate new pool utilization
    let mut new_pool_utilization = BTreeMap::new();
    for (asset_id, _amount) in &borrowed_from_pool {
        if let Some(pool) = lending_pool.get_pool(asset_id) {
            new_pool_utilization.insert(*asset_id, pool.utilization());
        }
    }

    Ok(BatchExecutionResult {
        clearing_price,
        position_updates,
        borrowed_from_pool,
        new_pool_utilization,
        events,
    })
}

/// Execute a single margin order
fn execute_single_order(
    order: MarginOrder,
    clearing_price: Price,
    lending_pool: &mut LendingPool,
    positions: &mut BTreeMap<[u8; 32], Position>,
    current_block: u64,
    borrowed_from_pool: &mut BTreeMap<AssetId, Amount>,
) -> Result<PositionUpdate> {
    // Calculate borrowed amount needed for leverage
    let position_value = Amount(
        clearing_price
            .checked_mul_amount(order.size.0)
            .ok_or_else(|| anyhow::anyhow!("overflow calculating position value"))?,
    );

    // SECURITY FIX: Use checked arithmetic
    // borrowed_amount = position_value * (leverage - 1) / leverage
    let leverage = order.leverage.multiplier();
    let borrowed_amount = Amount(
        position_value
            .0
            .checked_mul(leverage.checked_sub(1).ok_or_else(|| {
                anyhow::anyhow!("Invalid leverage value")
            })?)
            .and_then(|v| v.checked_div(leverage))
            .ok_or_else(|| anyhow::anyhow!("Overflow calculating borrowed amount"))?,
    );

    // Borrow from pool
    let pool = lending_pool
        .get_pool_mut(&order.quote_asset)
        .ok_or_else(|| anyhow::anyhow!("pool not found"))?;

    pool.accrue_interest(current_block);
    pool.total_borrowed = pool
        .total_borrowed
        .checked_add(borrowed_amount)
        .ok_or_else(|| anyhow::anyhow!("overflow in borrow"))?;

    // Track total borrowed
    let current_borrowed = borrowed_from_pool
        .get(&order.quote_asset)
        .copied()
        .unwrap_or(Amount::ZERO);
    borrowed_from_pool.insert(
        order.quote_asset,
        current_borrowed
            .checked_add(borrowed_amount)
            .ok_or_else(|| anyhow::anyhow!("overflow tracking borrowed"))?,
    );

    // Update position
    let position = positions.entry(order.owner).or_insert_with(|| Position::new(order.owner));

    let current_debt = position
        .debt
        .get(&order.quote_asset)
        .copied()
        .unwrap_or(Amount::ZERO);
    position.debt.insert(
        order.quote_asset,
        current_debt
            .checked_add(borrowed_amount)
            .ok_or_else(|| anyhow::anyhow!("overflow in position debt"))?,
    );

    Ok(PositionUpdate {
        owner: order.owner,
        base_asset: order.base_asset,
        quote_asset: order.quote_asset,
        side: order.side,
        size: order.size,
        entry_price: clearing_price,
        leverage: order.leverage,
        borrowed_amount,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_batch_aggregation() {
        let base = AssetId([1; 32]);
        let quote = AssetId([2; 32]);
        let mut batch = MarginBatch::new(base, quote, 100);

        // Add long order
        let long_order = MarginOrder {
            owner: [3; 32],
            base_asset: base,
            quote_asset: quote,
            side: Side::Long,
            size: Amount(1000),
            leverage: Leverage::new(3).unwrap(),
            collateral_proof: unimplemented!(),
            submitted_block: 100,
        };

        batch.add_order(long_order).unwrap();
        assert_eq!(batch.total_long_size.0, 1000);
        assert_eq!(batch.net_imbalance(), 1000);
    }

    #[test]
    fn test_clearing_price_calculation() {
        let oracle_price = Price {
            numerator: 1000,
            denominator: 1,
        };

        // More longs (1000) than shorts (0) → price should increase
        let clearing_price = calculate_clearing_price(
            oracle_price,
            1000, // net imbalance
            1000, // total size
            Ratio::from_percent(5), // 5% max slippage
        );

        // With 100% imbalance and 5% max slippage, price should be ~5% higher
        assert!(clearing_price.gt(&oracle_price));
    }
}
