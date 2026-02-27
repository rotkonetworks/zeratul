//! LLC-style governance rules for syndicates
//!
//! traditional LLC structures with max 100 splittable shares.
//! ownership determines voting weight and profit distribution.
//!
//! # share model
//!
//! - max 100 shares total (can represent percentages directly)
//! - each member holds 1+ shares
//! - voting weight = shares held / total shares
//! - distributions proportional to ownership
//!
//! # action thresholds
//!
//! different actions require different approval levels:
//! - routine operations: simple majority (>50%)
//! - major decisions: supermajority (>66%)
//! - amendments: unanimous or near-unanimous (>90%)

use alloc::collections::BTreeMap;
use alloc::vec::Vec;
use alloc::string::String;

/// maximum shares in a syndicate (represents 100%)
pub const MAX_SHARES: u32 = 100;

/// member identifier (maps to osst index)
pub type MemberId = u32;

/// share registry tracking ownership
#[derive(Clone, Debug, Default)]
pub struct ShareRegistry {
    /// member_id -> shares owned
    holdings: BTreeMap<MemberId, u32>,
    /// total shares issued (<= MAX_SHARES)
    total_issued: u32,
}

impl ShareRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    /// create registry with initial allocation
    /// allocations: vec of (member_id, shares)
    pub fn with_allocation(allocations: &[(MemberId, u32)]) -> Result<Self, GovernanceError> {
        let mut registry = Self::new();
        for (member, shares) in allocations {
            registry.issue(*member, *shares)?;
        }
        Ok(registry)
    }

    /// issue shares to member (initial allocation or new issuance)
    pub fn issue(&mut self, member: MemberId, shares: u32) -> Result<(), GovernanceError> {
        if shares == 0 {
            return Err(GovernanceError::ZeroShares);
        }
        if self.total_issued + shares > MAX_SHARES {
            return Err(GovernanceError::ExceedsMaxShares {
                requested: shares,
                available: MAX_SHARES - self.total_issued,
            });
        }

        *self.holdings.entry(member).or_insert(0) += shares;
        self.total_issued += shares;
        Ok(())
    }

    /// transfer shares between members
    pub fn transfer(
        &mut self,
        from: MemberId,
        to: MemberId,
        shares: u32,
    ) -> Result<(), GovernanceError> {
        if shares == 0 {
            return Err(GovernanceError::ZeroShares);
        }

        let from_balance = self.holdings.get(&from).copied().unwrap_or(0);
        if from_balance < shares {
            return Err(GovernanceError::InsufficientShares {
                has: from_balance,
                needs: shares,
            });
        }

        // deduct from sender
        if from_balance == shares {
            self.holdings.remove(&from);
        } else {
            *self.holdings.get_mut(&from).unwrap() -= shares;
        }

        // credit to receiver
        *self.holdings.entry(to).or_insert(0) += shares;

        Ok(())
    }

    /// burn shares (member exit with buyout)
    pub fn burn(&mut self, member: MemberId, shares: u32) -> Result<(), GovernanceError> {
        let balance = self.holdings.get(&member).copied().unwrap_or(0);
        if balance < shares {
            return Err(GovernanceError::InsufficientShares {
                has: balance,
                needs: shares,
            });
        }

        if balance == shares {
            self.holdings.remove(&member);
        } else {
            *self.holdings.get_mut(&member).unwrap() -= shares;
        }
        self.total_issued -= shares;

        Ok(())
    }

    /// get shares held by member
    pub fn shares_of(&self, member: MemberId) -> u32 {
        self.holdings.get(&member).copied().unwrap_or(0)
    }

    /// get ownership percentage (0-100)
    pub fn percentage_of(&self, member: MemberId) -> u32 {
        if self.total_issued == 0 {
            return 0;
        }
        let shares = self.shares_of(member);
        (shares * 100) / self.total_issued
    }

    /// get voting weight as fraction (shares / total)
    pub fn weight_of(&self, member: MemberId) -> (u32, u32) {
        (self.shares_of(member), self.total_issued)
    }

    /// total shares issued
    pub fn total_issued(&self) -> u32 {
        self.total_issued
    }

    /// shares available for new issuance
    pub fn available(&self) -> u32 {
        MAX_SHARES - self.total_issued
    }

    /// number of members
    pub fn member_count(&self) -> usize {
        self.holdings.len()
    }

    /// iterate over all members and their holdings
    pub fn members(&self) -> impl Iterator<Item = (MemberId, u32)> + '_ {
        self.holdings.iter().map(|(&id, &shares)| (id, shares))
    }

    /// check if member exists
    pub fn is_member(&self, member: MemberId) -> bool {
        self.holdings.contains_key(&member)
    }
}

