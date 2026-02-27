//! syndicate formation and share allocation
//!
//! how shares are distributed among members:
//!
//! - **capital-weighted**: shares proportional to deposited capital
//! - **founder-controlled**: initiator gets all shares, allocates manually
//! - **equal**: same shares regardless of capital
//!
//! after formation, share policy determines if new shares can be minted
//! or if shares are fixed and only transferable.

use alloc::vec::Vec;

use crate::governance::{ShareRegistry, GovernanceRules, GovernanceError, MAX_SHARES};

/// how shares are allocated at formation
#[derive(Clone, Debug)]
pub enum FormationMode {
    /// shares proportional to capital committed
    /// first param is minimum contribution
    CapitalWeighted { min_contribution: u128 },

    /// founder gets all shares, allocates manually
    FounderControlled { founder: u32 },

    /// equal shares to all founding members
    EqualShares,
}

/// ongoing share policy after formation
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SharePolicy {
    /// shares are fixed, no new minting
    /// transfers allowed between members
    Fixed,

    /// new shares can be minted for capital at NAV
    /// dilutes existing members proportionally
    OpenEnded {
        /// minimum investment to join
        min_investment: u128,
    },

    /// shares locked, no transfers allowed
    /// members can only exit via buyout
    Locked,
}

impl Default for SharePolicy {
    fn default() -> Self {
        Self::Fixed
    }
}

/// capital commitment during formation
#[derive(Clone, Debug)]
pub struct Commitment {
    pub member_id: u32,
    /// amount committed (in base units)
    pub amount: u128,
    /// penumbra address for this member
    pub address: [u8; 80],
}

/// syndicate formation ceremony
#[derive(Clone, Debug)]
pub struct Formation {
    /// formation mode
    pub mode: FormationMode,
    /// share policy after formation
    pub policy: SharePolicy,
    /// governance rules
    pub rules: GovernanceRules,
    /// capital commitments
    commitments: Vec<Commitment>,
    /// whether formation is finalized
    finalized: bool,
}

impl Formation {
    /// start capital-weighted formation
    pub fn capital_weighted(min_contribution: u128) -> Self {
        Self {
            mode: FormationMode::CapitalWeighted { min_contribution },
            policy: SharePolicy::Fixed,
            rules: GovernanceRules::default(),
            commitments: Vec::new(),
            finalized: false,
        }
    }

    /// start founder-controlled formation
    pub fn founder_controlled(founder: u32) -> Self {
        Self {
            mode: FormationMode::FounderControlled { founder },
            policy: SharePolicy::Fixed,
            rules: GovernanceRules::default(),
            commitments: Vec::new(),
            finalized: false,
        }
    }

    /// start equal-shares formation
    pub fn equal_shares() -> Self {
        Self {
            mode: FormationMode::EqualShares,
            policy: SharePolicy::Fixed,
            rules: GovernanceRules::default(),
            commitments: Vec::new(),
            finalized: false,
        }
    }

    /// set share policy
    pub fn with_policy(mut self, policy: SharePolicy) -> Self {
        self.policy = policy;
        self
    }

    /// set governance rules
    pub fn with_rules(mut self, rules: GovernanceRules) -> Self {
        self.rules = rules;
        self
    }

    /// add capital commitment
    pub fn commit(&mut self, member_id: u32, amount: u128, address: [u8; 80]) -> Result<(), FormationError> {
        if self.finalized {
            return Err(FormationError::AlreadyFinalized);
        }

        // check minimum for capital-weighted
        if let FormationMode::CapitalWeighted { min_contribution } = &self.mode {
            if amount < *min_contribution {
                return Err(FormationError::BelowMinimum {
                    provided: amount,
                    minimum: *min_contribution,
                });
            }
        }

        // check for duplicate
        if self.commitments.iter().any(|c| c.member_id == member_id) {
            return Err(FormationError::DuplicateMember(member_id));
        }

        self.commitments.push(Commitment { member_id, amount, address });
        Ok(())
    }

