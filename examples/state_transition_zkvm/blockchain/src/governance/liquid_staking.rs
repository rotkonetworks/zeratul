//! Liquid Staking with stZT
//!
//! Simple liquid staking where rewards auto-compound into the exchange rate.
//! Users hold stZT tokens that increase in value as validators earn rewards.

use super::{AccountId, Balance, EraIndex};
use crate::frost::{FrostSignature, ThresholdRequirement};
use anyhow::{bail, Result};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// Liquid staking token (stZT)
///
/// Key properties:
/// - Exchange rate increases with rewards (auto-compounding)
/// - Backed 1:1 by ZT in FROST custody pool
/// - Can be used as collateral, traded, etc.
/// - Unbonding requires 7-day waiting period
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LiquidStakingPool {
    /// Total stZT supply
    pub total_supply: Balance,

    /// Total ZT backing (in FROST custody)
    pub total_backing: Balance,

    /// Current exchange rate (backing / supply)
    /// When rewards are added, backing increases → rate increases
    pub exchange_rate_numerator: u128,
    pub exchange_rate_denominator: u128,

    /// FROST custody pool (11/15 validators)
    pub custody_pool: FrostCustodyPool,

    /// Pending unbonding requests
    pub unbonding_queue: Vec<UnbondingRequest>,

    /// Era when pool was created
    pub inception_era: EraIndex,
}

impl LiquidStakingPool {
    /// Create new liquid staking pool
    pub fn new(inception_era: EraIndex) -> Self {
        Self {
            total_supply: 0,
            total_backing: 0,
            exchange_rate_numerator: 1,
            exchange_rate_denominator: 1,
            custody_pool: FrostCustodyPool::new(),
            unbonding_queue: Vec::new(),
            inception_era,
        }
    }

    /// Get current exchange rate (ZT per stZT)
    pub fn current_rate(&self) -> (u128, u128) {
        if self.total_supply == 0 {
            return (1, 1); // Initial rate: 1 stZT = 1 ZT
        }

        // Rate = total_backing / total_supply
        (self.total_backing, self.total_supply)
    }

    /// Stake ZT → mint stZT
    ///
    /// User deposits ZT, receives stZT based on current exchange rate
    pub fn stake(&mut self, account: AccountId, zt_amount: Balance) -> Result<Balance> {
        if zt_amount == 0 {
            bail!("Cannot stake zero amount");
        }

        // Calculate stZT to mint based on current rate
        let (backing_per_supply, supply_per_backing) = self.current_rate();
        let stz_minted = if self.total_supply == 0 {
            // First deposit: 1:1 rate
            zt_amount
        } else {
            // Calculate: stZT = ZT * (total_supply / total_backing)
            zt_amount
                .checked_mul(supply_per_backing)
                .and_then(|v| v.checked_div(backing_per_supply))
                .ok_or_else(|| anyhow::anyhow!("Overflow calculating stZT amount"))?
        };

        // Update pool state
        self.total_supply = self
            .total_supply
            .checked_add(stz_minted)
            .ok_or_else(|| anyhow::anyhow!("Overflow adding to total supply"))?;

        self.total_backing = self
            .total_backing
            .checked_add(zt_amount)
            .ok_or_else(|| anyhow::anyhow!("Overflow adding to total backing"))?;

        // Custody pool receives the ZT
        self.custody_pool.deposit(zt_amount)?;

        tracing::info!(
            "Staked {} ZT for account {} → minted {} stZT (rate: {}/{})",
            zt_amount,
            hex::encode(account),
            stz_minted,
            backing_per_supply,
            supply_per_backing
        );

        Ok(stz_minted)
    }