/// types of actions requiring approval
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ActionType {
    /// routine operations (transfers, small expenditures)
    Routine,
    /// major decisions (large investments, new contracts)
    Major,
    /// structural changes (add/remove members, amend rules)
    Amendment,
    /// existential (dissolve syndicate, merge)
    Existential,
}

/// governance rules for the syndicate
#[derive(Clone, Debug)]
pub struct GovernanceRules {
    /// threshold for routine operations (percentage)
    pub routine_threshold: u32,
    /// threshold for major decisions
    pub major_threshold: u32,
    /// threshold for amendments
    pub amendment_threshold: u32,
    /// threshold for existential decisions
    pub existential_threshold: u32,
    /// require all members to vote (quorum) or just threshold
    pub require_quorum: bool,
    /// minimum quorum percentage (if require_quorum)
    pub quorum_percentage: u32,
    /// allow share transfers
    pub transfers_allowed: bool,
    /// require approval for transfers
    pub transfer_approval_required: bool,
}

impl Default for GovernanceRules {
    fn default() -> Self {
        Self {
            routine_threshold: 51,      // simple majority
            major_threshold: 67,        // supermajority
            amendment_threshold: 75,    // 3/4
            existential_threshold: 90,  // near-unanimous
            require_quorum: true,
            quorum_percentage: 50,
            transfers_allowed: true,
            transfer_approval_required: true,
        }
    }
}

impl GovernanceRules {
    /// strict rules (higher thresholds)
    pub fn strict() -> Self {
        Self {
            routine_threshold: 67,
            major_threshold: 75,
            amendment_threshold: 90,
            existential_threshold: 100,  // unanimous
            require_quorum: true,
            quorum_percentage: 67,
            transfers_allowed: true,
            transfer_approval_required: true,
        }
    }

    /// permissive rules (lower thresholds)
    pub fn permissive() -> Self {
        Self {
            routine_threshold: 51,
            major_threshold: 51,
            amendment_threshold: 67,
            existential_threshold: 75,
            require_quorum: false,
            quorum_percentage: 0,
            transfers_allowed: true,
            transfer_approval_required: false,
        }
    }

    /// get threshold for action type
    pub fn threshold_for(&self, action: ActionType) -> u32 {
        match action {
            ActionType::Routine => self.routine_threshold,
            ActionType::Major => self.major_threshold,
            ActionType::Amendment => self.amendment_threshold,
            ActionType::Existential => self.existential_threshold,
        }
    }
}

/// a proposal awaiting votes
#[derive(Clone, Debug)]
pub struct Proposal {
    /// unique proposal id
    pub id: u64,
    /// type of action
    pub action_type: ActionType,
    /// description
    pub description: String,
    /// encoded action data
    pub data: Vec<u8>,
    /// votes received: member_id -> (approve, shares_at_time)
    votes: BTreeMap<MemberId, (bool, u32)>,
    /// total shares that voted yes
    yes_shares: u32,
    /// total shares that voted no
    no_shares: u32,
    /// proposal state
    pub state: ProposalState,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ProposalState {
    /// accepting votes
    Open,
    /// passed threshold, ready to execute
    Passed,
    /// failed to reach threshold
    Failed,
    /// executed
    Executed,
    /// cancelled by proposer
    Cancelled,
}

impl Proposal {
    pub fn new(id: u64, action_type: ActionType, description: String, data: Vec<u8>) -> Self {
        Self {
            id,
            action_type,
            description,
            data,
            votes: BTreeMap::new(),
            yes_shares: 0,
            no_shares: 0,
            state: ProposalState::Open,
        }
    }

