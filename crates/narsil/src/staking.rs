//! nZEC staking token with exchange rate appreciation
//!
//! adapted from penumbra's delegation token model. no inflation -
//! rewards come entirely from rake revenue (1% of arbitrated pots).
//!
//! # how it works
//!
//! deposit ZEC into FROST escrow → receive nZEC at current exchange rate.
//! each era, rake revenue increases the ZEC/nZEC exchange rate.
//! your nZEC position appreciates without receiving "income".
//! undelegate: burn nZEC → receive ZEC at new (higher) rate.
//!
//! # nZEC properties
//!
//! - nomination power: weight votes in NPoS election
//! - slashable: misbehavior reduces exchange rate for that validator's pool
//! - no direct payouts: exchange rate appreciation only
//! - per-validator: nZEC(alice) and nZEC(bob) are separate pools with
//!   separate exchange rates (different commission, different slash history)
//!
//! # exchange rate math (from penumbra, simplified)
//!
//! ```text
//! delegate:   nzec_amount = zec_amount / exchange_rate
//! undelegate: zec_amount  = nzec_amount * exchange_rate
//!
//! era update: exchange_rate *= (1 + reward_rate)
//! where:      reward_rate = era_rake_revenue / total_staked_zec
//!
//! slash:      exchange_rate *= (1 - penalty_rate)
//! ```

use alloc::collections::BTreeMap;
use alloc::vec::Vec;

use crate::wire::Hash32;

/// scaling factor for fixed-point exchange rates (10^8, same as penumbra)
const RATE_SCALE: u128 = 100_000_000;

/// initial exchange rate: 1 nZEC = 1 ZEC (scaled)
const INITIAL_RATE: u128 = RATE_SCALE;

/// per-validator staking pool
#[derive(Clone, Debug)]
pub struct ValidatorPool {
    /// validator pubkey
    pub validator: Hash32,
    /// ZEC/nZEC exchange rate (fixed-point, scaled by RATE_SCALE)
    pub exchange_rate: u128,
    /// total nZEC in this pool
    pub total_nzec: u128,
    /// commission in basis points (validator takes this % of rewards)
    pub commission_bps: u32,
    /// whether pool is accepting new delegations
    pub active: bool,
}

impl ValidatorPool {
    pub fn new(validator: Hash32, commission_bps: u32) -> Self {
        Self {
            validator,
            exchange_rate: INITIAL_RATE,
            total_nzec: 0,
            commission_bps,
            active: true,
        }
    }

    /// total ZEC value of this pool
    pub fn total_zec(&self) -> u64 {
        ((self.total_nzec * self.exchange_rate) / RATE_SCALE) as u64
    }

    /// convert ZEC amount to nZEC at current rate
    pub fn zec_to_nzec(&self, zec: u64) -> u128 {
        (zec as u128 * RATE_SCALE) / self.exchange_rate
    }

    /// convert nZEC amount to ZEC at current rate
    pub fn nzec_to_zec(&self, nzec: u128) -> u64 {
        ((nzec * self.exchange_rate) / RATE_SCALE) as u64
    }
}

/// delegation record: who staked how much nZEC to which validator
#[derive(Clone, Debug)]
pub struct Delegation {
    /// delegator pubkey
    pub delegator: Hash32,
    /// validator pubkey
    pub validator: Hash32,
    /// nZEC amount held
    pub nzec: u128,
    /// era when delegated (for unbonding period tracking)
    pub start_era: u64,
}

impl Delegation {
    /// current ZEC value of this delegation
    pub fn zec_value(&self, pool: &ValidatorPool) -> u64 {
        pool.nzec_to_zec(self.nzec)
    }
}

/// pending undelegation (unbonding period)
#[derive(Clone, Debug)]
pub struct Undelegation {
    /// delegator pubkey
    pub delegator: Hash32,
    /// validator pubkey
    pub validator: Hash32,
    /// nZEC being undelegated
    pub nzec: u128,
    /// ZEC value at time of undelegation (locked in)
    pub zec_value: u64,
    /// era when unbonding completes
    pub completion_era: u64,
}