    /// Request unbonding: stZT → ZT (after delay)
    ///
    /// Burns stZT, queues withdrawal from FROST custody
    pub fn request_unbond(
        &mut self,
        account: AccountId,
        stz_amount: Balance,
        current_era: EraIndex,
    ) -> Result<Balance> {
        if stz_amount == 0 {
            bail!("Cannot unbond zero amount");
        }

        if stz_amount > self.total_supply {
            bail!("Insufficient stZT supply");
        }

        // Calculate ZT to receive based on current rate
        let (backing_per_supply, supply_per_backing) = self.current_rate();
        let zt_to_receive = stz_amount
            .checked_mul(backing_per_supply)
            .and_then(|v| v.checked_div(supply_per_backing))
            .ok_or_else(|| anyhow::anyhow!("Overflow calculating ZT amount"))?;

        // Burn stZT immediately
        self.total_supply = self
            .total_supply
            .checked_sub(stz_amount)
            .ok_or_else(|| anyhow::anyhow!("Underflow subtracting from supply"))?;

        self.total_backing = self
            .total_backing
            .checked_sub(zt_to_receive)
            .ok_or_else(|| anyhow::anyhow!("Underflow subtracting from backing"))?;

        // Add to unbonding queue (7-day delay)
        let unlock_era = current_era + 168; // 168 eras = 7 days at 1 hour per era
        let request = UnbondingRequest {
            account,
            amount: zt_to_receive,
            request_era: current_era,
            unlock_era,
        };

        self.unbonding_queue.push(request);

        tracing::info!(
            "Unbonding request: {} stZT → {} ZT for account {} (unlocks at era {})",
            stz_amount,
            zt_to_receive,
            hex::encode(account),
            unlock_era
        );

        Ok(zt_to_receive)
    }

    /// Process unbonding queue (requires FROST 11/15 signature)
    ///
    /// Withdraws unlocked funds from FROST custody
    pub fn process_unbonding(
        &mut self,
        current_era: EraIndex,
        frost_signature: FrostSignature,
    ) -> Result<Vec<(AccountId, Balance)>> {
        // Verify FROST signature (11/15 Byzantine threshold)
        if frost_signature.threshold != ThresholdRequirement::ByzantineThreshold {
            bail!("Invalid threshold: expected 11/15 for withdrawals");
        }

        frost_signature.verify_threshold(15)?;

        // Find all unlocked requests
        let (unlocked, still_locked): (Vec<_>, Vec<_>) = self
            .unbonding_queue
            .iter()
            .cloned()
            .partition(|req| req.unlock_era <= current_era);

        if unlocked.is_empty() {
            return Ok(Vec::new());
        }

        // Calculate total to withdraw
        let total_withdrawal: Balance = unlocked.iter().map(|req| req.amount).sum();

        // Withdraw from FROST custody
        self.custody_pool.withdraw(total_withdrawal, frost_signature)?;

        // Update queue
        self.unbonding_queue = still_locked;

        // Collect payouts
        let payouts: Vec<(AccountId, Balance)> = unlocked
            .iter()
            .map(|req| (req.account, req.amount))
            .collect();

        tracing::info!(
            "Processed {} unbonding requests, withdrew {} ZT from custody",
            payouts.len(),
            total_withdrawal
        );

        Ok(payouts)
    }

    /// Add era rewards (auto-compounds into exchange rate)
    ///
    /// When validators earn rewards, they're added to backing.
    /// This increases the exchange rate → stZT holders get richer!
    pub fn add_rewards(&mut self, rewards: Balance) -> Result<()> {
        if rewards == 0 {
            return Ok(());
        }

        let old_rate = self.current_rate();

        // Add to backing (supply stays same → rate increases!)
        self.total_backing = self
            .total_backing
            .checked_add(rewards)
            .ok_or_else(|| anyhow::anyhow!("Overflow adding rewards"))?;

        self.custody_pool.deposit(rewards)?;

        let new_rate = self.current_rate();

        tracing::info!(
            "Added {} ZT rewards to pool. Rate: {}/{} → {}/{}",
            rewards,
            old_rate.0,
            old_rate.1,
            new_rate.0,
            new_rate.1
        );

        Ok(())
    }

    /// Get stZT value in ZT
    pub fn stz_to_zt(&self, stz_amount: Balance) -> Balance {
        if self.total_supply == 0 {
            return stz_amount; // 1:1 rate initially
        }

        let (backing, supply) = self.current_rate();
        stz_amount
            .checked_mul(backing)
            .and_then(|v| v.checked_div(supply))
            .unwrap_or(0)
    }

    /// Get ZT value in stZT
    pub fn zt_to_stz(&self, zt_amount: Balance) -> Balance {
        if self.total_supply == 0 {
            return zt_amount; // 1:1 rate initially
        }

        let (backing, supply) = self.current_rate();
        zt_amount
            .checked_mul(supply)
            .and_then(|v| v.checked_div(backing))
            .unwrap_or(0)
    }

