//! coordinator selection for relay duties
//!
//! someone needs to aggregate contributions and post to relays.
//! by default, the member with most shares takes this role - they
//! have the most at stake and incentive to keep things running.
//!
//! if primary coordinator is unresponsive, duties fall through to
//! the next largest shareholder, and so on.
//!
//! # coordinator responsibilities
//!
//! - aggregate osst contributions into final signature
//! - post signed transactions to relay
//! - broadcast proposal announcements
//! - collect and tally votes
//!
//! # selection algorithm
//!
//! 1. sort members by share count (descending)
//! 2. on ties, use lexicographic pubkey ordering (deterministic)
//! 3. primary = first, fallbacks = rest in order
//!
//! # failover
//!
//! if primary doesn't act within timeout, any member can assume
//! coordinator role. the protocol is safe regardless of who
//! coordinates - it just needs someone to do it.

use alloc::vec::Vec;
use alloc::collections::BTreeMap;

use crate::wire::Hash32;

/// coordinator selection strategy
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SelectionStrategy {
    /// largest shareholder coordinates (default)
    LargestHolder,
    /// rotate through members each round
    RoundRobin,
    /// designated member always coordinates
    Designated,
}

/// coordinator selection based on share ownership
#[derive(Clone, Debug)]
pub struct CoordinatorSelector {
    /// members sorted by shares (descending), then by pubkey
    ranked_members: Vec<RankedMember>,
    /// current coordinator index (0 = primary)
    current_idx: usize,
    /// timeout before failover (in seconds)
    failover_timeout: u64,
    /// last activity timestamp from coordinator
    last_activity: u64,
    /// selection strategy
    strategy: SelectionStrategy,
    /// designated coordinator (if strategy == Designated)
    designated: Option<[u8; 32]>,
    /// round counter for round-robin
    round_counter: u64,
}

/// member with ranking info
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RankedMember {
    /// member pubkey
    pub pubkey: [u8; 32],
    /// share count
    pub shares: u8,
    /// rank (0 = highest shares)
    pub rank: usize,
}

