//! integration tests for narsil syndicate flow
//!
//! tests the complete lifecycle:
//! 1. formation ceremony
//! 2. proposal creation
//! 3. voting
//! 4. osst contribution aggregation

use narsil::{
    // ceremony
    FormationCeremony, CeremonyPhase, JoiningMember, DkgCommitment, DkgShare,
    // client
    MemberStorage, ProposalBuilder, SyndicateClient,
    // state
    SyndicateStateManager,
    // aggregator
    ContributionCollector, BatchedAggregator,
    // wire types
    WireGovernanceRules, VoteType, SignedContribution,
    WireContribution, WireProposalKind,
    // crypto
    MemberCrypto,
};

/// helper to create a test syndicate with 3 members
fn setup_test_syndicate() -> (
    [u8; 32],                    // syndicate_id
    Vec<([u8; 32], [u8; 32])>,   // (pubkey, secret) for each member
) {
    let syndicate_id = [1u8; 32];
    let members = vec![
        ([10u8; 32], [11u8; 32]), // alice: pubkey, secret
        ([20u8; 32], [21u8; 32]), // bob
        ([30u8; 32], [31u8; 32]), // carol
    ];
    (syndicate_id, members)
}

#[test]
fn test_formation_ceremony_full_flow() {
    let mut ceremony = FormationCeremony::new(WireGovernanceRules::default(), 67);

    // add 3 members
    ceremony.add_member(JoiningMember {
        pubkey: [1u8; 32],
        viewing_key: [11u8; 32],
        name: "alice".into(),
        shares: vec![],
    }).unwrap();

    ceremony.add_member(JoiningMember {
        pubkey: [2u8; 32],
        viewing_key: [22u8; 32],
        name: "bob".into(),
        shares: vec![],
    }).unwrap();

    ceremony.add_member(JoiningMember {
        pubkey: [3u8; 32],
        viewing_key: [33u8; 32],
        name: "carol".into(),
        shares: vec![],
    }).unwrap();

    // allocate shares equally
    ceremony.allocate_equal().unwrap();

    // verify allocation (100 / 3 = 33 each, +1 to first)
    assert_eq!(ceremony.members[0].shares.len(), 34);
    assert_eq!(ceremony.members[1].shares.len(), 33);
    assert_eq!(ceremony.members[2].shares.len(), 33);

    // start committing phase
    ceremony.start_committing().unwrap();
    assert_eq!(ceremony.phase, CeremonyPhase::Committing);

    // add commitments
    for member in &[[1u8; 32], [2u8; 32], [3u8; 32]] {
        ceremony.add_commitment(DkgCommitment {
            pubkey: *member,
            commitment: [member[0] * 10; 32],
            verification_points: vec![],
        }).unwrap();
    }

    assert_eq!(ceremony.phase, CeremonyPhase::Sharing);

    // add shares (each member sends to each other)
    let members = [[1u8; 32], [2u8; 32], [3u8; 32]];
    for from in &members {
        for to in &members {
            if from != to {
                ceremony.add_share(DkgShare {
                    from: *from,
                    to: *to,
                    share_indices: vec![1],
                    encrypted_data: vec![from[0], to[0]],
                }).unwrap();
            }
        }
    }

    assert_eq!(ceremony.phase, CeremonyPhase::Finalizing);

    // finalize
    let result = ceremony.finalize().unwrap();

    assert!(ceremony.is_complete());
    assert_eq!(result.threshold, 67);
    assert_eq!(result.members.len(), 3);
    assert_eq!(result.backup_packages.len(), 3);
}