    /// Get APY for stakers
    ///
    /// APY = (era_rewards / total_backing) * eras_per_year * 100
    pub fn calculate_apy(&self, era_rewards: Balance, eras_per_year: u64) -> f64 {
        if self.total_backing == 0 {
            return 0.0;
        }

        let era_return = era_rewards as f64 / self.total_backing as f64;
        let annual_return = era_return * eras_per_year as f64;
        annual_return * 100.0
    }
}

/// FROST custody pool (11/15 Byzantine threshold)
///
/// Validators collectively custody all staked ZT.
/// Trust assumption: 11/15 validators must cooperate to steal funds.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FrostCustodyPool {
    /// Total ZT in custody
    pub total_custody: Balance,

    /// Validator set commitment (for ZK proofs later)
    pub validator_set_commitment: Option<[u8; 32]>,
}

impl FrostCustodyPool {
    pub fn new() -> Self {
        Self {
            total_custody: 0,
            validator_set_commitment: None,
        }
    }

    /// Deposit ZT into custody
    pub fn deposit(&mut self, amount: Balance) -> Result<()> {
        self.total_custody = self
            .total_custody
            .checked_add(amount)
            .ok_or_else(|| anyhow::anyhow!("Overflow in custody deposit"))?;

        tracing::debug!("Deposited {} ZT to FROST custody (total: {})", amount, self.total_custody);

        Ok(())
    }

    /// Withdraw ZT from custody (requires FROST 11/15 signature)
    pub fn withdraw(&mut self, amount: Balance, frost_signature: FrostSignature) -> Result<()> {
        // Verify FROST signature
        if frost_signature.threshold != ThresholdRequirement::ByzantineThreshold {
            bail!("Invalid threshold for withdrawal");
        }

        if amount > self.total_custody {
            bail!("Insufficient custody funds: requested {}, available {}", amount, self.total_custody);
        }

        self.total_custody = self
            .total_custody
            .checked_sub(amount)
            .ok_or_else(|| anyhow::anyhow!("Underflow in custody withdrawal"))?;

        tracing::info!(
            "Withdrew {} ZT from FROST custody (remaining: {}, authorized by {}/{} validators)",
            amount,
            self.total_custody,
            frost_signature.signers.len(),
            15
        );

        Ok(())
    }
}

/// Unbonding request
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UnbondingRequest {
    /// Account requesting unbond
    pub account: AccountId,

    /// Amount of ZT to receive
    pub amount: Balance,

    /// Era when unbonding was requested
    pub request_era: EraIndex,

    /// Era when funds will be unlocked
    pub unlock_era: EraIndex,
}