/// staking error
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum StakingError {
    /// pool doesn't exist
    UnknownValidator(Hash32),
    /// pool not accepting delegations
    PoolInactive,
    /// insufficient nZEC balance
    InsufficientBalance { has: u128, needs: u128 },
    /// unbonding not complete
    StillUnbonding { current_era: u64, completion_era: u64 },
    /// validator already registered
    AlreadyRegistered,
    /// zero amount
    ZeroAmount,
}

/// era reward distribution result
#[derive(Clone, Debug)]
pub struct EraRewards {
    pub era: u64,
    /// total rake collected this era
    pub total_rake: u64,
    /// per-validator: (validator, reward_to_pool, commission_to_validator)
    pub distributions: Vec<(Hash32, u64, u64)>,
}

/// staking state machine
#[derive(Clone, Debug)]
pub struct Staking {
    /// per-validator pools
    pools: BTreeMap<Hash32, ValidatorPool>,
    /// delegations: (delegator, validator) -> delegation
    delegations: BTreeMap<(Hash32, Hash32), Delegation>,
    /// pending undelegations
    undelegations: Vec<Undelegation>,
    /// current era
    pub current_era: u64,
    /// unbonding period in eras
    pub unbonding_eras: u64,
    /// accumulated rake this era (in ZEC base units)
    pub era_rake: u64,
}

impl Staking {
    pub fn new(unbonding_eras: u64) -> Self {
        Self {
            pools: BTreeMap::new(),
            delegations: BTreeMap::new(),
            undelegations: Vec::new(),
            current_era: 0,
            unbonding_eras,
            era_rake: 0,
        }
    }

    /// register a validator pool
    pub fn register_pool(
        &mut self,
        validator: Hash32,
        commission_bps: u32,
    ) -> Result<(), StakingError> {
        if self.pools.contains_key(&validator) {
            return Err(StakingError::AlreadyRegistered);
        }
        self.pools.insert(validator, ValidatorPool::new(validator, commission_bps));
        Ok(())
    }

    /// delegate ZEC to a validator, receive nZEC
    pub fn delegate(
        &mut self,
        delegator: Hash32,
        validator: Hash32,
        zec_amount: u64,
    ) -> Result<u128, StakingError> {
        if zec_amount == 0 {
            return Err(StakingError::ZeroAmount);
        }
        let pool = self.pools.get_mut(&validator)
            .ok_or(StakingError::UnknownValidator(validator))?;
        if !pool.active {
            return Err(StakingError::PoolInactive);
        }

        let nzec = pool.zec_to_nzec(zec_amount);
        pool.total_nzec += nzec;

        let key = (delegator, validator);
        let delegation = self.delegations.entry(key).or_insert(Delegation {
            delegator,
            validator,
            nzec: 0,
            start_era: self.current_era,
        });
        delegation.nzec += nzec;

        Ok(nzec)
    }

    /// start undelegation (enters unbonding period)
    pub fn undelegate(
        &mut self,
        delegator: Hash32,
        validator: Hash32,
        nzec_amount: u128,
    ) -> Result<Undelegation, StakingError> {
        if nzec_amount == 0 {
            return Err(StakingError::ZeroAmount);
        }
        let key = (delegator, validator);
        let delegation = self.delegations.get_mut(&key)
            .ok_or(StakingError::UnknownValidator(validator))?;

        if delegation.nzec < nzec_amount {
            return Err(StakingError::InsufficientBalance {
                has: delegation.nzec,
                needs: nzec_amount,
            });
        }

        let pool = self.pools.get_mut(&validator)
            .ok_or(StakingError::UnknownValidator(validator))?;

        let zec_value = pool.nzec_to_zec(nzec_amount);
        delegation.nzec -= nzec_amount;
        pool.total_nzec -= nzec_amount;

        // clean up empty delegations
        if delegation.nzec == 0 {
            self.delegations.remove(&key);
        }

        let undelegation = Undelegation {
            delegator,
            validator,
            nzec: nzec_amount,
            zec_value,
            completion_era: self.current_era + self.unbonding_eras,
        };
        self.undelegations.push(undelegation.clone());

        Ok(undelegation)
    }