impl CoordinatorSelector {
    /// create selector from share ownership map (default: largest holder)
    pub fn new(share_ownership: &BTreeMap<[u8; 32], u8>) -> Self {
        let mut members: Vec<_> = share_ownership
            .iter()
            .map(|(pk, shares)| (*pk, *shares))
            .collect();

        // sort by shares descending, then pubkey ascending for determinism
        members.sort_by(|a, b| {
            b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0))
        });

        let ranked_members = members
            .into_iter()
            .enumerate()
            .map(|(rank, (pubkey, shares))| RankedMember { pubkey, shares, rank })
            .collect();

        Self {
            ranked_members,
            current_idx: 0,
            failover_timeout: 60, // 1 minute default
            last_activity: 0,
            strategy: SelectionStrategy::LargestHolder,
            designated: None,
            round_counter: 0,
        }
    }

    /// use round-robin strategy (rotate through members)
    pub fn with_round_robin(mut self) -> Self {
        self.strategy = SelectionStrategy::RoundRobin;
        self
    }

    /// designate a specific member as coordinator
    pub fn with_designated(mut self, pubkey: [u8; 32]) -> Self {
        self.strategy = SelectionStrategy::Designated;
        self.designated = Some(pubkey);
        // move designated member to front of current_idx
        if let Some(idx) = self.ranked_members.iter().position(|m| m.pubkey == pubkey) {
            self.current_idx = idx;
        }
        self
    }

    /// change coordinator dynamically
    pub fn designate(&mut self, pubkey: [u8; 32]) -> bool {
        if let Some(idx) = self.ranked_members.iter().position(|m| m.pubkey == pubkey) {
            self.designated = Some(pubkey);
            self.current_idx = idx;
            self.last_activity = 0;
            true
        } else {
            false
        }
    }

    /// advance to next round (for round-robin)
    pub fn next_round(&mut self) {
        self.round_counter += 1;
        if self.strategy == SelectionStrategy::RoundRobin {
            self.current_idx = (self.round_counter as usize) % self.ranked_members.len();
            self.last_activity = 0;
        }
    }

    /// get current strategy
    pub fn strategy(&self) -> SelectionStrategy {
        self.strategy
    }

    /// create from share registry
    pub fn from_registry(registry: &crate::shares::ShareRegistry) -> Self {
        let ownership = registry.owners()
            .iter()
            .map(|pk| (*pk, registry.share_count(pk)))
            .collect();
        Self::new(&ownership)
    }

    /// set failover timeout
    pub fn with_timeout(mut self, seconds: u64) -> Self {
        self.failover_timeout = seconds;
        self
    }

    /// get current coordinator pubkey
    pub fn current(&self) -> Option<&[u8; 32]> {
        self.ranked_members.get(self.current_idx).map(|m| &m.pubkey)
    }

    /// get primary coordinator (highest shares)
    pub fn primary(&self) -> Option<&[u8; 32]> {
        self.ranked_members.first().map(|m| &m.pubkey)
    }

    /// get all coordinators in priority order
    pub fn priority_order(&self) -> Vec<&[u8; 32]> {
        self.ranked_members.iter().map(|m| &m.pubkey).collect()
    }

    /// check if pubkey is current coordinator
    pub fn is_coordinator(&self, pubkey: &[u8; 32]) -> bool {
        self.current().map(|c| c == pubkey).unwrap_or(false)
    }

    /// check if pubkey is in coordinator lineup
    pub fn is_potential_coordinator(&self, pubkey: &[u8; 32]) -> bool {
        self.ranked_members.iter().any(|m| &m.pubkey == pubkey)
    }

    /// get rank for pubkey (0 = highest shares)
    pub fn rank_of(&self, pubkey: &[u8; 32]) -> Option<usize> {
        self.ranked_members.iter()
            .find(|m| &m.pubkey == pubkey)
            .map(|m| m.rank)
    }

    /// record activity from current coordinator
    pub fn record_activity(&mut self, timestamp: u64) {
        self.last_activity = timestamp;
    }

    /// check if coordinator has timed out
    pub fn is_timed_out(&self, now: u64) -> bool {
        if self.last_activity == 0 {
            return false; // no activity yet, give them a chance
        }
        now.saturating_sub(self.last_activity) > self.failover_timeout
    }

    /// failover to next coordinator
    pub fn failover(&mut self) -> Option<&[u8; 32]> {
        if self.current_idx + 1 < self.ranked_members.len() {
            self.current_idx += 1;
            self.last_activity = 0; // reset for new coordinator
            self.current()
        } else {
            None // no more fallbacks
        }
    }

    /// reset to primary coordinator
    pub fn reset(&mut self) {
        self.current_idx = 0;
        self.last_activity = 0;
    }

    /// check if we should attempt coordination (based on rank and timeout)
    ///
    /// returns true if:
    /// - we are the current coordinator, or
    /// - current coordinator timed out and we're next in line
    pub fn should_coordinate(&self, our_pubkey: &[u8; 32], now: u64) -> bool {
        if self.is_coordinator(our_pubkey) {
            return true;
        }

        // check if we're next in failover chain
        if let Some(our_rank) = self.rank_of(our_pubkey) {
            if self.is_timed_out(now) && our_rank == self.current_idx + 1 {
                return true;
            }
        }

        false
    }

    /// get coordinator duties for a round
    pub fn duties(&self) -> CoordinatorDuties {
        CoordinatorDuties {
            coordinator: self.current().copied(),
            fallbacks: self.ranked_members
                .iter()
                .skip(self.current_idx + 1)
                .map(|m| m.pubkey)
                .collect(),
            timeout: self.failover_timeout,
        }
    }

    /// number of potential coordinators
    pub fn coordinator_count(&self) -> usize {
        self.ranked_members.len()
    }
}

/// coordinator duties for a round
#[derive(Clone, Debug)]
pub struct CoordinatorDuties {
    /// current coordinator
    pub coordinator: Option<[u8; 32]>,
    /// fallback coordinators in order
    pub fallbacks: Vec<[u8; 32]>,
    /// timeout before failover
    pub timeout: u64,
}