impl UnbondingRequest {
    pub fn is_unlocked(&self, current_era: EraIndex) -> bool {
        current_era >= self.unlock_era
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn create_test_account(id: u8) -> AccountId {
        let mut account = [0u8; 32];
        account[0] = id;
        account
    }

    fn create_test_frost_signature() -> FrostSignature {
        FrostSignature {
            signature: [0u8; 64],
            signers: (0..11).collect(), // 11/15 signers
            threshold: ThresholdRequirement::ByzantineThreshold,
        }
    }

    #[test]
    fn test_initial_staking() {
        let mut pool = LiquidStakingPool::new(1);
        let account = create_test_account(1);

        // First stake: 1:1 rate
        let stz_minted = pool.stake(account, 1000 * 10u128.pow(18)).unwrap();
        assert_eq!(stz_minted, 1000 * 10u128.pow(18));
        assert_eq!(pool.total_supply, 1000 * 10u128.pow(18));
        assert_eq!(pool.total_backing, 1000 * 10u128.pow(18));

        let (backing, supply) = pool.current_rate();
        assert_eq!(backing, supply); // 1:1 initially
    }

    #[test]
    fn test_rewards_increase_rate() {
        let mut pool = LiquidStakingPool::new(1);
        let account = create_test_account(1);

        // Initial stake
        pool.stake(account, 1000 * 10u128.pow(18)).unwrap();

        // Add 100 ZT rewards
        pool.add_rewards(100 * 10u128.pow(18)).unwrap();

        // Supply unchanged, backing increased → rate increased
        assert_eq!(pool.total_supply, 1000 * 10u128.pow(18));
        assert_eq!(pool.total_backing, 1100 * 10u128.pow(18));

        // 1000 stZT now worth 1100 ZT
        let zt_value = pool.stz_to_zt(1000 * 10u128.pow(18));
        assert_eq!(zt_value, 1100 * 10u128.pow(18));
    }

    #[test]
    fn test_stake_after_rewards() {
        let mut pool = LiquidStakingPool::new(1);
        let alice = create_test_account(1);
        let bob = create_test_account(2);

        // Alice stakes 1000 ZT
        pool.stake(alice, 1000 * 10u128.pow(18)).unwrap();

        // Add 100 ZT rewards (10% gain)
        pool.add_rewards(100 * 10u128.pow(18)).unwrap();

        // Rate is now 1100 ZT / 1000 stZT = 1.1 ZT per stZT

        // Bob stakes 1100 ZT → should get 1000 stZT
        let bob_stz = pool.stake(bob, 1100 * 10u128.pow(18)).unwrap();
        assert_eq!(bob_stz, 1000 * 10u128.pow(18));

        // Total: 2000 stZT backed by 2200 ZT
        assert_eq!(pool.total_supply, 2000 * 10u128.pow(18));
        assert_eq!(pool.total_backing, 2200 * 10u128.pow(18));
    }

    #[test]
    fn test_unbonding() {
        let mut pool = LiquidStakingPool::new(1);
        let account = create_test_account(1);

        // Stake 1000 ZT
        let stz_minted = pool.stake(account, 1000 * 10u128.pow(18)).unwrap();

        // Add rewards (10% gain)
        pool.add_rewards(100 * 10u128.pow(18)).unwrap();

        // Unbond 500 stZT → should get 550 ZT (500 * 1.1)
        let zt_to_receive = pool.request_unbond(account, 500 * 10u128.pow(18), 1).unwrap();
        assert_eq!(zt_to_receive, 550 * 10u128.pow(18));

        // Should be in unbonding queue
        assert_eq!(pool.unbonding_queue.len(), 1);
        assert_eq!(pool.unbonding_queue[0].unlock_era, 169); // 1 + 168
    }

    #[test]
    fn test_process_unbonding() {
        let mut pool = LiquidStakingPool::new(1);
        let account = create_test_account(1);

        // Stake and unbond
        pool.stake(account, 1000 * 10u128.pow(18)).unwrap();
        pool.request_unbond(account, 1000 * 10u128.pow(18), 1).unwrap();

        // Try to process before unlock era
        let frost_sig = create_test_frost_signature();
        let payouts = pool.process_unbonding(100, frost_sig.clone()).unwrap();
        assert_eq!(payouts.len(), 0);

        // Process after unlock era
        let payouts = pool.process_unbonding(200, frost_sig).unwrap();
        assert_eq!(payouts.len(), 1);
        assert_eq!(payouts[0].0, account);
        assert_eq!(payouts[0].1, 1000 * 10u128.pow(18));

        // Queue should be empty
        assert_eq!(pool.unbonding_queue.len(), 0);
    }

    #[test]
    fn test_apy_calculation() {
        let mut pool = LiquidStakingPool::new(1);
        let account = create_test_account(1);

        // Stake 1000 ZT
        pool.stake(account, 1000 * 10u128.pow(18)).unwrap();

        // Era reward = 100 ZT
        // Backing = 1000 ZT
        // Era return = 10%
        // Eras per year = 365
        // APY = 10% * 365 = 3650%
        let apy = pool.calculate_apy(100 * 10u128.pow(18), 365);
        assert!((apy - 3650.0).abs() < 0.1);
    }

    #[test]
    fn test_conversion_helpers() {
        let mut pool = LiquidStakingPool::new(1);
        let account = create_test_account(1);

        pool.stake(account, 1000 * 10u128.pow(18)).unwrap();
        pool.add_rewards(100 * 10u128.pow(18)).unwrap();

        // Rate: 1100 ZT / 1000 stZT = 1.1

        // 500 stZT → 550 ZT
        let zt = pool.stz_to_zt(500 * 10u128.pow(18));
        assert_eq!(zt, 550 * 10u128.pow(18));

        // 1100 ZT → 1000 stZT
        let stz = pool.zt_to_stz(1100 * 10u128.pow(18));
        assert_eq!(stz, 1000 * 10u128.pow(18));
    }
}
