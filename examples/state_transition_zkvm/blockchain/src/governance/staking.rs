//! Staking Module
//!
//! Manages stake bonding, unbonding, and account ledgers.

use super::{AccountId, Balance, EraIndex};
use anyhow::{bail, Result};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// Nominator state
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NominatorState {
    /// Nominator account
    pub account: AccountId,

    /// Active stake (currently bonded)
    pub active_stake: Balance,

    /// Unbonding stake (waiting to unlock)
    pub unbonding: Vec<UnbondingChunk>,

    /// Nominated validators
    pub targets: Vec<AccountId>,

    /// Total rewards earned (cumulative)
    pub total_rewards: Balance,

    /// Pending rewards (not yet claimed)
    pub pending_rewards: Balance,
}

impl NominatorState {
    /// Create new nominator
    pub fn new(account: AccountId) -> Self {
        Self {
            account,
            active_stake: 0,
            unbonding: Vec::new(),
            targets: Vec::new(),
            total_rewards: 0,
            pending_rewards: 0,
        }
    }

    /// Total bonded stake (active + unbonding)
    pub fn total_bonded(&self) -> Balance {
        self.active_stake + self.unbonding.iter().map(|u| u.amount).sum::<Balance>()
    }

    /// Check if nominator can unbond (has active stake)
    pub fn can_unbond(&self) -> bool {
        self.active_stake > 0
    }

    /// Add pending rewards
    pub fn add_rewards(&mut self, amount: Balance) {
        self.pending_rewards += amount;
        self.total_rewards += amount;
    }

    /// Claim pending rewards
    pub fn claim_rewards(&mut self) -> Balance {
        let amount = self.pending_rewards;
        self.pending_rewards = 0;
        amount
    }
}

/// Unbonding chunk
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UnbondingChunk {
    /// Amount being unbonded
    pub amount: Balance,

    /// Era when unbonding was initiated
    pub unbond_era: EraIndex,

    /// Era when funds will be unlocked
    pub unlock_era: EraIndex,
}

impl UnbondingChunk {
    /// Check if unbonding period has passed
    pub fn is_unlocked(&self, current_era: EraIndex) -> bool {
        current_era >= self.unlock_era
    }
}

/// Staking ledger (per account)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StakingLedger {
    /// Account ID
    pub account: AccountId,

    /// Total staked (active + unbonding)
    pub total: Balance,

    /// Active bonded stake
    pub active: Balance,

    /// Unbonding chunks
    pub unbonding: Vec<UnbondingChunk>,
}

impl StakingLedger {
    /// Create new ledger
    pub fn new(account: AccountId) -> Self {
        Self {
            account,
            total: 0,
            active: 0,
            unbonding: Vec::new(),
        }
    }

    /// Bond additional stake
    pub fn bond(&mut self, amount: Balance) -> Result<()> {
        if amount == 0 {
            bail!("Cannot bond zero amount");
        }

        self.active += amount;
        self.total += amount;

        Ok(())
    }

    /// Unbond stake
    pub fn unbond(&mut self, amount: Balance, current_era: EraIndex, unbonding_eras: u32) -> Result<()> {
        if amount == 0 {
            bail!("Cannot unbond zero amount");
        }

        if amount > self.active {
            bail!("Insufficient active stake to unbond");
        }

        // Move stake from active to unbonding
        self.active -= amount;

        let chunk = UnbondingChunk {
            amount,
            unbond_era: current_era,
            unlock_era: current_era + unbonding_eras as u64,
        };

        self.unbonding.push(chunk);

        Ok(())
    }

    /// Withdraw unlocked funds
    pub fn withdraw_unbonded(&mut self, current_era: EraIndex) -> Result<Balance> {
        let (unlocked, still_locked): (Vec<_>, Vec<_>) = self
            .unbonding
            .iter()
            .partition(|chunk| chunk.is_unlocked(current_era));

        let withdrawn_amount: Balance = unlocked.iter().map(|chunk| chunk.amount).sum();

        if withdrawn_amount > 0 {
            self.total -= withdrawn_amount;
            self.unbonding = still_locked.into_iter().cloned().collect();
        }

        Ok(withdrawn_amount)
    }

    /// Get total unbonding amount
    pub fn unbonding_total(&self) -> Balance {
        self.unbonding.iter().map(|chunk| chunk.amount).sum()
    }