    /// total capital committed
    pub fn total_committed(&self) -> u128 {
        self.commitments.iter().map(|c| c.amount).sum()
    }

    /// number of members
    pub fn member_count(&self) -> usize {
        self.commitments.len()
    }

    /// finalize formation and create share registry
    pub fn finalize(mut self) -> Result<(ShareRegistry, GovernanceRules, SharePolicy), FormationError> {
        if self.finalized {
            return Err(FormationError::AlreadyFinalized);
        }

        if self.commitments.is_empty() {
            return Err(FormationError::NoCommitments);
        }

        let registry = match &self.mode {
            FormationMode::CapitalWeighted { .. } => {
                self.allocate_capital_weighted()?
            }
            FormationMode::FounderControlled { founder } => {
                self.allocate_founder_controlled(*founder)?
            }
            FormationMode::EqualShares => {
                self.allocate_equal()?
            }
        };

        self.finalized = true;
        Ok((registry, self.rules, self.policy))
    }

    fn allocate_capital_weighted(&self) -> Result<ShareRegistry, FormationError> {
        let total = self.total_committed();
        if total == 0 {
            return Err(FormationError::NoCapital);
        }

        let mut allocations = Vec::new();
        let mut allocated = 0u32;

        // allocate proportionally, rounding down
        for (i, commitment) in self.commitments.iter().enumerate() {
            let shares = if i == self.commitments.len() - 1 {
                // last member gets remainder
                MAX_SHARES - allocated
            } else {
                let share_fraction = (commitment.amount as u128 * MAX_SHARES as u128) / total;
                share_fraction as u32
            };

            if shares > 0 {
                allocations.push((commitment.member_id, shares));
                allocated += shares;
            }
        }

        ShareRegistry::with_allocation(&allocations)
            .map_err(|e| FormationError::AllocationFailed(e))
    }

    fn allocate_founder_controlled(&self, founder: u32) -> Result<ShareRegistry, FormationError> {
        // founder gets all 100 shares
        ShareRegistry::with_allocation(&[(founder, MAX_SHARES)])
            .map_err(|e| FormationError::AllocationFailed(e))
    }

    fn allocate_equal(&self) -> Result<ShareRegistry, FormationError> {
        let n = self.commitments.len() as u32;
        if n == 0 {
            return Err(FormationError::NoCommitments);
        }

        let shares_each = MAX_SHARES / n;
        let remainder = MAX_SHARES % n;

        let mut allocations = Vec::new();
        for (i, commitment) in self.commitments.iter().enumerate() {
            // distribute remainder to first members
            let extra = if (i as u32) < remainder { 1 } else { 0 };
            allocations.push((commitment.member_id, shares_each + extra));
        }

        ShareRegistry::with_allocation(&allocations)
            .map_err(|e| FormationError::AllocationFailed(e))
    }
}

/// calculate shares for new investment at current NAV
pub fn shares_for_investment(
    investment: u128,
    current_nav: u128,
    total_shares: u32,
) -> u32 {
    if current_nav == 0 || total_shares == 0 {
        // first investment gets all shares
        return MAX_SHARES;
    }

    // new_shares / (total_shares + new_shares) = investment / (current_nav + investment)
    // solving: new_shares = total_shares * investment / current_nav
    let new_shares = (total_shares as u128 * investment) / current_nav;
    new_shares.min(MAX_SHARES as u128) as u32
}

/// calculate NAV per share
pub fn nav_per_share(total_assets: u128, total_shares: u32) -> u128 {
    if total_shares == 0 {
        return 0;
    }
    total_assets / total_shares as u128
}