    /// cast vote (weighted by shares)
    pub fn vote(
        &mut self,
        member: MemberId,
        approve: bool,
        registry: &ShareRegistry,
    ) -> Result<(), GovernanceError> {
        if self.state != ProposalState::Open {
            return Err(GovernanceError::ProposalNotOpen);
        }

        if self.votes.contains_key(&member) {
            return Err(GovernanceError::AlreadyVoted);
        }

        let shares = registry.shares_of(member);
        if shares == 0 {
            return Err(GovernanceError::NotAMember);
        }

        self.votes.insert(member, (approve, shares));
        if approve {
            self.yes_shares += shares;
        } else {
            self.no_shares += shares;
        }

        Ok(())
    }

    /// check if proposal has reached threshold
    pub fn check_threshold(&self, rules: &GovernanceRules, registry: &ShareRegistry) -> bool {
        let total = registry.total_issued();
        if total == 0 {
            return false;
        }

        // check quorum if required
        if rules.require_quorum {
            let voted = self.yes_shares + self.no_shares;
            let quorum_needed = (total * rules.quorum_percentage) / 100;
            if voted < quorum_needed {
                return false;
            }
        }

        // check threshold
        let threshold = rules.threshold_for(self.action_type);
        let yes_percentage = (self.yes_shares * 100) / total;
        yes_percentage >= threshold
    }

    /// finalize proposal based on votes
    pub fn finalize(&mut self, rules: &GovernanceRules, registry: &ShareRegistry) {
        if self.state != ProposalState::Open {
            return;
        }

        if self.check_threshold(rules, registry) {
            self.state = ProposalState::Passed;
        } else {
            self.state = ProposalState::Failed;
        }
    }

    /// mark as executed
    pub fn mark_executed(&mut self) {
        if self.state == ProposalState::Passed {
            self.state = ProposalState::Executed;
        }
    }

    /// get vote summary
    pub fn summary(&self, total_shares: u32) -> VoteSummary {
        VoteSummary {
            yes_shares: self.yes_shares,
            no_shares: self.no_shares,
            total_shares,
            voters: self.votes.len() as u32,
            yes_percentage: if total_shares > 0 {
                (self.yes_shares * 100) / total_shares
            } else {
                0
            },
        }
    }
}

#[derive(Clone, Debug)]
pub struct VoteSummary {
    pub yes_shares: u32,
    pub no_shares: u32,
    pub total_shares: u32,
    pub voters: u32,
    pub yes_percentage: u32,
}

/// distribution calculation
#[derive(Clone, Debug)]
pub struct Distribution {
    /// amount to distribute
    pub amount: u128,
    /// per-member amounts
    pub allocations: BTreeMap<MemberId, u128>,
}

impl Distribution {
    /// calculate pro-rata distribution based on share ownership
    pub fn calculate(amount: u128, registry: &ShareRegistry) -> Self {
        let total = registry.total_issued() as u128;
        let mut allocations = BTreeMap::new();
        let mut distributed = 0u128;

        // calculate each member's share
        let members: Vec<_> = registry.members().collect();
        for (i, (member, shares)) in members.iter().enumerate() {
            let share_amount = if i == members.len() - 1 {
                // last member gets remainder to avoid rounding errors
                amount - distributed
            } else {
                (amount * (*shares as u128)) / total
            };
            allocations.insert(*member, share_amount);
            distributed += share_amount;
        }

        Self { amount, allocations }
    }

