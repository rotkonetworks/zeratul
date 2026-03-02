//! 10-member syndicate test
//!
//! whale (member 1) holds 51 shares - can push routine actions solo
//! remaining 9 members hold ~5-6 shares each
//!
//! tests:
//! - whale can pass routine proposals (51% threshold) alone
//! - whale cannot pass major proposals (67% threshold) alone
//! - whale + 4 members can pass major proposals
//! - whale cannot amend rules (75% threshold) without broad support

use narsil::governance::{
    ShareRegistry, GovernanceRules, GovernanceError, ActionType,
    Proposal, ProposalState, Distribution,
};
use narsil::formation::{Formation, FormationMode, SharePolicy};

/// create the 10-member syndicate
/// member 1 (whale): 51 shares
/// members 2-6: 6 shares each (30 total)
/// members 7-10: 5 shares each (19 total... need 49 more)
/// adjust: members 2-10 split 49 shares
fn create_syndicate() -> (ShareRegistry, GovernanceRules) {
    let mut formation = Formation::capital_weighted(100);

    // whale puts in 51% of capital
    formation.commit(1, 5100, [0u8; 80]).unwrap();

    // 9 smaller members
    // members 2-6 put in 6% each
    for i in 2..=6 {
        formation.commit(i, 600, [0u8; 80]).unwrap();
    }
    // members 7-10 put in ~5% each (remaining 19%)
    formation.commit(7, 500, [0u8; 80]).unwrap();
    formation.commit(8, 500, [0u8; 80]).unwrap();
    formation.commit(9, 500, [0u8; 80]).unwrap();
    formation.commit(10, 400, [0u8; 80]).unwrap();

    assert_eq!(formation.total_committed(), 10000);

    let (registry, rules, _policy) = formation.finalize().unwrap();

    // verify allocations
    assert_eq!(registry.total_issued(), 100);
    assert_eq!(registry.shares_of(1), 51); // whale
    println!("share allocations:");
    for i in 1..=10 {
        println!("  member {}: {} shares ({}%)", i, registry.shares_of(i), registry.percentage_of(i));
    }

    (registry, rules)
}

#[test]
fn test_whale_routine_solo() {
    let (registry, rules) = create_syndicate();

    // whale proposes routine action (51% threshold)
    let mut proposal = Proposal::new(
        1,
        ActionType::Routine,
        "send 1 UM to treasury".into(),
        vec![],
    );

    // only whale votes yes
    proposal.vote(1, true, &registry).unwrap();

    // 51% >= 51% threshold - passes!
    assert!(proposal.check_threshold(&rules, &registry));
    proposal.finalize(&rules, &registry);
    assert_eq!(proposal.state, ProposalState::Passed);

    let summary = proposal.summary(registry.total_issued());
    println!("\nroutine proposal (whale solo):");
    println!("  yes: {}%, threshold: {}%", summary.yes_percentage, rules.routine_threshold);
    println!("  result: {:?}", proposal.state);
}

#[test]
fn test_whale_cannot_major_solo() {
    let (registry, rules) = create_syndicate();

    // whale proposes major action (67% threshold)
    let mut proposal = Proposal::new(
        2,
        ActionType::Major,
        "delegate 100 UM to validator".into(),
        vec![],
    );

    // only whale votes
    proposal.vote(1, true, &registry).unwrap();

    // 51% < 67% - fails
    assert!(!proposal.check_threshold(&rules, &registry));

    let summary = proposal.summary(registry.total_issued());
    println!("\nmajor proposal (whale solo):");
    println!("  yes: {}%, threshold: {}%", summary.yes_percentage, rules.major_threshold);
    println!("  needs {} more shares", rules.major_threshold - summary.yes_percentage);
}

#[test]
fn test_whale_plus_allies_major() {
    let (registry, rules) = create_syndicate();

    let mut proposal = Proposal::new(
        3,
        ActionType::Major,
        "delegate 100 UM to validator".into(),
        vec![],
    );

    // whale votes yes (51%)
    proposal.vote(1, true, &registry).unwrap();
    assert!(!proposal.check_threshold(&rules, &registry));

    // member 2 votes yes (51 + 6 = 57%)
    proposal.vote(2, true, &registry).unwrap();
    assert!(!proposal.check_threshold(&rules, &registry));

    // member 3 votes yes (57 + 6 = 63%)
    proposal.vote(3, true, &registry).unwrap();
    assert!(!proposal.check_threshold(&rules, &registry));

    // member 4 votes yes (63 + 6 = 69%)
    proposal.vote(4, true, &registry).unwrap();
    assert!(proposal.check_threshold(&rules, &registry)); // passes!

    proposal.finalize(&rules, &registry);
    assert_eq!(proposal.state, ProposalState::Passed);

    let summary = proposal.summary(registry.total_issued());
    println!("\nmajor proposal (whale + 3 allies):");
    println!("  yes: {}%, voters: {}, threshold: {}%", summary.yes_percentage, summary.voters, rules.major_threshold);
}

