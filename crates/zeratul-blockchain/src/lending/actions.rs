//! Actions for interacting with the lending pool
//!
//! Each action is privacy-preserving with ZK proofs using AccidentalComputer

use super::types::*;
use anyhow::{bail, Result};
use serde::{Deserialize, Serialize};
use zeratul_circuit::AccidentalComputerProof;

/// Action types for the lending pool
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum LendingAction {
    /// Supply assets to the lending pool
    Supply {
        asset_id: AssetId,
        amount: Amount,
        proof: AccidentalComputerProof,
    },

    /// Withdraw assets from the lending pool
    Withdraw {
        asset_id: AssetId,
        amount: Amount,
        proof: AccidentalComputerProof,
    },

    /// Borrow assets from the lending pool
    Borrow {
        asset_id: AssetId,
        amount: Amount,
        collateral_proof: AccidentalComputerProof,
    },

    /// Repay borrowed assets
    Repay {
        asset_id: AssetId,
        amount: Amount,
        proof: AccidentalComputerProof,
    },

    /// Add collateral to a position
    AddCollateral {
        asset_id: AssetId,
        amount: Amount,
        proof: AccidentalComputerProof,
    },

    /// Remove collateral from a position
    RemoveCollateral {
        asset_id: AssetId,
        amount: Amount,
        health_proof: AccidentalComputerProof,
    },
}

/// Result of executing a lending action
#[derive(Debug, Clone)]
pub struct ActionResult {
    /// Updated pool state
    pub pool_state: PoolState,

    /// Updated user position (if applicable)
    pub position: Option<Position>,

    /// Events generated
    pub events: Vec<LendingEvent>,
}

/// Events emitted by lending actions
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum LendingEvent {
    Supplied {
        user: [u8; 32],
        asset_id: AssetId,
        amount: Amount,
    },
    Withdrawn {
        user: [u8; 32],
        asset_id: AssetId,
        amount: Amount,
    },
    Borrowed {
        user: [u8; 32],
        asset_id: AssetId,
        amount: Amount,
    },
    Repaid {
        user: [u8; 32],
        asset_id: AssetId,
        amount: Amount,
    },
    CollateralAdded {
        user: [u8; 32],
        asset_id: AssetId,
        amount: Amount,
    },
    CollateralRemoved {
        user: [u8; 32],
        asset_id: AssetId,
        amount: Amount,
    },
}

/// Execute a supply action
pub fn execute_supply(
    pool: &mut PoolState,
    user: [u8; 32],
    amount: Amount,
    current_block: u64,
) -> Result<LendingEvent> {
    // Accrue interest before modifying state
    pool.accrue_interest(current_block);

    // Add to pool
    pool.total_supplied = pool
        .total_supplied
        .checked_add(amount)
        .ok_or_else(|| anyhow::anyhow!("overflow in supply"))?;

    Ok(LendingEvent::Supplied {
        user,
        asset_id: pool.asset_id,
        amount,
    })
}

/// Execute a withdraw action
pub fn execute_withdraw(
    pool: &mut PoolState,
    user: [u8; 32],
    amount: Amount,
    current_block: u64,
) -> Result<LendingEvent> {
    // Accrue interest before modifying state
    pool.accrue_interest(current_block);

    // Check available liquidity
    let available = pool
        .total_supplied
        .checked_sub(pool.total_borrowed)
        .ok_or_else(|| anyhow::anyhow!("pool is over-borrowed"))?;

    if amount.0 > available.0 {
        bail!("insufficient liquidity in pool");
    }

    // Subtract from pool
    pool.total_supplied = pool
        .total_supplied
        .checked_sub(amount)
        .ok_or_else(|| anyhow::anyhow!("insufficient supply"))?;

    Ok(LendingEvent::Withdrawn {
        user,
        asset_id: pool.asset_id,
        amount,
    })
}

/// Execute a borrow action
pub fn execute_borrow(
    pool: &mut PoolState,
    position: &mut Position,
    asset_id: AssetId,
    amount: Amount,
    current_block: u64,
    oracle_prices: &std::collections::HashMap<AssetId, Amount>,
    lending_pool: &LendingPool,
) -> Result<LendingEvent> {
    // Accrue interest before modifying state
    pool.accrue_interest(current_block);

    // Check available liquidity
    let available = pool
        .total_supplied
        .checked_sub(pool.total_borrowed)
        .ok_or_else(|| anyhow::anyhow!("pool is over-borrowed"))?;

    if amount.0 > available.0 {
        bail!("insufficient liquidity in pool");
    }

    // Add to position debt
    let current_debt = position.debt.get(&asset_id).copied().unwrap_or(Amount::ZERO);
    let new_debt = current_debt
        .checked_add(amount)
        .ok_or_else(|| anyhow::anyhow!("overflow in debt"))?;
    position.debt.insert(asset_id, new_debt);
    position.last_update_block = current_block;

    // Check health factor after borrow
    let health = position
        .health_factor(lending_pool, oracle_prices)
        .ok_or_else(|| anyhow::anyhow!("failed to calculate health factor"))?;

    if health.lt(&Ratio::ONE) {
        bail!("borrow would make position undercollateralized");
    }

    // Update pool
    pool.total_borrowed = pool
        .total_borrowed
        .checked_add(amount)
        .ok_or_else(|| anyhow::anyhow!("overflow in borrow"))?;

    Ok(LendingEvent::Borrowed {
        user: position.owner,
        asset_id,
        amount,
    })
}

