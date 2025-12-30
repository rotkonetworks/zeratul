//! tests for osst-threshold pallet

use crate::{self as pallet_osst_threshold, *};
use frame_support::{
    assert_noop, assert_ok,
    derive_impl,
    parameter_types,
};
use sp_runtime::{
    traits::IdentityLookup,
    BuildStorage,
};

// pallas curve for test key generation
use pasta_curves::{
    group::{ff::Field, Group, GroupEncoding},
    pallas::{Point as PallasPoint, Scalar as PallasScalar},
};
use osst::{SecretShare, Contribution as OsstContrib, OsstPoint};

type Block = frame_system::mocking::MockBlock<Test>;

frame_support::construct_runtime!(
    pub enum Test {
        System: frame_system,
        OsstThreshold: pallet_osst_threshold,
    }
);

parameter_types! {
    pub const BlockHashCount: u64 = 250;
}

#[derive_impl(frame_system::config_preludes::TestDefaultConfig)]
impl frame_system::Config for Test {
    type BaseCallFilter = frame_support::traits::Everything;
    type Block = Block;
    type BlockHashCount = BlockHashCount;
    type AccountId = u64;
    type Lookup = IdentityLookup<Self::AccountId>;
    type AccountData = ();
}

parameter_types! {
    pub const MinCustodians: u32 = 4;  // strict BFT minimum
    pub const MaxCustodians: u32 = 100;
    pub const ThresholdNumerator: u32 = 2;
    pub const ThresholdDenominator: u32 = 3;
    pub const ReshareTimeout: u32 = 100;
    pub const LivenessValidity: u32 = 1000;
    pub const EpochDuration: u32 = 10000;
}

impl pallet_osst_threshold::Config for Test {
    type MinCustodians = MinCustodians;
    type MaxCustodians = MaxCustodians;
    type ThresholdNumerator = ThresholdNumerator;
    type ThresholdDenominator = ThresholdDenominator;
    type ReshareTimeout = ReshareTimeout;
    type LivenessValidity = LivenessValidity;
    type EpochDuration = EpochDuration;
}

pub fn new_test_ext() -> sp_io::TestExternalities {
    let t = frame_system::GenesisConfig::<Test>::default()
        .build_storage()
        .unwrap();
    let mut ext = sp_io::TestExternalities::new(t);
    ext.execute_with(|| System::set_block_number(1));
    ext
}

fn run_to_block(n: u64) {
    while System::block_number() < n {
        System::set_block_number(System::block_number() + 1);
        System::on_initialize(System::block_number());
        OsstThreshold::on_finalize(System::block_number());
    }
}

/// helper to complete a full reshare with 4 custodians (strict BFT)
fn complete_reshare() {
    // register 4 custodians (minimum for strict BFT)
    for i in 1..=4 {
        assert_ok!(OsstThreshold::register_custodian(
            RuntimeOrigin::signed(i),
            [i as u8; 32],
        ));
    }

    // confirm we're in commitments phase
    assert!(matches!(
        pallet::CurrentResharePhase::<Test>::get(),
        ResharePhase::Commitments { .. }
    ));

    // threshold for n=4: floor(4*2/3) + 1 = 2 + 1 = 3
    let coefficients = vec![[1u8; 32], [2u8; 32], [3u8; 32]];

    // submit 3 commitments (threshold)
    assert_ok!(OsstThreshold::submit_dealer_commitment(
        RuntimeOrigin::signed(1),
        coefficients.clone(),
        vec![],
        [0u8; 64],
    ));
    assert_ok!(OsstThreshold::submit_dealer_commitment(
        RuntimeOrigin::signed(2),
        coefficients.clone(),
        vec![],
        [0u8; 64],
    ));
    assert_ok!(OsstThreshold::submit_dealer_commitment(
        RuntimeOrigin::signed(3),
        coefficients.clone(),
        vec![],
        [0u8; 64],
    ));

    // confirm we're in subshares phase
    assert!(matches!(
        pallet::CurrentResharePhase::<Test>::get(),
        ResharePhase::Subshares { .. }
    ));

    // 3 dealers send to all 4 recipients
    for dealer in 1..=3u64 {
        for recipient in 1..=4u32 {
            assert_ok!(OsstThreshold::submit_subshare(
                RuntimeOrigin::signed(dealer),
                recipient,
                vec![0u8; 48],
                [0u8; 32],
            ));
        }
    }

    // confirm we're in verification phase
    assert!(matches!(
        pallet::CurrentResharePhase::<Test>::get(),
        ResharePhase::Verification { .. }
    ));

    // all 4 finalize
    for i in 1..=4 {
        assert_ok!(OsstThreshold::finalize_reshare(
            RuntimeOrigin::signed(i),
            [i as u8; 32],
        ));
    }

    // confirm reshare complete
    assert!(matches!(
        pallet::CurrentResharePhase::<Test>::get(),
        ResharePhase::Idle
    ));
}