    /// claim completed undelegations, returns total ZEC to return
    pub fn claim_undelegations(
        &mut self,
        delegator: &Hash32,
    ) -> Result<u64, StakingError> {
        let mut total_zec = 0u64;

        self.undelegations.retain(|u| {
            if &u.delegator == delegator && u.completion_era <= self.current_era {
                total_zec += u.zec_value;
                false // remove
            } else {
                true // keep
            }
        });

        Ok(total_zec)
    }

    /// record rake revenue for this era
    pub fn add_rake(&mut self, amount: u64) {
        self.era_rake += amount;
    }

    /// end era: distribute rake as exchange rate appreciation
    ///
    /// each validator pool's exchange rate increases proportional to
    /// their share of total staked ZEC. validator commission is paid
    /// by giving them a slightly better rate than delegators.
    pub fn end_era(&mut self, dev_fund_bps: u32) -> EraRewards {
        let total_rake = self.era_rake;
        let mut distributions = Vec::new();

        // dev fund gets its cut first
        let dev_cut = ((total_rake as u128 * dev_fund_bps as u128) / 10_000) as u64;
        let distributable = total_rake - dev_cut;

        // total staked ZEC across all pools
        let total_staked: u128 = self.pools.values()
            .filter(|p| p.active)
            .map(|p| p.total_nzec * p.exchange_rate / RATE_SCALE)
            .sum();

        if total_staked > 0 && distributable > 0 {
            for pool in self.pools.values_mut() {
                if !pool.active || pool.total_nzec == 0 {
                    continue;
                }

                let pool_zec = pool.total_nzec * pool.exchange_rate / RATE_SCALE;
                // pool's share of distributable rake
                let pool_reward = (distributable as u128 * pool_zec) / total_staked;

                // validator commission
                let commission = (pool_reward * pool.commission_bps as u128) / 10_000;
                let delegator_reward = pool_reward - commission;

                // increase exchange rate by delegator reward portion
                // new_rate = old_rate * (1 + delegator_reward / pool_zec)
                // = old_rate + old_rate * delegator_reward / pool_zec
                if pool_zec > 0 {
                    let rate_increase = (pool.exchange_rate * delegator_reward) / pool_zec;
                    pool.exchange_rate += rate_increase;
                }

                distributions.push((
                    pool.validator,
                    delegator_reward as u64,
                    commission as u64,
                ));
            }
        }

        self.current_era += 1;
        self.era_rake = 0;

        EraRewards {
            era: self.current_era,
            total_rake,
            distributions,
        }
    }

    /// slash a validator: reduce their pool's exchange rate
    /// penalty_bps: slash amount in basis points (1000 = 10%)
    pub fn slash(
        &mut self,
        validator: &Hash32,
        penalty_bps: u32,
    ) -> Result<u64, StakingError> {
        let pool = self.pools.get_mut(validator)
            .ok_or(StakingError::UnknownValidator(*validator))?;

        let old_zec = pool.total_zec();

        // reduce exchange rate: new_rate = old_rate * (1 - penalty/10000)
        pool.exchange_rate = (pool.exchange_rate * (10_000 - penalty_bps as u128)) / 10_000;

        let new_zec = pool.total_zec();
        let slashed = old_zec.saturating_sub(new_zec);

        // also slash pending undelegations for this validator
        for u in &mut self.undelegations {
            if u.validator == *validator && u.completion_era > self.current_era {
                let penalty = ((u.zec_value as u128 * penalty_bps as u128) / 10_000) as u64;
                u.zec_value = u.zec_value.saturating_sub(penalty);
            }
        }

        Ok(slashed)
    }

    /// get pool for validator
    pub fn pool(&self, validator: &Hash32) -> Option<&ValidatorPool> {
        self.pools.get(validator)
    }

    /// get delegation
    pub fn delegation(&self, delegator: &Hash32, validator: &Hash32) -> Option<&Delegation> {
        self.delegations.get(&(*delegator, *validator))
    }

    /// total ZEC staked across all pools
    pub fn total_staked(&self) -> u64 {
        self.pools.values().map(|p| p.total_zec()).sum()
    }