/// coordinator role a member can take
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CoordinatorRole {
    /// primary coordinator (most shares)
    Primary,
    /// backup coordinator (nth in line)
    Backup(usize),
    /// not a coordinator (but can still participate)
    Participant,
}

impl CoordinatorRole {
    /// check if this is an active coordinator role
    pub fn is_coordinator(&self) -> bool {
        matches!(self, CoordinatorRole::Primary | CoordinatorRole::Backup(_))
    }
}

/// round coordinator state
#[derive(Clone, Debug)]
pub struct RoundCoordinator {
    /// round/proposal id
    pub round_id: Hash32,
    /// selector for this round
    selector: CoordinatorSelector,
    /// collected contributions
    contributions: Vec<CollectedItem>,
    /// collected votes
    votes: Vec<CollectedItem>,
    /// phase
    phase: CoordinatorPhase,
}

#[derive(Clone, Debug)]
struct CollectedItem {
    from: [u8; 32],
    data: Vec<u8>,
    received_at: u64,
}

/// coordinator phase
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CoordinatorPhase {
    /// collecting votes
    Voting,
    /// collecting osst contributions
    Signing,
    /// aggregating and broadcasting
    Aggregating,
    /// done
    Complete,
}

impl RoundCoordinator {
    /// create coordinator for a round
    pub fn new(round_id: Hash32, selector: CoordinatorSelector) -> Self {
        Self {
            round_id,
            selector,
            contributions: Vec::new(),
            votes: Vec::new(),
            phase: CoordinatorPhase::Voting,
        }
    }

    /// get current phase
    pub fn phase(&self) -> CoordinatorPhase {
        self.phase
    }

    /// advance to next phase
    pub fn advance(&mut self) {
        self.phase = match self.phase {
            CoordinatorPhase::Voting => CoordinatorPhase::Signing,
            CoordinatorPhase::Signing => CoordinatorPhase::Aggregating,
            CoordinatorPhase::Aggregating => CoordinatorPhase::Complete,
            CoordinatorPhase::Complete => CoordinatorPhase::Complete,
        };
    }

    /// add vote from member
    pub fn add_vote(&mut self, from: [u8; 32], vote_data: Vec<u8>, timestamp: u64) {
        if !self.votes.iter().any(|v| v.from == from) {
            self.votes.push(CollectedItem {
                from,
                data: vote_data,
                received_at: timestamp,
            });
        }
    }

    /// add contribution from member
    pub fn add_contribution(&mut self, from: [u8; 32], contrib_data: Vec<u8>, timestamp: u64) {
        if !self.contributions.iter().any(|c| c.from == from) {
            self.contributions.push(CollectedItem {
                from,
                data: contrib_data,
                received_at: timestamp,
            });
        }
    }

    /// get vote count
    pub fn vote_count(&self) -> usize {
        self.votes.len()
    }

    /// get contribution count
    pub fn contribution_count(&self) -> usize {
        self.contributions.len()
    }

    /// get all contribution data
    pub fn contributions(&self) -> Vec<(&[u8; 32], &[u8])> {
        self.contributions.iter()
            .map(|c| (&c.from, c.data.as_slice()))
            .collect()
    }

    /// get all vote data
    pub fn votes(&self) -> Vec<(&[u8; 32], &[u8])> {
        self.votes.iter()
            .map(|v| (&v.from, v.data.as_slice()))
            .collect()
    }

    /// check if we have enough contributions for threshold
    pub fn has_threshold(&self, threshold_shares: u8) -> bool {
        // sum shares from contributors
        let share_sum: u8 = self.contributions.iter()
            .filter_map(|c| {
                self.selector.ranked_members.iter()
                    .find(|m| m.pubkey == c.from)
                    .map(|m| m.shares)
            })
            .sum();
        share_sum >= threshold_shares
    }

    /// get coordinator selector
    pub fn selector(&self) -> &CoordinatorSelector {
        &self.selector
    }