#[test]
fn test_proposal_vote_flow() {
    let (syndicate_id, members) = setup_test_syndicate();
    let (alice_pk, alice_secret) = members[0];
    let alice_viewing = [12u8; 32];

    // create state manager
    let mut state = SyndicateStateManager::new(syndicate_id, WireGovernanceRules::default());

    // add alice with 40 shares
    state.add_member(alice_pk, "alice".into(), (1..=40).collect()).unwrap();

    // add bob with 30 shares
    let (bob_pk, _) = members[1];
    state.add_member(bob_pk, "bob".into(), (41..=70).collect()).unwrap();

    // add carol with 30 shares
    let (carol_pk, _) = members[2];
    state.add_member(carol_pk, "carol".into(), (71..=100).collect()).unwrap();

    // create alice's client
    let storage = MemberStorage::new(
        syndicate_id,
        alice_pk,
        alice_viewing,
        (1..=40).collect(),
    );

    let mut client = SyndicateClient::new(
        storage,
        state.clone(),
        alice_secret,
        alice_viewing,
    );

    // alice creates a proposal
    let mut rng = rand::thread_rng();
    let builder = ProposalBuilder::signaling("test proposal")
        .with_description("testing the full flow")
        .with_threshold(67);

    let (envelope, proposal_id) = client.create_proposal(builder, &mut rng);

    assert_eq!(proposal_id, 1);
    assert!(matches!(
        envelope.payload,
        narsil::MessagePayload::Proposal(_)
    ));

    // alice votes yes with her 40 shares
    let vote_envelope = client.create_vote(
        proposal_id,
        VoteType::Yes,
        (1..=40).collect(),
        &mut rng,
    ).unwrap();

    assert!(matches!(
        vote_envelope.payload,
        narsil::MessagePayload::Vote(_)
    ));
}

#[test]
fn test_contribution_aggregation() {
    let proposal_id = 1;
    let threshold = 67;

    let mut collector = ContributionCollector::new(proposal_id, threshold);

    // alice contributes 40 shares
    let alice_contrib = SignedContribution {
        contribution: WireContribution {
            proposal_id,
            share_ids: (1..=40).collect(),
            osst_data: vec![1, 2, 3, 4],
        },
        contributor_pubkey: [1u8; 32],
        signature: [0u8; 64],
    };

    collector.add_contribution(&alice_contrib).unwrap();
    assert_eq!(collector.collected_count(), 40);
    assert!(!collector.is_ready());

    // bob contributes 30 shares
    let bob_contrib = SignedContribution {
        contribution: WireContribution {
            proposal_id,
            share_ids: (41..=70).collect(),
            osst_data: vec![5, 6, 7, 8],
        },
        contributor_pubkey: [2u8; 32],
        signature: [0u8; 64],
    };

    collector.add_contribution(&bob_contrib).unwrap();
    assert_eq!(collector.collected_count(), 70);
    assert!(collector.is_ready()); // 70 >= 67

    // aggregate
    let signature = collector.aggregate().unwrap();
    assert!(!signature.is_empty());
}

#[test]
fn test_batched_aggregator() {
    let proposal_id = 1;
    let threshold = 51;

    let mut agg = BatchedAggregator::new(proposal_id, threshold);

    // member with 30 shares
    let contrib1 = SignedContribution {
        contribution: WireContribution {
            proposal_id,
            share_ids: (1..=30).collect(),
            osst_data: vec![1, 2, 3],
        },
        contributor_pubkey: [1u8; 32],
        signature: [0u8; 64],
    };

    agg.add(&contrib1).unwrap();
    assert!(!agg.is_ready());

    // member with 25 shares (total now 55 >= 51)
    let contrib2 = SignedContribution {
        contribution: WireContribution {
            proposal_id,
            share_ids: (31..=55).collect(),
            osst_data: vec![4, 5, 6],
        },
        contributor_pubkey: [2u8; 32],
        signature: [0u8; 64],
    };

    agg.add(&contrib2).unwrap();
    assert!(agg.is_ready());

    let sig = agg.finalize().unwrap();
    assert!(!sig.is_empty());
}