    /// get allocation for specific member
    pub fn allocation_for(&self, member: MemberId) -> u128 {
        self.allocations.get(&member).copied().unwrap_or(0)
    }
}

/// governance errors
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum GovernanceError {
    ZeroShares,
    ExceedsMaxShares { requested: u32, available: u32 },
    InsufficientShares { has: u32, needs: u32 },
    NotAMember,
    AlreadyVoted,
    ProposalNotOpen,
    TransferNotAllowed,
    TransferRequiresApproval,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_share_registry_basic() {
        let mut registry = ShareRegistry::new();

        registry.issue(1, 30).unwrap();
        registry.issue(2, 25).unwrap();
        registry.issue(3, 45).unwrap();

        assert_eq!(registry.total_issued(), 100);
        assert_eq!(registry.shares_of(1), 30);
        assert_eq!(registry.shares_of(2), 25);
        assert_eq!(registry.shares_of(3), 45);
        assert_eq!(registry.percentage_of(1), 30);
    }

    #[test]
    fn test_share_registry_transfer() {
        let mut registry = ShareRegistry::with_allocation(&[(1, 50), (2, 50)]).unwrap();

        registry.transfer(1, 2, 20).unwrap();

        assert_eq!(registry.shares_of(1), 30);
        assert_eq!(registry.shares_of(2), 70);
    }

    #[test]
    fn test_share_registry_exceeds_max() {
        let mut registry = ShareRegistry::new();
        registry.issue(1, 80).unwrap();

        let result = registry.issue(2, 30);
        assert!(matches!(
            result,
            Err(GovernanceError::ExceedsMaxShares { requested: 30, available: 20 })
        ));
    }

    #[test]
    fn test_proposal_voting() {
        let registry = ShareRegistry::with_allocation(&[
            (1, 30),
            (2, 25),
            (3, 45),
        ]).unwrap();
        let rules = GovernanceRules::default();

        let mut proposal = Proposal::new(
            1,
            ActionType::Routine,
            "test proposal".into(),
            vec![],
        );

        // member 1 (30%) votes yes
        proposal.vote(1, true, &registry).unwrap();
        assert!(!proposal.check_threshold(&rules, &registry)); // 30% < 51%

        // member 3 (45%) votes yes
        proposal.vote(3, true, &registry).unwrap();
        assert!(proposal.check_threshold(&rules, &registry)); // 75% >= 51%
    }

    #[test]
    fn test_proposal_supermajority() {
        let registry = ShareRegistry::with_allocation(&[
            (1, 34),
            (2, 33),
            (3, 33),
        ]).unwrap();
        let rules = GovernanceRules::default();

        let mut proposal = Proposal::new(
            1,
            ActionType::Major,  // requires 67%
            "major decision".into(),
            vec![],
        );

        proposal.vote(1, true, &registry).unwrap();
        proposal.vote(2, true, &registry).unwrap();
        // 67% exactly
        assert!(proposal.check_threshold(&rules, &registry));
    }

    #[test]
    fn test_distribution_calculation() {
        let registry = ShareRegistry::with_allocation(&[
            (1, 50),
            (2, 30),
            (3, 20),
        ]).unwrap();

        let dist = Distribution::calculate(1000, &registry);

        assert_eq!(dist.allocation_for(1), 500);
        assert_eq!(dist.allocation_for(2), 300);
        assert_eq!(dist.allocation_for(3), 200);

        // verify total
        let total: u128 = dist.allocations.values().sum();
        assert_eq!(total, 1000);
    }

    #[test]
    fn test_distribution_odd_amounts() {
        let registry = ShareRegistry::with_allocation(&[
            (1, 33),
            (2, 33),
            (3, 34),
        ]).unwrap();

        // 100 can't be evenly split 3 ways
        let dist = Distribution::calculate(100, &registry);

        let total: u128 = dist.allocations.values().sum();
        assert_eq!(total, 100); // should still sum to exact amount
    }

    #[test]
    fn test_governance_rules_thresholds() {
        let rules = GovernanceRules::default();

        assert_eq!(rules.threshold_for(ActionType::Routine), 51);
        assert_eq!(rules.threshold_for(ActionType::Major), 67);
        assert_eq!(rules.threshold_for(ActionType::Amendment), 75);
        assert_eq!(rules.threshold_for(ActionType::Existential), 90);
    }

    #[test]
    fn test_quorum_requirement() {
        let registry = ShareRegistry::with_allocation(&[
            (1, 25),
            (2, 25),
            (3, 25),
            (4, 25),
        ]).unwrap();

        let rules = GovernanceRules {
            require_quorum: true,
            quorum_percentage: 50,
            routine_threshold: 51,
            ..Default::default()
        };

        let mut proposal = Proposal::new(1, ActionType::Routine, "test".into(), vec![]);

        // only member 1 votes (25% participation)
        proposal.vote(1, true, &registry).unwrap();
        assert!(!proposal.check_threshold(&rules, &registry)); // quorum not met

        // member 2 also votes (50% participation, but only 50% yes - needs 51%)
        proposal.vote(2, true, &registry).unwrap();
        assert!(!proposal.check_threshold(&rules, &registry)); // quorum met but 50% < 51%

        // member 3 votes yes (75% participation, 75% yes)
        proposal.vote(3, true, &registry).unwrap();
        assert!(proposal.check_threshold(&rules, &registry)); // passes
    }
}