// ============ registration tests ============

#[test]
fn test_register_custodian() {
    new_test_ext().execute_with(|| {
        let encryption_key = [1u8; 32];

        assert_ok!(OsstThreshold::register_custodian(
            RuntimeOrigin::signed(1),
            encryption_key,
        ));

        let info = OsstThreshold::get_custodian(&1).unwrap();
        assert_eq!(info.index, 1);
        assert_eq!(info.encryption_key, encryption_key);
        assert!(!info.active);

        // check storage
        assert!(pallet::Custodians::<Test>::contains_key(1));
        assert!(pallet::CustodianByIndex::<Test>::contains_key(1));
        assert_eq!(pallet::ActiveCustodians::<Test>::get().len(), 1);
    });
}

#[test]
fn test_register_multiple_custodians() {
    new_test_ext().execute_with(|| {
        for i in 1..=5 {
            assert_ok!(OsstThreshold::register_custodian(
                RuntimeOrigin::signed(i),
                [i as u8; 32],
            ));
        }

        assert_eq!(pallet::ActiveCustodians::<Test>::get().len(), 5);
        assert_eq!(pallet::NextCustodianIndex::<Test>::get(), 5);

        // each should have correct index
        for i in 1..=5 {
            let info = OsstThreshold::get_custodian(&i).unwrap();
            assert_eq!(info.index, i as u32);
        }
    });
}

#[test]
fn test_register_already_registered() {
    new_test_ext().execute_with(|| {
        assert_ok!(OsstThreshold::register_custodian(
            RuntimeOrigin::signed(1),
            [1u8; 32],
        ));

        assert_noop!(
            OsstThreshold::register_custodian(RuntimeOrigin::signed(1), [2u8; 32]),
            Error::<Test>::AlreadyRegistered
        );
    });
}

#[test]
fn test_register_too_many_custodians() {
    new_test_ext().execute_with(|| {
        // register up to max
        for i in 1..=100 {
            assert_ok!(OsstThreshold::register_custodian(
                RuntimeOrigin::signed(i),
                [i as u8; 32],
            ));
        }

        // 101st should fail
        assert_noop!(
            OsstThreshold::register_custodian(RuntimeOrigin::signed(101), [101u8; 32]),
            Error::<Test>::TooManyCustodians
        );
    });
}

// ============ reshare tests ============

#[test]
fn test_reshare_starts_with_min_custodians() {
    new_test_ext().execute_with(|| {
        // register min custodians (4 for strict BFT)
        for i in 1..=4 {
            assert_ok!(OsstThreshold::register_custodian(
                RuntimeOrigin::signed(i),
                [i as u8; 32],
            ));
        }

        // reshare should have started
        assert!(matches!(
            pallet::CurrentResharePhase::<Test>::get(),
            ResharePhase::Commitments { .. }
        ));
    });
}

#[test]
fn test_reshare_not_started_below_min() {
    new_test_ext().execute_with(|| {
        // register only 3 (below min of 4)
        for i in 1..=3 {
            assert_ok!(OsstThreshold::register_custodian(
                RuntimeOrigin::signed(i),
                [i as u8; 32],
            ));
        }

        // reshare should not have started
        assert!(matches!(
            pallet::CurrentResharePhase::<Test>::get(),
            ResharePhase::Idle
        ));
    });
}