    /// get mutable coordinator selector
    pub fn selector_mut(&mut self) -> &mut CoordinatorSelector {
        &mut self.selector
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_ownership() -> BTreeMap<[u8; 32], u8> {
        let mut map = BTreeMap::new();
        map.insert([1u8; 32], 40); // alice - most shares
        map.insert([2u8; 32], 35); // bob
        map.insert([3u8; 32], 25); // carol
        map
    }

    #[test]
    fn test_coordinator_selection() {
        let selector = CoordinatorSelector::new(&make_ownership());

        // alice should be primary (most shares)
        assert_eq!(selector.primary(), Some(&[1u8; 32]));
        assert_eq!(selector.current(), Some(&[1u8; 32]));
        assert!(selector.is_coordinator(&[1u8; 32]));
        assert!(!selector.is_coordinator(&[2u8; 32]));
    }

    #[test]
    fn test_rank_ordering() {
        let selector = CoordinatorSelector::new(&make_ownership());

        assert_eq!(selector.rank_of(&[1u8; 32]), Some(0)); // alice = rank 0
        assert_eq!(selector.rank_of(&[2u8; 32]), Some(1)); // bob = rank 1
        assert_eq!(selector.rank_of(&[3u8; 32]), Some(2)); // carol = rank 2
        assert_eq!(selector.rank_of(&[4u8; 32]), None);    // unknown
    }

    #[test]
    fn test_priority_order() {
        let selector = CoordinatorSelector::new(&make_ownership());
        let order = selector.priority_order();

        assert_eq!(order.len(), 3);
        assert_eq!(order[0], &[1u8; 32]); // alice first
        assert_eq!(order[1], &[2u8; 32]); // bob second
        assert_eq!(order[2], &[3u8; 32]); // carol third
    }

    #[test]
    fn test_failover() {
        let mut selector = CoordinatorSelector::new(&make_ownership());

        assert_eq!(selector.current(), Some(&[1u8; 32])); // alice

        // failover to bob
        let next = selector.failover();
        assert_eq!(next, Some(&[2u8; 32]));
        assert_eq!(selector.current(), Some(&[2u8; 32]));

        // failover to carol
        let next = selector.failover();
        assert_eq!(next, Some(&[3u8; 32]));

        // no more fallbacks
        let next = selector.failover();
        assert!(next.is_none());
    }

    #[test]
    fn test_timeout_detection() {
        let mut selector = CoordinatorSelector::new(&make_ownership())
            .with_timeout(60);

        // no activity yet - not timed out
        assert!(!selector.is_timed_out(100));

        // record activity
        selector.record_activity(100);
        assert!(!selector.is_timed_out(150)); // 50s elapsed, not timed out

        // 61s elapsed - timed out
        assert!(selector.is_timed_out(161));
    }

    #[test]
    fn test_should_coordinate() {
        let mut selector = CoordinatorSelector::new(&make_ownership())
            .with_timeout(60);

        // alice should coordinate (she's primary)
        assert!(selector.should_coordinate(&[1u8; 32], 100));

        // bob shouldn't yet
        assert!(!selector.should_coordinate(&[2u8; 32], 100));

        // record alice activity at t=100
        selector.record_activity(100);

        // at t=150, alice hasn't timed out yet (50s < 60s)
        assert!(!selector.should_coordinate(&[2u8; 32], 150));

        // at t=161, alice has timed out (61s > 60s), bob should take over
        assert!(selector.should_coordinate(&[2u8; 32], 161));

        // carol still shouldn't (bob is next, not her)
        assert!(!selector.should_coordinate(&[3u8; 32], 161));
    }

    #[test]
    fn test_tie_breaking() {
        let mut map = BTreeMap::new();
        map.insert([2u8; 32], 50); // bob
        map.insert([1u8; 32], 50); // alice (same shares, lower pubkey)
        map.insert([3u8; 32], 50); // carol

        let selector = CoordinatorSelector::new(&map);

        // with equal shares, lower pubkey wins
        assert_eq!(selector.rank_of(&[1u8; 32]), Some(0)); // alice first
        assert_eq!(selector.rank_of(&[2u8; 32]), Some(1)); // bob second
        assert_eq!(selector.rank_of(&[3u8; 32]), Some(2)); // carol third
    }

    #[test]
    fn test_reset() {
        let mut selector = CoordinatorSelector::new(&make_ownership());

        selector.failover();
        selector.failover();
        assert_eq!(selector.current(), Some(&[3u8; 32])); // carol

        selector.reset();
        assert_eq!(selector.current(), Some(&[1u8; 32])); // back to alice
    }

    #[test]
    fn test_round_coordinator() {
        let selector = CoordinatorSelector::new(&make_ownership());
        let mut coord = RoundCoordinator::new([0u8; 32], selector);

        assert_eq!(coord.phase(), CoordinatorPhase::Voting);

        // add votes
        coord.add_vote([1u8; 32], vec![1, 2, 3], 100);
        coord.add_vote([2u8; 32], vec![4, 5, 6], 101);
        assert_eq!(coord.vote_count(), 2);

        // duplicate vote ignored
        coord.add_vote([1u8; 32], vec![7, 8, 9], 102);
        assert_eq!(coord.vote_count(), 2);

        coord.advance();
        assert_eq!(coord.phase(), CoordinatorPhase::Signing);

        // add contributions
        coord.add_contribution([1u8; 32], vec![10, 11], 200);
        coord.add_contribution([2u8; 32], vec![12, 13], 201);
        assert_eq!(coord.contribution_count(), 2);

        // check threshold (alice=40 + bob=35 = 75 shares)
        assert!(coord.has_threshold(67));
        assert!(!coord.has_threshold(80));
    }

    #[test]
    fn test_duties() {
        let selector = CoordinatorSelector::new(&make_ownership());
        let duties = selector.duties();

        assert_eq!(duties.coordinator, Some([1u8; 32]));
        assert_eq!(duties.fallbacks.len(), 2);
        assert_eq!(duties.fallbacks[0], [2u8; 32]);
        assert_eq!(duties.fallbacks[1], [3u8; 32]);
    }

    #[test]
    fn test_designated_coordinator() {
        // designate carol (smallest holder) as coordinator
        let selector = CoordinatorSelector::new(&make_ownership())
            .with_designated([3u8; 32]);

        assert_eq!(selector.strategy(), SelectionStrategy::Designated);
        assert_eq!(selector.current(), Some(&[3u8; 32])); // carol is now coordinator
        assert!(selector.is_coordinator(&[3u8; 32]));
        assert!(!selector.is_coordinator(&[1u8; 32])); // alice isn't anymore
    }

    #[test]
    fn test_dynamic_designation() {
        let mut selector = CoordinatorSelector::new(&make_ownership());

        // initially alice (most shares)
        assert_eq!(selector.current(), Some(&[1u8; 32]));

        // dynamically designate bob
        assert!(selector.designate([2u8; 32]));
        assert_eq!(selector.current(), Some(&[2u8; 32]));

        // can't designate unknown member
        assert!(!selector.designate([99u8; 32]));
    }

    #[test]
    fn test_round_robin() {
        let mut selector = CoordinatorSelector::new(&make_ownership())
            .with_round_robin();

        assert_eq!(selector.strategy(), SelectionStrategy::RoundRobin);

        // round 0: first member (alice - rank 0)
        assert_eq!(selector.current(), Some(&[1u8; 32]));

        // round 1: second member (bob - rank 1)
        selector.next_round();
        assert_eq!(selector.current(), Some(&[2u8; 32]));

        // round 2: third member (carol - rank 2)
        selector.next_round();
        assert_eq!(selector.current(), Some(&[3u8; 32]));

        // round 3: wraps to first (alice)
        selector.next_round();
        assert_eq!(selector.current(), Some(&[1u8; 32]));
    }

    #[test]
    fn test_strategy_default() {
        let selector = CoordinatorSelector::new(&make_ownership());
        assert_eq!(selector.strategy(), SelectionStrategy::LargestHolder);
    }
}