    /// Check if can withdraw any funds
    pub fn can_withdraw(&self, current_era: EraIndex) -> bool {
        self.unbonding.iter().any(|chunk| chunk.is_unlocked(current_era))
    }
}

/// Staking manager
pub struct StakingManager {
    /// Staking ledgers
    ledgers: BTreeMap<AccountId, StakingLedger>,

    /// Nominator states
    nominators: BTreeMap<AccountId, NominatorState>,

    /// Configuration
    config: StakingConfig,
}

#[derive(Debug, Clone)]
pub struct StakingConfig {
    /// Minimum bonded amount
    pub min_bond: Balance,

    /// Unbonding period in eras
    pub unbonding_eras: u32,

    /// Maximum unbonding chunks per account
    pub max_unbonding_chunks: usize,
}

impl Default for StakingConfig {
    fn default() -> Self {
        Self {
            min_bond: 100 * 10u128.pow(18),    // 100 ZT
            unbonding_eras: 168,                // 7 days at 1 hour per era
            max_unbonding_chunks: 32,
        }
    }
}

impl StakingManager {
    /// Create new staking manager
    pub fn new(config: StakingConfig) -> Self {
        Self {
            ledgers: BTreeMap::new(),
            nominators: BTreeMap::new(),
            config,
        }
    }

    /// Bond stake for nominator
    pub fn bond(&mut self, account: AccountId, amount: Balance) -> Result<()> {
        if amount < self.config.min_bond {
            bail!("Amount {} below minimum bond {}", amount, self.config.min_bond);
        }

        // Get or create ledger
        let ledger = self
            .ledgers
            .entry(account)
            .or_insert_with(|| StakingLedger::new(account));

        ledger.bond(amount)?;

        // Get or create nominator
        let nominator = self
            .nominators
            .entry(account)
            .or_insert_with(|| NominatorState::new(account));

        nominator.active_stake += amount;

        tracing::info!(
            "Bonded {} for account {} (total: {})",
            amount,
            hex::encode(account),
            ledger.total
        );

        Ok(())
    }

    /// Unbond stake
    pub fn unbond(&mut self, account: &AccountId, amount: Balance, current_era: EraIndex) -> Result<()> {
        let ledger = self
            .ledgers
            .get_mut(account)
            .ok_or_else(|| anyhow::anyhow!("Account not found"))?;

        // Check max unbonding chunks
        if ledger.unbonding.len() >= self.config.max_unbonding_chunks {
            bail!("Maximum unbonding chunks reached");
        }

        ledger.unbond(amount, current_era, self.config.unbonding_eras)?;

        // Update nominator
        let nominator = self
            .nominators
            .get_mut(account)
            .ok_or_else(|| anyhow::anyhow!("Nominator not found"))?;

        nominator.active_stake -= amount;

        tracing::info!(
            "Unbonded {} for account {} (unlocks at era {})",
            amount,
            hex::encode(account),
            current_era + self.config.unbonding_eras as u64
        );

        Ok(())
    }

    /// Withdraw unlocked funds
    pub fn withdraw_unbonded(&mut self, account: &AccountId, current_era: EraIndex) -> Result<Balance> {
        let ledger = self
            .ledgers
            .get_mut(account)
            .ok_or_else(|| anyhow::anyhow!("Account not found"))?;

        let withdrawn = ledger.withdraw_unbonded(current_era)?;

        tracing::info!(
            "Withdrew {} unbonded funds for account {}",
            withdrawn,
            hex::encode(account)
        );

        Ok(withdrawn)
    }

    /// Add rewards to nominator
    pub fn add_rewards(&mut self, account: &AccountId, amount: Balance) -> Result<()> {
        let nominator = self
            .nominators
            .get_mut(account)
            .ok_or_else(|| anyhow::anyhow!("Nominator not found"))?;

        nominator.add_rewards(amount);

        tracing::debug!("Added {} rewards for account {}", amount, hex::encode(account));

        Ok(())
    }

    /// Claim rewards
    pub fn claim_rewards(&mut self, account: &AccountId) -> Result<Balance> {
        let nominator = self
            .nominators
            .get_mut(account)
            .ok_or_else(|| anyhow::anyhow!("Nominator not found"))?;

        let amount = nominator.claim_rewards();

        tracing::info!("Claimed {} rewards for account {}", amount, hex::encode(account));

        Ok(amount)
    }

    /// Get staking ledger
    pub fn get_ledger(&self, account: &AccountId) -> Option<&StakingLedger> {
        self.ledgers.get(account)
    }