#[test]
fn test_submit_dealer_commitment() {
    new_test_ext().execute_with(|| {
        // register 4 custodians
        for i in 1..=4 {
            assert_ok!(OsstThreshold::register_custodian(
                RuntimeOrigin::signed(i),
                [i as u8; 32],
            ));
        }

        // threshold for 4 custodians with strict BFT = 3
        let coefficients = vec![[1u8; 32], [2u8; 32], [3u8; 32]];

        assert_ok!(OsstThreshold::submit_dealer_commitment(
            RuntimeOrigin::signed(1),
            coefficients.clone(),
            vec![], // empty liveness proof
            [0u8; 64], // dummy signature
        ));

        assert!(pallet::DealerCommitments::<Test>::contains_key(1));
    });
}

#[test]
fn test_submit_dealer_commitment_wrong_phase() {
    new_test_ext().execute_with(|| {
        // register only 2 (reshare not started)
        for i in 1..=2 {
            assert_ok!(OsstThreshold::register_custodian(
                RuntimeOrigin::signed(i),
                [i as u8; 32],
            ));
        }

        let coefficients = vec![[1u8; 32], [2u8; 32]];

        assert_noop!(
            OsstThreshold::submit_dealer_commitment(
                RuntimeOrigin::signed(1),
                coefficients,
                vec![],
                [0u8; 64],
            ),
            Error::<Test>::WrongResharePhase
        );
    });
}

#[test]
fn test_submit_dealer_commitment_not_registered() {
    new_test_ext().execute_with(|| {
        // register 4 custodians
        for i in 1..=4 {
            assert_ok!(OsstThreshold::register_custodian(
                RuntimeOrigin::signed(i),
                [i as u8; 32],
            ));
        }

        let coefficients = vec![[1u8; 32], [2u8; 32], [3u8; 32]];

        // account 99 not registered
        assert_noop!(
            OsstThreshold::submit_dealer_commitment(
                RuntimeOrigin::signed(99),
                coefficients,
                vec![],
                [0u8; 64],
            ),
            Error::<Test>::NotRegistered
        );
    });
}

#[test]
fn test_reshare_phase_advances_to_subshares() {
    new_test_ext().execute_with(|| {
        // register 4 custodians
        for i in 1..=4 {
            assert_ok!(OsstThreshold::register_custodian(
                RuntimeOrigin::signed(i),
                [i as u8; 32],
            ));
        }

        // threshold = 3 for strict BFT, so 3 dealers needed
        let coefficients = vec![[1u8; 32], [2u8; 32], [3u8; 32]];

        // first dealer
        assert_ok!(OsstThreshold::submit_dealer_commitment(
            RuntimeOrigin::signed(1),
            coefficients.clone(),
            vec![],
            [0u8; 64],
        ));

        // still in commitments phase
        assert!(matches!(
            pallet::CurrentResharePhase::<Test>::get(),
            ResharePhase::Commitments { .. }
        ));

        // second dealer
        assert_ok!(OsstThreshold::submit_dealer_commitment(
            RuntimeOrigin::signed(2),
            coefficients.clone(),
            vec![],
            [0u8; 64],
        ));

        // still in commitments phase (need 3)
        assert!(matches!(
            pallet::CurrentResharePhase::<Test>::get(),
            ResharePhase::Commitments { .. }
        ));

        // third dealer
        assert_ok!(OsstThreshold::submit_dealer_commitment(
            RuntimeOrigin::signed(3),
            coefficients.clone(),
            vec![],
            [0u8; 64],
        ));

        // should advance to subshares phase
        assert!(matches!(
            pallet::CurrentResharePhase::<Test>::get(),
            ResharePhase::Subshares { .. }
        ));
    });
}

#[test]
fn test_submit_subshare() {
    new_test_ext().execute_with(|| {
        // register 4 custodians
        for i in 1..=4 {
            assert_ok!(OsstThreshold::register_custodian(
                RuntimeOrigin::signed(i),
                [i as u8; 32],
            ));
        }

        let coefficients = vec![[1u8; 32], [2u8; 32], [3u8; 32]];

        // submit 3 commitments to advance to subshares
        assert_ok!(OsstThreshold::submit_dealer_commitment(
            RuntimeOrigin::signed(1),
            coefficients.clone(),
            vec![],
            [0u8; 64],
        ));
        assert_ok!(OsstThreshold::submit_dealer_commitment(
            RuntimeOrigin::signed(2),
            coefficients.clone(),
            vec![],
            [0u8; 64],
        ));
        assert_ok!(OsstThreshold::submit_dealer_commitment(
            RuntimeOrigin::signed(3),
            coefficients.clone(),
            vec![],
            [0u8; 64],
        ));

        // now in subshares phase
        assert!(matches!(
            pallet::CurrentResharePhase::<Test>::get(),
            ResharePhase::Subshares { .. }
        ));

        // dealer 1 submits subshare to recipient 2
        assert_ok!(OsstThreshold::submit_subshare(
            RuntimeOrigin::signed(1),
            2, // to recipient
            vec![0u8; 48], // encrypted share
            [0u8; 32], // ephemeral pk
        ));

        assert!(pallet::Subshares::<Test>::contains_key(1, 2));
        assert_eq!(pallet::ReceivedSubshareCount::<Test>::get(2), 1);
    });
}