/// calculate buyout value for shares
pub fn buyout_value(shares: u32, total_assets: u128, total_shares: u32) -> u128 {
    if total_shares == 0 {
        return 0;
    }
    (total_assets * shares as u128) / total_shares as u128
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FormationError {
    AlreadyFinalized,
    NoCommitments,
    NoCapital,
    BelowMinimum { provided: u128, minimum: u128 },
    DuplicateMember(u32),
    AllocationFailed(GovernanceError),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_capital_weighted_formation() {
        let mut formation = Formation::capital_weighted(100);

        formation.commit(1, 300, [0u8; 80]).unwrap();
        formation.commit(2, 500, [0u8; 80]).unwrap();
        formation.commit(3, 200, [0u8; 80]).unwrap();

        assert_eq!(formation.total_committed(), 1000);

        let (registry, _, _) = formation.finalize().unwrap();

        // 300/1000 = 30%, 500/1000 = 50%, 200/1000 = 20%
        assert_eq!(registry.shares_of(1), 30);
        assert_eq!(registry.shares_of(2), 50);
        assert_eq!(registry.shares_of(3), 20);
        assert_eq!(registry.total_issued(), 100);
    }

    #[test]
    fn test_founder_controlled_formation() {
        let mut formation = Formation::founder_controlled(1);

        formation.commit(1, 1000, [0u8; 80]).unwrap();
        formation.commit(2, 0, [0u8; 80]).unwrap();  // joining without capital

        let (registry, _, _) = formation.finalize().unwrap();

        // founder gets all shares
        assert_eq!(registry.shares_of(1), 100);
        assert_eq!(registry.shares_of(2), 0);
    }

    #[test]
    fn test_equal_shares_formation() {
        let mut formation = Formation::equal_shares();

        formation.commit(1, 1000, [0u8; 80]).unwrap();
        formation.commit(2, 100, [0u8; 80]).unwrap();
        formation.commit(3, 10, [0u8; 80]).unwrap();

        let (registry, _, _) = formation.finalize().unwrap();

        // equal regardless of capital: 100/3 = 33, 33, 34
        assert_eq!(registry.shares_of(1), 34);  // gets remainder
        assert_eq!(registry.shares_of(2), 33);
        assert_eq!(registry.shares_of(3), 33);
        assert_eq!(registry.total_issued(), 100);
    }

    #[test]
    fn test_minimum_contribution() {
        let mut formation = Formation::capital_weighted(1000);

        // below minimum fails
        let result = formation.commit(1, 500, [0u8; 80]);
        assert!(matches!(result, Err(FormationError::BelowMinimum { .. })));

        // at minimum succeeds
        formation.commit(1, 1000, [0u8; 80]).unwrap();
    }

    #[test]
    fn test_duplicate_member_rejected() {
        let mut formation = Formation::capital_weighted(100);

        formation.commit(1, 500, [0u8; 80]).unwrap();
        let result = formation.commit(1, 300, [0u8; 80]);

        assert!(matches!(result, Err(FormationError::DuplicateMember(1))));
    }

    #[test]
    fn test_shares_for_investment() {
        // existing: 100 shares, 1000 NAV
        // investing 500 should get ~33 shares (500/1500 of new total)
        let new_shares = shares_for_investment(500, 1000, 100);
        assert_eq!(new_shares, 50);  // 100 * 500 / 1000

        // first investment gets all
        let first = shares_for_investment(1000, 0, 0);
        assert_eq!(first, 100);
    }

    #[test]
    fn test_nav_calculations() {
        // 10000 assets, 100 shares = 100 per share
        assert_eq!(nav_per_share(10000, 100), 100);

        // buyout 25 shares = 2500
        assert_eq!(buyout_value(25, 10000, 100), 2500);
    }

    #[test]
    fn test_open_ended_policy() {
        let formation = Formation::capital_weighted(100)
            .with_policy(SharePolicy::OpenEnded { min_investment: 1000 });

        assert!(matches!(formation.policy, SharePolicy::OpenEnded { min_investment: 1000 }));
    }
}