#[test]
fn test_encryption_between_members() {
    let syndicate_id = [1u8; 32];

    let alice = MemberCrypto::new([10u8; 32], syndicate_id);
    let bob = MemberCrypto::new([20u8; 32], syndicate_id);

    // alice encrypts for bob
    let plaintext = b"secret syndicate proposal data";
    let nonce = [42u8; 12];

    let ciphertext = alice.encrypt_for(bob.pubkey(), plaintext, &nonce);

    // bob decrypts
    let decrypted = bob.decrypt_from(alice.pubkey(), &ciphertext, &nonce).unwrap();

    assert_eq!(decrypted.as_slice(), plaintext);
}

#[test]
fn test_state_hash_replay_protection() {
    use narsil::{ReplayValidator, ReplayCheck};

    let state_hash = [1u8; 32];
    let mut validator = ReplayValidator::new(state_hash);

    let alice = [10u8; 32];
    validator.add_sender(alice);

    // create mock envelope
    let envelope = narsil::Envelope {
        version: 1,
        syndicate_id: [0u8; 32],
        state_hash,
        sequence: 1,
        payload: narsil::MessagePayload::SyncRequest(narsil::SyncRequest {
            current_state_hash: [0u8; 32],
            current_sequence: 0,
        }),
        signature: [0u8; 64],
    };

    // first message valid
    assert_eq!(validator.validate(&envelope, &alice), ReplayCheck::Valid);
    validator.record(&alice, 1);

    // replay fails
    assert!(matches!(
        validator.validate(&envelope, &alice),
        ReplayCheck::DuplicateSequence { .. }
    ));

    // different state hash fails
    let old_envelope = narsil::Envelope {
        state_hash: [99u8; 32], // wrong state
        ..envelope.clone()
    };
    assert!(matches!(
        validator.validate(&old_envelope, &alice),
        ReplayCheck::StaleState { .. }
    ));
}

#[test]
fn test_share_registry_operations() {
    use narsil::PubkeyShareRegistry;

    let alice = [1u8; 32];
    let bob = [2u8; 32];

    // create registry with custom allocation
    let registry = PubkeyShareRegistry::with_allocation(&[
        (alice, 60),
        (bob, 40),
    ]).unwrap();

    assert!(registry.is_fully_allocated());
    assert_eq!(registry.share_count(&alice), 60);
    assert_eq!(registry.share_count(&bob), 40);

    // check ownership
    assert_eq!(registry.owner(1), Some(&alice));
    assert_eq!(registry.owner(60), Some(&alice));
    assert_eq!(registry.owner(61), Some(&bob));
    assert_eq!(registry.owner(100), Some(&bob));

    // threshold check
    let alice_shares: Vec<u8> = (1..=60).collect();
    assert!(registry.meets_threshold(&alice_shares, 51));
    assert!(registry.meets_threshold(&alice_shares, 60));
    assert!(!registry.meets_threshold(&alice_shares, 61));
}

#[test]
fn test_vss_backup_and_recovery() {
    use narsil::{ShareDistributor, VssError};

    let distributor = ShareDistributor::new(2); // 2-of-3 threshold

    let owner = [1u8; 32];
    let recipients = [[2u8; 32], [3u8; 32], [4u8; 32]];
    let secret_data = b"encrypted osst share";

    let mut rng = rand::thread_rng();
    let package = distributor.create_package(owner, secret_data, &recipients, &mut rng);

    assert_eq!(package.header.threshold, 2);
    assert_eq!(package.header.total, 3);
    assert_eq!(package.backup_shares.len(), 3);

    // reconstruct from any 2 shares
    let reconstructed = ShareDistributor::reconstruct(
        &package.header,
        &package.backup_shares[0..2],
    ).unwrap();

    assert_eq!(reconstructed.as_slice(), secret_data);

    // 1 share not enough
    let result = ShareDistributor::reconstruct(
        &package.header,
        &package.backup_shares[0..1],
    );
    assert_eq!(result, Err(VssError::InsufficientShares));
}