    /// get nZEC balance for a delegator across all validators
    pub fn nzec_balance(&self, delegator: &Hash32) -> Vec<(Hash32, u128, u64)> {
        self.delegations.iter()
            .filter(|((d, _), _)| d == delegator)
            .filter_map(|((_, v), del)| {
                self.pools.get(v).map(|pool| (*v, del.nzec, pool.nzec_to_zec(del.nzec)))
            })
            .collect()
    }

    /// nomination power: nZEC balance converted to ZEC value (for NPoS voting weight)
    pub fn nomination_power(&self, delegator: &Hash32) -> u64 {
        self.nzec_balance(delegator).iter().map(|(_, _, zec)| zec).sum()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pk(n: u8) -> Hash32 {
        let mut h = [0u8; 32];
        h[0] = n;
        h
    }

    #[test]
    fn test_delegate_and_undelegate() {
        let mut staking = Staking::new(2);
        staking.register_pool(pk(1), 500).unwrap(); // 5% commission

        // alice delegates 10000 ZEC
        let nzec = staking.delegate(pk(10), pk(1), 10_000).unwrap();
        assert_eq!(nzec, 10_000 * RATE_SCALE / INITIAL_RATE); // 1:1 at start

        let pool = staking.pool(&pk(1)).unwrap();
        assert_eq!(pool.total_zec(), 10_000);

        // alice undelegates half
        let u = staking.undelegate(pk(10), pk(1), nzec / 2).unwrap();
        assert_eq!(u.zec_value, 5_000);
        assert_eq!(u.completion_era, 2); // unbonding period

        // can't claim yet
        let claimed = staking.claim_undelegations(&pk(10)).unwrap();
        assert_eq!(claimed, 0);

        // advance 2 eras
        staking.end_era(0);
        staking.end_era(0);
        let claimed = staking.claim_undelegations(&pk(10)).unwrap();
        assert_eq!(claimed, 5_000);
    }

    #[test]
    fn test_exchange_rate_appreciation() {
        let mut staking = Staking::new(2);
        staking.register_pool(pk(1), 0).unwrap(); // 0% commission for simplicity

        // alice delegates 100_000 ZEC
        let nzec = staking.delegate(pk(10), pk(1), 100_000).unwrap();

        // era 1: 1000 ZEC rake collected (1% of 100k pot)
        staking.add_rake(1_000);
        let rewards = staking.end_era(0); // 0% dev fund for simplicity
        assert_eq!(rewards.total_rake, 1_000);

        // exchange rate should have increased
        let pool = staking.pool(&pk(1)).unwrap();
        assert!(pool.exchange_rate > INITIAL_RATE);

        // alice's nZEC is now worth more
        let zec_value = pool.nzec_to_zec(nzec);
        assert_eq!(zec_value, 101_000); // 100k + 1k rake
    }

    #[test]
    fn test_commission_split() {
        let mut staking = Staking::new(2);
        staking.register_pool(pk(1), 1000).unwrap(); // 10% commission

        // alice delegates 100_000 ZEC
        let nzec = staking.delegate(pk(10), pk(1), 100_000).unwrap();

        // 1000 ZEC rake
        staking.add_rake(1_000);
        let rewards = staking.end_era(0);

        // validator gets 10% commission = 100 ZEC
        // delegators get 90% = 900 ZEC via rate appreciation
        let (_, delegator_reward, commission) = rewards.distributions[0];
        assert_eq!(commission, 100);
        assert_eq!(delegator_reward, 900);

        // alice's nZEC is worth 100_000 + 900 = 100_900
        let pool = staking.pool(&pk(1)).unwrap();
        let zec_value = pool.nzec_to_zec(nzec);
        assert_eq!(zec_value, 100_900);
    }

    #[test]
    fn test_dev_fund_cut() {
        let mut staking = Staking::new(2);
        staking.register_pool(pk(1), 0).unwrap();

        staking.delegate(pk(10), pk(1), 100_000).unwrap();

        staking.add_rake(1_000);
        let rewards = staking.end_era(2000); // 20% to dev fund

        // 1000 rake - 200 dev fund = 800 distributable
        let (_, delegator_reward, _) = rewards.distributions[0];
        assert_eq!(delegator_reward, 800);
    }

    #[test]
    fn test_slash_reduces_exchange_rate() {
        let mut staking = Staking::new(2);
        staking.register_pool(pk(1), 0).unwrap();

        let nzec = staking.delegate(pk(10), pk(1), 100_000).unwrap();

        // slash 10%
        let slashed = staking.slash(&pk(1), 1000).unwrap();
        assert_eq!(slashed, 10_000);

        // alice's nZEC is now worth 90_000
        let pool = staking.pool(&pk(1)).unwrap();
        assert_eq!(pool.nzec_to_zec(nzec), 90_000);
    }

    #[test]
    fn test_slash_hits_unbonding() {
        let mut staking = Staking::new(5);
        staking.register_pool(pk(1), 0).unwrap();

        let nzec = staking.delegate(pk(10), pk(1), 100_000).unwrap();

        // start undelegation
        let u = staking.undelegate(pk(10), pk(1), nzec / 2).unwrap();
        assert_eq!(u.zec_value, 50_000);

        // slash 10% - should hit both staked and unbonding
        staking.slash(&pk(1), 1000).unwrap();

        // unbonding position also slashed
        let pending = &staking.undelegations[0];
        assert_eq!(pending.zec_value, 45_000); // 50k - 10%
    }

    #[test]
    fn test_nomination_power() {
        let mut staking = Staking::new(2);
        staking.register_pool(pk(1), 0).unwrap();
        staking.register_pool(pk(2), 0).unwrap();

        staking.delegate(pk(10), pk(1), 5_000).unwrap();
        staking.delegate(pk(10), pk(2), 3_000).unwrap();

        assert_eq!(staking.nomination_power(&pk(10)), 8_000);

        // after rake, nomination power increases
        staking.add_rake(800); // 1% of 80k
        staking.end_era(0);

        assert_eq!(staking.nomination_power(&pk(10)), 8_800);
    }

    #[test]
    fn test_multi_era_compounding() {
        let mut staking = Staking::new(2);
        staking.register_pool(pk(1), 0).unwrap();

        let nzec = staking.delegate(pk(10), pk(1), 100_000).unwrap();

        // 5 eras of 1% rake each
        for _ in 0..5 {
            let pool_zec = staking.pool(&pk(1)).unwrap().total_zec();
            let rake = pool_zec / 100; // 1% of current value
            staking.add_rake(rake);
            staking.end_era(0);
        }

        // should compound: 100k * 1.01^5 ≈ 105,101
        let final_value = staking.pool(&pk(1)).unwrap().nzec_to_zec(nzec);
        assert!(final_value >= 105_000, "compounding should exceed 105k, got {}", final_value);
        assert!(final_value <= 105_200, "should be ~105,101, got {}", final_value);
    }

    #[test]
    fn test_multiple_delegators_fair_split() {
        let mut staking = Staking::new(2);
        staking.register_pool(pk(1), 0).unwrap();

        // alice stakes 70k, bob stakes 30k
        let alice_nzec = staking.delegate(pk(10), pk(1), 70_000).unwrap();
        let bob_nzec = staking.delegate(pk(11), pk(1), 30_000).unwrap();

        // 1000 ZEC rake
        staking.add_rake(1_000);
        staking.end_era(0);

        let pool = staking.pool(&pk(1)).unwrap();
        let alice_zec = pool.nzec_to_zec(alice_nzec);
        let bob_zec = pool.nzec_to_zec(bob_nzec);

        // alice gets 70% of rewards = 700, bob gets 30% = 300
        assert_eq!(alice_zec, 70_700);
        assert_eq!(bob_zec, 30_300);
    }

    #[test]
    fn test_pool_registration_errors() {
        let mut staking = Staking::new(2);
        staking.register_pool(pk(1), 0).unwrap();

        assert_eq!(
            staking.register_pool(pk(1), 0).unwrap_err(),
            StakingError::AlreadyRegistered
        );

        assert_eq!(
            staking.delegate(pk(10), pk(99), 1000).unwrap_err(),
            StakingError::UnknownValidator(pk(99))
        );
    }
}