/// Execute a repay action
pub fn execute_repay(
    pool: &mut PoolState,
    position: &mut Position,
    asset_id: AssetId,
    amount: Amount,
    current_block: u64,
) -> Result<LendingEvent> {
    // Accrue interest before modifying state
    pool.accrue_interest(current_block);

    // Get current debt
    let current_debt = position
        .debt
        .get(&asset_id)
        .copied()
        .ok_or_else(|| anyhow::anyhow!("no debt for this asset"))?;

    // Can't repay more than debt
    let repay_amount = if amount.0 > current_debt.0 {
        current_debt
    } else {
        amount
    };

    // Update position
    let new_debt = current_debt
        .checked_sub(repay_amount)
        .ok_or_else(|| anyhow::anyhow!("underflow in repay"))?;

    if new_debt == Amount::ZERO {
        position.debt.remove(&asset_id);
    } else {
        position.debt.insert(asset_id, new_debt);
    }
    position.last_update_block = current_block;

    // Update pool
    pool.total_borrowed = pool
        .total_borrowed
        .checked_sub(repay_amount)
        .ok_or_else(|| anyhow::anyhow!("underflow in pool borrow"))?;

    Ok(LendingEvent::Repaid {
        user: position.owner,
        asset_id,
        amount: repay_amount,
    })
}

/// Execute add collateral action
pub fn execute_add_collateral(
    position: &mut Position,
    asset_id: AssetId,
    amount: Amount,
    current_block: u64,
) -> Result<LendingEvent> {
    let current_collateral = position
        .collateral
        .get(&asset_id)
        .copied()
        .unwrap_or(Amount::ZERO);

    let new_collateral = current_collateral
        .checked_add(amount)
        .ok_or_else(|| anyhow::anyhow!("overflow in collateral"))?;

    position.collateral.insert(asset_id, new_collateral);
    position.last_update_block = current_block;

    Ok(LendingEvent::CollateralAdded {
        user: position.owner,
        asset_id,
        amount,
    })
}

/// Execute remove collateral action
pub fn execute_remove_collateral(
    position: &mut Position,
    asset_id: AssetId,
    amount: Amount,
    current_block: u64,
    oracle_prices: &std::collections::HashMap<AssetId, Amount>,
    lending_pool: &LendingPool,
) -> Result<LendingEvent> {
    let current_collateral = position
        .collateral
        .get(&asset_id)
        .copied()
        .ok_or_else(|| anyhow::anyhow!("no collateral for this asset"))?;

    if amount.0 > current_collateral.0 {
        bail!("insufficient collateral");
    }

    let new_collateral = current_collateral
        .checked_sub(amount)
        .ok_or_else(|| anyhow::anyhow!("underflow in collateral"))?;

    // Update position temporarily to check health
    if new_collateral == Amount::ZERO {
        position.collateral.remove(&asset_id);
    } else {
        position.collateral.insert(asset_id, new_collateral);
    }

    // Check health factor after removal
    let health = position
        .health_factor(lending_pool, oracle_prices)
        .ok_or_else(|| anyhow::anyhow!("failed to calculate health factor"))?;

    if health.lt(&Ratio::ONE) {
        // Revert the change
        position.collateral.insert(asset_id, current_collateral);
        bail!("removing collateral would make position undercollateralized");
    }

    position.last_update_block = current_block;

    Ok(LendingEvent::CollateralRemoved {
        user: position.owner,
        asset_id,
        amount,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_supply_and_withdraw() {
        let mut pool = PoolState {
            asset_id: AssetId([1; 32]),
            total_supplied: Amount::ZERO,
            total_borrowed: Amount::ZERO,
            last_update_block: 0,
            params: PoolParams::default(),
        };

        let user = [2; 32];

        // Supply
        execute_supply(&mut pool, user, Amount(1000), 0).unwrap();
        assert_eq!(pool.total_supplied.0, 1000);

        // Withdraw
        execute_withdraw(&mut pool, user, Amount(500), 0).unwrap();
        assert_eq!(pool.total_supplied.0, 500);
    }

    #[test]
    fn test_borrow_and_repay() {
        let mut lending_pool = LendingPool::new();
        let asset_id = AssetId([1; 32]);
        lending_pool.add_pool(asset_id, PoolParams::default());

        let mut pool_state = lending_pool.get_pool_mut(&asset_id).unwrap().clone();
        pool_state.total_supplied = Amount(10000);

        let mut position = Position::new([2; 32]);
        position.collateral.insert(asset_id, Amount(2000));

        let mut prices = std::collections::HashMap::new();
        prices.insert(asset_id, Amount(1));

        // Borrow
        execute_borrow(
            &mut pool_state,
            &mut position,
            asset_id,
            Amount(1000),
            0,
            &prices,
            &lending_pool,
        )
        .unwrap();

        assert_eq!(pool_state.total_borrowed.0, 1000);
        assert_eq!(position.debt.get(&asset_id).unwrap().0, 1000);

        // Repay
        execute_repay(&mut pool_state, &mut position, asset_id, Amount(500), 0).unwrap();

        assert_eq!(pool_state.total_borrowed.0, 500);
        assert_eq!(position.debt.get(&asset_id).unwrap().0, 500);
    }

    #[test]
    fn test_insufficient_liquidity() {
        let mut pool = PoolState {
            asset_id: AssetId([1; 32]),
            total_supplied: Amount(1000),
            total_borrowed: Amount(800),
            last_update_block: 0,
            params: PoolParams::default(),
        };

        let user = [2; 32];

        // Try to withdraw more than available
        let result = execute_withdraw(&mut pool, user, Amount(300), 0);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("insufficient liquidity"));
    }
}