#[test]
fn test_full_reshare_flow() {
    new_test_ext().execute_with(|| {
        // register 4 custodians
        for i in 1..=4 {
            assert_ok!(OsstThreshold::register_custodian(
                RuntimeOrigin::signed(i),
                [i as u8; 32],
            ));
        }

        let coefficients = vec![[1u8; 32], [2u8; 32], [3u8; 32]];

        // submit 3 commitments (threshold) - phase advances after 3rd
        assert_ok!(OsstThreshold::submit_dealer_commitment(
            RuntimeOrigin::signed(1),
            coefficients.clone(),
            vec![],
            [0u8; 64],
        ));
        assert_ok!(OsstThreshold::submit_dealer_commitment(
            RuntimeOrigin::signed(2),
            coefficients.clone(),
            vec![],
            [0u8; 64],
        ));
        assert_ok!(OsstThreshold::submit_dealer_commitment(
            RuntimeOrigin::signed(3),
            coefficients.clone(),
            vec![],
            [0u8; 64],
        ));

        // should be in subshares phase after threshold reached
        assert!(matches!(
            pallet::CurrentResharePhase::<Test>::get(),
            ResharePhase::Subshares { .. }
        ));

        // 3 dealers send to all 4 recipients
        for dealer in 1..=3u64 {
            for recipient in 1..=4u32 {
                assert_ok!(OsstThreshold::submit_subshare(
                    RuntimeOrigin::signed(dealer),
                    recipient,
                    vec![0u8; 48],
                    [0u8; 32],
                ));
            }
        }

        // should advance to verification phase
        assert!(matches!(
            pallet::CurrentResharePhase::<Test>::get(),
            ResharePhase::Verification { .. }
        ));

        // all 4 finalize
        for i in 1..=4 {
            assert_ok!(OsstThreshold::finalize_reshare(
                RuntimeOrigin::signed(i),
                [i as u8; 32], // public share
            ));
        }

        // should be back to idle with epoch finalized
        assert!(matches!(
            pallet::CurrentResharePhase::<Test>::get(),
            ResharePhase::Idle
        ));

        let epoch = pallet::CurrentEpoch::<Test>::get();
        assert_eq!(epoch.epoch, 0); // first epoch
        assert_eq!(epoch.threshold, 3); // strict BFT for n=4
        assert_eq!(epoch.custodian_count, 4);
    });
}

// ============ timeout tests ============

#[test]
fn test_reshare_timeout() {
    new_test_ext().execute_with(|| {
        // register 4 custodians
        for i in 1..=4 {
            assert_ok!(OsstThreshold::register_custodian(
                RuntimeOrigin::signed(i),
                [i as u8; 32],
            ));
        }

        // get deadline
        let deadline = match pallet::CurrentResharePhase::<Test>::get() {
            ResharePhase::Commitments { deadline } => deadline,
            _ => panic!("expected commitments phase"),
        };

        // run past deadline
        run_to_block((deadline + 1) as u64);

        // should be failed
        assert!(matches!(
            pallet::CurrentResharePhase::<Test>::get(),
            ResharePhase::Failed { reason: ReshareFailureReason::Timeout }
        ));
    });
}

// ============ osst verification tests ============

#[test]
fn test_verify_osst_no_custody() {
    new_test_ext().execute_with(|| {
        let contributions = vec![
            OsstContribution {
                index: 1,
                public_share: [1u8; 32],
                response: [2u8; 32],
            },
        ];

        assert_noop!(
            OsstThreshold::verify_osst(
                RuntimeOrigin::signed(1),
                contributions,
                b"payload".to_vec(),
            ),
            Error::<Test>::NoCustody
        );
    });
}