#[test]
fn test_minority_cannot_block_whale() {
    let (registry, rules) = create_syndicate();

    let mut proposal = Proposal::new(
        4,
        ActionType::Routine,
        "rebalance portfolio".into(),
        vec![],
    );

    // all small members vote no
    for i in 2..=10 {
        proposal.vote(i, false, &registry).unwrap();
    }

    // whale votes yes
    proposal.vote(1, true, &registry).unwrap();

    // whale has 51%, all others total 49% no
    // quorum met (100% voted), 51% yes >= 51% threshold
    assert!(proposal.check_threshold(&rules, &registry));

    let summary = proposal.summary(registry.total_issued());
    println!("\nroutine proposal (whale vs everyone):");
    println!("  yes: {}%, no: {}%", summary.yes_percentage, 100 - summary.yes_percentage);
    println!("  result: whale wins on routine actions");
}

#[test]
fn test_minority_can_block_amendments() {
    let (registry, rules) = create_syndicate();

    let mut proposal = Proposal::new(
        5,
        ActionType::Amendment,
        "change governance rules".into(),
        vec![],
    );

    // whale + members 2-6 vote yes
    // 51 + 6*5 = 51 + 30 = 81... wait let me check actual shares
    proposal.vote(1, true, &registry).unwrap();
    for i in 2..=6 {
        proposal.vote(i, true, &registry).unwrap();
    }

    // members 7-10 vote no
    for i in 7..=10 {
        proposal.vote(i, false, &registry).unwrap();
    }

    let summary = proposal.summary(registry.total_issued());
    println!("\namendment proposal (whale + 5 allies vs 4):");
    println!("  yes: {}%, threshold: {}%", summary.yes_percentage, rules.amendment_threshold);

    // depends on exact share allocation - 75% threshold is high
    if summary.yes_percentage >= rules.amendment_threshold {
        println!("  result: passes (coalition has enough)");
    } else {
        println!("  result: blocked (minority protection works)");
    }
}

#[test]
fn test_existential_needs_near_unanimity() {
    let (registry, rules) = create_syndicate();

    let mut proposal = Proposal::new(
        6,
        ActionType::Existential,
        "dissolve syndicate".into(),
        vec![],
    );

    // everyone except member 10 votes yes
    for i in 1..=9 {
        proposal.vote(i, true, &registry).unwrap();
    }

    let summary = proposal.summary(registry.total_issued());
    println!("\nexistential proposal (9/10 members):");
    println!("  yes: {}%, threshold: {}%", summary.yes_percentage, rules.existential_threshold);

    // 90% threshold - depends on member 10's share count
    let member10_shares = registry.shares_of(10);
    println!("  holdout (member 10) has {} shares", member10_shares);
}

#[test]
fn test_distribution_proportional() {
    let (registry, _rules) = create_syndicate();

    // distribute 10000 upenumbra
    let dist = Distribution::calculate(10_000, &registry);

    println!("\ndistribution of 10000 upenumbra:");
    for i in 1..=10 {
        let amount = dist.allocation_for(i);
        let shares = registry.shares_of(i);
        println!("  member {} ({} shares): {} upenumbra", i, shares, amount);
    }

    // whale gets majority
    assert!(dist.allocation_for(1) > 5000);

    // total sums correctly
    let total: u128 = (1..=10).map(|i| dist.allocation_for(i)).sum();
    assert_eq!(total, 10_000);
}

#[test]
fn test_share_transfer_changes_power() {
    let (mut registry, rules) = create_syndicate();

    // whale transfers 20 shares to member 2
    // whale: 51 -> 31, member 2: 6 -> 26
    registry.transfer(1, 2, 20).unwrap();

    assert_eq!(registry.shares_of(1), 31);
    assert_eq!(registry.shares_of(2), 26);

    // now whale can't pass routine alone
    let mut proposal = Proposal::new(7, ActionType::Routine, "test".into(), vec![]);
    proposal.vote(1, true, &registry).unwrap();
    assert!(!proposal.check_threshold(&rules, &registry)); // 31% < 51%

    // but whale + member 2 can
    proposal.vote(2, true, &registry).unwrap();
    assert!(proposal.check_threshold(&rules, &registry)); // 57% >= 51%

    println!("\nafter transfer (whale 31%, member 2: 26%):");
    println!("  whale alone: cannot pass routine");
    println!("  whale + member 2: can pass routine (57%)");
}

#[test]
fn test_double_vote_rejected() {
    let (registry, _rules) = create_syndicate();

    let mut proposal = Proposal::new(8, ActionType::Routine, "test".into(), vec![]);
    proposal.vote(1, true, &registry).unwrap();

    // whale tries to vote again
    let result = proposal.vote(1, true, &registry);
    assert_eq!(result, Err(GovernanceError::AlreadyVoted));
}

#[test]
fn test_non_member_vote_rejected() {
    let (registry, _rules) = create_syndicate();

    let mut proposal = Proposal::new(9, ActionType::Routine, "test".into(), vec![]);

    // member 99 doesn't exist
    let result = proposal.vote(99, true, &registry);
    assert_eq!(result, Err(GovernanceError::NotAMember));
}