    /// Get nominator state
    pub fn get_nominator(&self, account: &AccountId) -> Option<&NominatorState> {
        self.nominators.get(account)
    }

    /// Get total staked across all accounts
    pub fn total_staked(&self) -> Balance {
        self.ledgers.values().map(|l| l.total).sum()
    }

    /// Get total active stake
    pub fn total_active_stake(&self) -> Balance {
        self.ledgers.values().map(|l| l.active).sum()
    }

    /// Get total unbonding stake
    pub fn total_unbonding(&self) -> Balance {
        self.ledgers.values().map(|l| l.unbonding_total()).sum()
    }

    /// Get number of nominators
    pub fn nominator_count(&self) -> usize {
        self.nominators.len()
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

    #[test]
    fn test_bond_and_unbond() {
        let mut manager = StakingManager::new(StakingConfig::default());
        let account = create_test_account(1);
        let amount = 1000 * 10u128.pow(18);

        // Bond
        manager.bond(account, amount).unwrap();
        let ledger = manager.get_ledger(&account).unwrap();
        assert_eq!(ledger.active, amount);
        assert_eq!(ledger.total, amount);

        // Unbond
        manager.unbond(&account, amount / 2, 1).unwrap();
        let ledger = manager.get_ledger(&account).unwrap();
        assert_eq!(ledger.active, amount / 2);
        assert_eq!(ledger.unbonding.len(), 1);
    }

    #[test]
    fn test_withdraw_unbonded() {
        let mut manager = StakingManager::new(StakingConfig::default());
        let account = create_test_account(1);
        let amount = 1000 * 10u128.pow(18);

        // Bond and unbond
        manager.bond(account, amount).unwrap();
        manager.unbond(&account, amount, 1).unwrap();

        // Try to withdraw before unlock era
        let withdrawn = manager.withdraw_unbonded(&account, 100).unwrap();
        assert_eq!(withdrawn, 0);

        // Withdraw after unlock era (1 + 168 = 169)
        let withdrawn = manager.withdraw_unbonded(&account, 200).unwrap();
        assert_eq!(withdrawn, amount);

        let ledger = manager.get_ledger(&account).unwrap();
        assert_eq!(ledger.total, 0);
        assert_eq!(ledger.unbonding.len(), 0);
    }

    #[test]
    fn test_rewards() {
        let mut manager = StakingManager::new(StakingConfig::default());
        let account = create_test_account(1);

        // Bond first
        manager.bond(account, 1000 * 10u128.pow(18)).unwrap();

        // Add rewards
        manager.add_rewards(&account, 50 * 10u128.pow(18)).unwrap();
        manager.add_rewards(&account, 30 * 10u128.pow(18)).unwrap();

        let nominator = manager.get_nominator(&account).unwrap();
        assert_eq!(nominator.pending_rewards, 80 * 10u128.pow(18));
        assert_eq!(nominator.total_rewards, 80 * 10u128.pow(18));

        // Claim
        let claimed = manager.claim_rewards(&account).unwrap();
        assert_eq!(claimed, 80 * 10u128.pow(18));

        let nominator = manager.get_nominator(&account).unwrap();
        assert_eq!(nominator.pending_rewards, 0);
        assert_eq!(nominator.total_rewards, 80 * 10u128.pow(18));
    }

    #[test]
    fn test_min_bond() {
        let mut manager = StakingManager::new(StakingConfig::default());
        let account = create_test_account(1);

        // Try to bond below minimum
        let result = manager.bond(account, 10 * 10u128.pow(18));
        assert!(result.is_err());

        // Bond above minimum
        let result = manager.bond(account, 100 * 10u128.pow(18));
        assert!(result.is_ok());
    }

    #[test]
    fn test_max_unbonding_chunks() {
        let config = StakingConfig {
            max_unbonding_chunks: 2,
            ..Default::default()
        };
        let mut manager = StakingManager::new(config);
        let account = create_test_account(1);

        // Bond
        manager.bond(account, 1000 * 10u128.pow(18)).unwrap();

        // Unbond twice (should succeed)
        manager.unbond(&account, 100 * 10u128.pow(18), 1).unwrap();
        manager.unbond(&account, 100 * 10u128.pow(18), 2).unwrap();

        // Try to unbond third time (should fail)
        let result = manager.unbond(&account, 100 * 10u128.pow(18), 3);
        assert!(result.is_err());
    }
}