#[test]
fn test_verify_osst_with_custody() {
    new_test_ext().execute_with(|| {
        use rand::rngs::OsRng;

        // generate a real secret and shares using shamir
        let mut rng = OsRng;
        let secret = <PallasScalar as Field>::random(&mut rng);
        let group_pubkey: PallasPoint = <PallasPoint as Group>::generator() * secret;

        // shamir split: f(x) = secret + a1*x  for threshold 2
        let a1 = <PallasScalar as Field>::random(&mut rng);
        let share1 = secret + a1 * PallasScalar::from(1u64);
        let share2 = secret + a1 * PallasScalar::from(2u64);

        let ss1 = SecretShare::<PallasScalar>::new(1, share1);
        let ss2 = SecretShare::<PallasScalar>::new(2, share2);

        let payload = b"test payload";

        // generate OSST contributions
        let contrib1: OsstContrib<PallasPoint> = ss1.contribute(&mut rng, payload);
        let contrib2: OsstContrib<PallasPoint> = ss2.contribute(&mut rng, payload);

        // set up the epoch with the correct group key
        let epoch = EpochInfo {
            epoch: 1,
            group_key: group_pubkey.to_bytes(),
            threshold: 2,
            custodian_count: 3,
            started_at: 1,
        };
        pallet::CurrentEpoch::<Test>::put(epoch);

        // convert to pallet contribution type
        let contributions = vec![
            OsstContribution {
                index: contrib1.index,
                public_share: contrib1.commitment.compress(),
                response: contrib1.response.to_bytes(),
            },
            OsstContribution {
                index: contrib2.index,
                public_share: contrib2.commitment.compress(),
                response: contrib2.response.to_bytes(),
            },
        ];

        // should succeed
        assert_ok!(OsstThreshold::verify_osst(
            RuntimeOrigin::signed(1),
            contributions,
            payload.to_vec(),
        ));
    });
}

#[test]
fn test_verify_osst_wrong_payload() {
    new_test_ext().execute_with(|| {
        use rand::rngs::OsRng;

        // generate real secret and shares
        let mut rng = OsRng;
        let secret = <PallasScalar as Field>::random(&mut rng);
        let group_pubkey: PallasPoint = <PallasPoint as Group>::generator() * secret;

        // shamir split
        let a1 = <PallasScalar as Field>::random(&mut rng);
        let share1 = secret + a1 * PallasScalar::from(1u64);
        let share2 = secret + a1 * PallasScalar::from(2u64);

        let ss1 = SecretShare::<PallasScalar>::new(1, share1);
        let ss2 = SecretShare::<PallasScalar>::new(2, share2);

        let correct_payload = b"correct payload";
        let wrong_payload = b"wrong payload";

        // generate contributions with correct payload
        let contrib1: OsstContrib<PallasPoint> = ss1.contribute(&mut rng, correct_payload);
        let contrib2: OsstContrib<PallasPoint> = ss2.contribute(&mut rng, correct_payload);

        // set up epoch
        let epoch = EpochInfo {
            epoch: 1,
            group_key: group_pubkey.to_bytes(),
            threshold: 2,
            custodian_count: 3,
            started_at: 1,
        };
        pallet::CurrentEpoch::<Test>::put(epoch);

        let contributions = vec![
            OsstContribution {
                index: contrib1.index,
                public_share: contrib1.commitment.compress(),
                response: contrib1.response.to_bytes(),
            },
            OsstContribution {
                index: contrib2.index,
                public_share: contrib2.commitment.compress(),
                response: contrib2.response.to_bytes(),
            },
        ];

        // verify with wrong payload - should fail
        assert_noop!(
            OsstThreshold::verify_osst(
                RuntimeOrigin::signed(1),
                contributions,
                wrong_payload.to_vec(),
            ),
            Error::<Test>::VerificationFailed
        );
    });
}