#[test]
fn test_reshare_session() {
    use narsil::{ReshareSession, ReshareProposal, ResharePhase, OldMember, ReshareCommitment};

    let syndicate_id = [1u8; 32];
    let proposal = ReshareProposal::key_rotation(9999999999);

    let old_members = vec![
        OldMember { pubkey: [1u8; 32], shares: (1..=50).collect() },
        OldMember { pubkey: [2u8; 32], shares: (51..=100).collect() },
    ];

    let mut session = ReshareSession::new(
        syndicate_id,
        0,
        proposal,
        old_members,
        51, // need 51 shares to approve
    );

    // add approvals
    for share_id in 1..=51 {
        session.add_approval(share_id).unwrap();
    }

    assert_eq!(session.phase, ResharePhase::Committing);

    // add commitments for all shares
    for share_id in 1..=50 {
        session.add_commitment(ReshareCommitment {
            from: [1u8; 32],
            share_id,
            commitment: [10u8; 32],
            verification_points: vec![],
        }).unwrap();
    }

    for share_id in 51..=100 {
        session.add_commitment(ReshareCommitment {
            from: [2u8; 32],
            share_id,
            commitment: [20u8; 32],
            verification_points: vec![],
        }).unwrap();
    }

    assert_eq!(session.phase, ResharePhase::Distributing);
}

#[test]
fn test_full_syndicate_lifecycle() {
    // this test simulates the full lifecycle:
    // 1. formation
    // 2. normal operation (proposal + vote)
    // 3. reshare (add member)

    // --- FORMATION ---
    let rules = WireGovernanceRules::default();
    let mut ceremony = FormationCeremony::new(rules.clone(), 51);

    ceremony.add_member(JoiningMember {
        pubkey: [1u8; 32],
        viewing_key: [11u8; 32],
        name: "alice".into(),
        shares: (1..=50).collect(),
    }).unwrap();

    ceremony.add_member(JoiningMember {
        pubkey: [2u8; 32],
        viewing_key: [22u8; 32],
        name: "bob".into(),
        shares: (51..=100).collect(),
    }).unwrap();

    ceremony.start_committing().unwrap();

    // fast-forward through ceremony
    ceremony.add_commitment(DkgCommitment {
        pubkey: [1u8; 32],
        commitment: [10u8; 32],
        verification_points: vec![],
    }).unwrap();
    ceremony.add_commitment(DkgCommitment {
        pubkey: [2u8; 32],
        commitment: [20u8; 32],
        verification_points: vec![],
    }).unwrap();

    ceremony.add_share(DkgShare {
        from: [1u8; 32], to: [2u8; 32],
        share_indices: vec![1], encrypted_data: vec![1],
    }).unwrap();
    ceremony.add_share(DkgShare {
        from: [2u8; 32], to: [1u8; 32],
        share_indices: vec![51], encrypted_data: vec![2],
    }).unwrap();

    let formation = ceremony.finalize().unwrap();
    assert!(ceremony.is_complete());

    // --- OPERATION ---
    let mut state = SyndicateStateManager::new(formation.syndicate_id, rules);
    state.add_member([1u8; 32], "alice".into(), (1..=50).collect()).unwrap();
    state.add_member([2u8; 32], "bob".into(), (51..=100).collect()).unwrap();

    // create proposal
    let proposal_id = state.submit_proposal(
        WireProposalKind::Signaling,
        "add carol".into(),
        "proposal to add carol as new member".into(),
        67, // supermajority
        9999999999,
        vec![],
    );

    // alice votes yes with 50 shares
    for share_id in 1..=50 {
        state.record_vote(proposal_id, share_id, VoteType::Yes, &[1u8; 32]).unwrap();
    }

    // bob votes yes with 17 shares (total 67)
    for share_id in 51..=67 {
        state.record_vote(proposal_id, share_id, VoteType::Yes, &[2u8; 32]).unwrap();
    }

    assert!(state.proposal_passed(proposal_id));

    // --- RESHARE (simulated) ---
    // in real impl, after proposal passes:
    // 1. initiate reshare session
    // 2. reallocate shares to include carol
    // 3. run DKG among all 3 members
    // 4. complete reshare, increment epoch
}