#[test]
fn test_verify_osst_duplicate_indices() {
    new_test_ext().execute_with(|| {
        // set up epoch with valid group key (just generator point)
        let group_key = <PallasPoint as Group>::generator().to_bytes();
        let epoch = EpochInfo {
            epoch: 1,
            group_key,
            threshold: 2,
            custodian_count: 3,
            started_at: 1,
        };
        pallet::CurrentEpoch::<Test>::put(epoch);

        // duplicate indices - should fail
        let contributions = vec![
            OsstContribution {
                index: 1,
                public_share: <PallasPoint as Group>::generator().to_bytes(),
                response: [1u8; 32],
            },
            OsstContribution {
                index: 1, // duplicate!
                public_share: <PallasPoint as Group>::generator().to_bytes(),
                response: [2u8; 32],
            },
        ];

        assert_noop!(
            OsstThreshold::verify_osst(
                RuntimeOrigin::signed(1),
                contributions,
                b"payload".to_vec(),
            ),
            Error::<Test>::VerificationFailed
        );
    });
}

// ============ liveness anchor tests ============

#[test]
fn test_set_liveness_anchor() {
    new_test_ext().execute_with(|| {
        assert_ok!(OsstThreshold::set_liveness_anchor(
            RuntimeOrigin::root(),
            1000,
            [42u8; 32],
        ));

        let anchor = pallet::LivenessAnchor::<Test>::get();
        assert_eq!(anchor.height, 1000);
        assert_eq!(anchor.block_hash, [42u8; 32]);
    });
}

#[test]
fn test_set_liveness_anchor_not_root() {
    new_test_ext().execute_with(|| {
        assert_noop!(
            OsstThreshold::set_liveness_anchor(
                RuntimeOrigin::signed(1),
                1000,
                [42u8; 32],
            ),
            sp_runtime::DispatchError::BadOrigin
        );
    });
}

// ============ force reshare tests ============

#[test]
fn test_force_reshare() {
    new_test_ext().execute_with(|| {
        complete_reshare();

        // now idle
        assert!(matches!(
            pallet::CurrentResharePhase::<Test>::get(),
            ResharePhase::Idle
        ));

        // force new reshare
        assert_ok!(OsstThreshold::force_reshare(RuntimeOrigin::root()));

        // should be in commitments phase
        assert!(matches!(
            pallet::CurrentResharePhase::<Test>::get(),
            ResharePhase::Commitments { .. }
        ));
    });
}

#[test]
fn test_force_reshare_not_root() {
    new_test_ext().execute_with(|| {
        assert_noop!(
            OsstThreshold::force_reshare(RuntimeOrigin::signed(1)),
            sp_runtime::DispatchError::BadOrigin
        );
    });
}

// ============ helper function tests ============

#[test]
fn test_compute_threshold() {
    new_test_ext().execute_with(|| {
        // strict BFT: t = floor(2n/3) + 1
        // 4 custodians -> floor(8/3) + 1 = 2 + 1 = 3
        // 7 custodians -> floor(14/3) + 1 = 4 + 1 = 5
        // 10 custodians -> floor(20/3) + 1 = 6 + 1 = 7

        for i in 1..=4 {
            assert_ok!(OsstThreshold::register_custodian(
                RuntimeOrigin::signed(i),
                [i as u8; 32],
            ));
        }

        // can't directly test compute_threshold, but we can check via coefficient count
        // threshold for 4 with strict BFT = 3

        // try with wrong number of coefficients
        let wrong_coefficients = vec![[1u8; 32], [2u8; 32]]; // only 2, needs 3

        assert_noop!(
            OsstThreshold::submit_dealer_commitment(
                RuntimeOrigin::signed(1),
                wrong_coefficients,
                vec![],
                [0u8; 64],
            ),
            Error::<Test>::InvalidCommitment
        );

        // correct number
        let correct_coefficients = vec![[1u8; 32], [2u8; 32], [3u8; 32]]; // 3 coefficients

        assert_ok!(OsstThreshold::submit_dealer_commitment(
            RuntimeOrigin::signed(1),
            correct_coefficients,
            vec![],
            [0u8; 64],
        ));
    });
}

#[test]
fn test_custody_state() {
    new_test_ext().execute_with(|| {
        // no custody initially
        assert!(OsstThreshold::get_custody_state().is_none());

        // complete reshare
        complete_reshare();

        // now have custody
        let state = OsstThreshold::get_custody_state();
        assert!(state.is_some());

        let (epoch, _group_key, threshold) = state.unwrap();
        assert_eq!(epoch, 0);
        assert_eq!(threshold, 3); // strict BFT for n=4
    });
}
