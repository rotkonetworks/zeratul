//! tests for frost-bridge pallet

use crate::{self as pallet_frost_bridge, *};
use frame_support::{
    assert_noop, assert_ok, derive_impl,
    parameter_types,
    traits::Hooks,
};
use sp_runtime::BuildStorage;

type Block = frame_system::mocking::MockBlock<Test>;

frame_support::construct_runtime!(
    pub enum Test {
        System: frame_system,
        FrostBridge: pallet_frost_bridge,
    }
);

#[derive_impl(frame_system::config_preludes::TestDefaultConfig)]
impl frame_system::Config for Test {
    type Block = Block;
}

parameter_types! {
    pub const MinSigners: u16 = 3;
    pub const MaxSigners: u16 = 10;
    pub const Threshold: u16 = 2;
    pub const DkgTimeout: u32 = 100;
    pub const SigningTimeout: u32 = 50;
    pub const RotationPeriod: u32 = 1000;
    pub const HeartbeatInterval: u32 = 10;
    pub const OfflineThreshold: u32 = 30;
    pub const SlashingGracePeriod: u32 = 5;
    pub const MinParticipationRate: u8 = 80;
    pub const CircuitBreakerThreshold: u32 = 3;
}

impl pallet_frost_bridge::Config for Test {
    type RuntimeEvent = RuntimeEvent;
    type MinSigners = MinSigners;
    type MaxSigners = MaxSigners;
    type Threshold = Threshold;
    type DkgTimeout = DkgTimeout;
    type SigningTimeout = SigningTimeout;
    type RotationPeriod = RotationPeriod;
    type HeartbeatInterval = HeartbeatInterval;
    type OfflineThreshold = OfflineThreshold;
    type SlashingGracePeriod = SlashingGracePeriod;
    type MinParticipationRate = MinParticipationRate;
    type CircuitBreakerThreshold = CircuitBreakerThreshold;
}

// helper to create test encryption key
fn test_encryption_key(seed: u8) -> [u8; 32] {
    [seed; 32]
}

// helper to create test commitment
fn test_commitment(seed: u8) -> [u8; 32] {
    [seed; 32]
}

// helper to create test public share
fn test_public_share(seed: u8) -> [u8; 32] {
    [seed; 32]
}

// helper to create test proof
fn test_proof(seed: u8) -> [u8; 64] {
    [seed; 64]
}

// helper to create test signature
fn test_signature(seed: u8) -> FrostSignature {
    FrostSignature {
        r: [seed; 32],
        s: [seed.wrapping_add(1); 32],
    }
}

// build test externalities
pub fn new_test_ext() -> sp_io::TestExternalities {
    let t = frame_system::GenesisConfig::<Test>::default()
        .build_storage()
        .unwrap();

    let mut ext = sp_io::TestExternalities::new(t);
    ext.execute_with(|| {
        System::set_block_number(1);
    });
    ext
}

// ============ registration tests ============

#[test]
fn register_signer_works() {
    new_test_ext().execute_with(|| {
        let encryption_key = test_encryption_key(1);

        assert_ok!(FrostBridge::register_signer(
            RuntimeOrigin::signed(1),
            encryption_key,
        ));

        // check signer was added
        assert!(pallet_frost_bridge::Signers::<Test>::contains_key(1));

        let signer = pallet_frost_bridge::Signers::<Test>::get(1).unwrap();
        assert_eq!(signer.index, 1);
        assert_eq!(signer.encryption_key, encryption_key);
        assert!(matches!(signer.status, SignerStatus::Active));

        // check active signer list
        let active = pallet_frost_bridge::ActiveSignerList::<Test>::get();
        assert_eq!(active.len(), 1);
        assert_eq!(active[0], 1);
    });
}

#[test]
fn register_signer_fails_if_already_registered() {
    new_test_ext().execute_with(|| {
        let encryption_key = test_encryption_key(1);

        assert_ok!(FrostBridge::register_signer(
            RuntimeOrigin::signed(1),
            encryption_key,
        ));

        assert_noop!(
            FrostBridge::register_signer(
                RuntimeOrigin::signed(1),
                encryption_key,
            ),
            Error::<Test>::AlreadyRegistered
        );
    });
}

#[test]
fn register_signer_fails_if_too_many() {
    new_test_ext().execute_with(|| {
        // register max signers (10)
        for i in 1..=10u64 {
            assert_ok!(FrostBridge::register_signer(
                RuntimeOrigin::signed(i),
                test_encryption_key(i as u8),
            ));
        }

        // 11th should fail
        assert_noop!(
            FrostBridge::register_signer(
                RuntimeOrigin::signed(11),
                test_encryption_key(11),
            ),
            Error::<Test>::TooManySigners
        );
    });
}

#[test]
fn dkg_starts_when_min_signers_reached() {
    new_test_ext().execute_with(|| {
        // need 3 signers minimum
        for i in 1..=3u64 {
            assert_ok!(FrostBridge::register_signer(
                RuntimeOrigin::signed(i),
                test_encryption_key(i as u8),
            ));
        }

        // DKG should have started
        let phase = pallet_frost_bridge::CurrentDkgPhase::<Test>::get();
        assert!(matches!(phase, DkgPhase::Round1 { .. }));
    });
}

// ============ DKG tests ============

#[test]
fn submit_dkg_commitment_works() {
    new_test_ext().execute_with(|| {
        // setup: register 3 signers to start DKG
        for i in 1..=3u64 {
            assert_ok!(FrostBridge::register_signer(
                RuntimeOrigin::signed(i),
                test_encryption_key(i as u8),
            ));
        }

        // should be in round 1
        assert!(matches!(
            pallet_frost_bridge::CurrentDkgPhase::<Test>::get(),
            DkgPhase::Round1 { .. }
        ));

        // submit commitment
        let commitment = test_commitment(1);
        assert_ok!(FrostBridge::submit_dkg_commitment(
            RuntimeOrigin::signed(1),
            commitment,
        ));

        // check commitment was stored
        assert!(pallet_frost_bridge::DkgCommitments::<Test>::contains_key(1));
    });
}

#[test]
fn submit_dkg_commitment_fails_wrong_phase() {
    new_test_ext().execute_with(|| {
        // only 2 signers, DKG hasn't started
        for i in 1..=2u64 {
            assert_ok!(FrostBridge::register_signer(
                RuntimeOrigin::signed(i),
                test_encryption_key(i as u8),
            ));
        }

        assert_noop!(
            FrostBridge::submit_dkg_commitment(
                RuntimeOrigin::signed(1),
                test_commitment(1),
            ),
            Error::<Test>::WrongDkgPhase
        );
    });
}

#[test]
fn dkg_round1_advances_when_all_commitments_received() {
    new_test_ext().execute_with(|| {
        // register 3 signers
        for i in 1..=3u64 {
            assert_ok!(FrostBridge::register_signer(
                RuntimeOrigin::signed(i),
                test_encryption_key(i as u8),
            ));
        }

        // all 3 submit commitments
        for i in 1..=3u64 {
            assert_ok!(FrostBridge::submit_dkg_commitment(
                RuntimeOrigin::signed(i),
                test_commitment(i as u8),
            ));
        }

        // should advance to round 2
        assert!(matches!(
            pallet_frost_bridge::CurrentDkgPhase::<Test>::get(),
            DkgPhase::Round2 { .. }
        ));
    });
}

#[test]
fn dkg_round2_accepts_shares() {
    new_test_ext().execute_with(|| {
        // register 3 signers
        for i in 1..=3u64 {
            assert_ok!(FrostBridge::register_signer(
                RuntimeOrigin::signed(i),
                test_encryption_key(i as u8),
            ));
        }

        // complete round 1
        for i in 1..=3u64 {
            assert_ok!(FrostBridge::submit_dkg_commitment(
                RuntimeOrigin::signed(i),
                test_commitment(i as u8),
            ));
        }

        // now in round 2, submit shares
        let share: BoundedVec<u8, MaxEncryptedShareSize> = vec![1, 2, 3, 4].try_into().unwrap();
        assert_ok!(FrostBridge::submit_dkg_share(
            RuntimeOrigin::signed(1),
            2, // to signer index 2
            share,
        ));

        // check share was stored
        assert!(pallet_frost_bridge::DkgShares::<Test>::contains_key(1, 2));
    });
}

#[test]
fn dkg_round2_advances_when_all_shares_received() {
    new_test_ext().execute_with(|| {
        // register 3 signers
        for i in 1..=3u64 {
            assert_ok!(FrostBridge::register_signer(
                RuntimeOrigin::signed(i),
                test_encryption_key(i as u8),
            ));
        }

        // complete round 1
        for i in 1..=3u64 {
            assert_ok!(FrostBridge::submit_dkg_commitment(
                RuntimeOrigin::signed(i),
                test_commitment(i as u8),
            ));
        }

        // complete round 2: each signer sends to all others (n*(n-1) = 3*2 = 6 shares)
        for from in 1..=3u16 {
            for to in 1..=3u16 {
                if from != to {
                    let share: BoundedVec<u8, MaxEncryptedShareSize> =
                        vec![from as u8, to as u8].try_into().unwrap();
                    assert_ok!(FrostBridge::submit_dkg_share(
                        RuntimeOrigin::signed(from as u64),
                        to,
                        share,
                    ));
                }
            }
        }

        // should advance to round 3
        assert!(matches!(
            pallet_frost_bridge::CurrentDkgPhase::<Test>::get(),
            DkgPhase::Round3 { .. }
        ));
    });
}

#[test]
fn dkg_completes_after_verification() {
    new_test_ext().execute_with(|| {
        // register 3 signers
        for i in 1..=3u64 {
            assert_ok!(FrostBridge::register_signer(
                RuntimeOrigin::signed(i),
                test_encryption_key(i as u8),
            ));
        }

        // complete round 1
        for i in 1..=3u64 {
            assert_ok!(FrostBridge::submit_dkg_commitment(
                RuntimeOrigin::signed(i),
                test_commitment(i as u8),
            ));
        }

        // complete round 2
        for from in 1..=3u16 {
            for to in 1..=3u16 {
                if from != to {
                    let share: BoundedVec<u8, MaxEncryptedShareSize> =
                        vec![from as u8, to as u8].try_into().unwrap();
                    assert_ok!(FrostBridge::submit_dkg_share(
                        RuntimeOrigin::signed(from as u64),
                        to,
                        share,
                    ));
                }
            }
        }

        // should be in round 3 now
        assert!(matches!(
            pallet_frost_bridge::CurrentDkgPhase::<Test>::get(),
            DkgPhase::Round3 { .. }
        ));

        // first verification completes DKG (all signers start as Active, so
        // the check_dkg_complete counts all 3 as Active and finishes)
        assert_ok!(FrostBridge::submit_dkg_verification(
            RuntimeOrigin::signed(1),
            test_public_share(1),
            test_proof(1),
        ));

        // DKG should be complete, back to idle
        assert!(matches!(
            pallet_frost_bridge::CurrentDkgPhase::<Test>::get(),
            DkgPhase::Idle
        ));

        // should have group public key
        assert!(pallet_frost_bridge::GroupPublicKey::<Test>::get().is_some());
    });
}

// ============ signing tests ============

fn setup_complete_dkg() {
    // register 3 signers
    for i in 1..=3u64 {
        let _ = FrostBridge::register_signer(
            RuntimeOrigin::signed(i),
            test_encryption_key(i as u8),
        );
    }

    // complete round 1
    for i in 1..=3u64 {
        let _ = FrostBridge::submit_dkg_commitment(
            RuntimeOrigin::signed(i),
            test_commitment(i as u8),
        );
    }

    // complete round 2
    for from in 1..=3u16 {
        for to in 1..=3u16 {
            if from != to {
                let share: BoundedVec<u8, MaxEncryptedShareSize> =
                    vec![from as u8, to as u8].try_into().unwrap();
                let _ = FrostBridge::submit_dkg_share(
                    RuntimeOrigin::signed(from as u64),
                    to,
                    share,
                );
            }
        }
    }

    // complete round 3 - only need one verification since all signers are already Active
    let _ = FrostBridge::submit_dkg_verification(
        RuntimeOrigin::signed(1),
        test_public_share(1),
        test_proof(1),
    );
}

#[test]
fn request_signature_works() {
    new_test_ext().execute_with(|| {
        setup_complete_dkg();

        let tx_data: BoundedVec<u8, MaxTxDataSize> = vec![1, 2, 3, 4].try_into().unwrap();
        let nonce = [42u8; 32];

        assert_ok!(FrostBridge::request_signature(
            RuntimeOrigin::signed(1),
            tx_data.clone(),
            nonce,
        ));

        // check request was created
        let request = pallet_frost_bridge::SigningQueue::<Test>::get(0).unwrap();
        assert_eq!(request.id, 0);
        assert_eq!(request.requester, 1);
        assert_eq!(request.tx_data, tx_data);
        assert!(matches!(request.status, SigningRequestStatus::WaitingForCommitments));
    });
}

#[test]
fn request_signature_fails_no_group_key() {
    new_test_ext().execute_with(|| {
        // no DKG, so no group key
        let tx_data: BoundedVec<u8, MaxTxDataSize> = vec![1, 2, 3, 4].try_into().unwrap();

        assert_noop!(
            FrostBridge::request_signature(
                RuntimeOrigin::signed(1),
                tx_data,
                [0u8; 32],
            ),
            Error::<Test>::NoGroupKey
        );
    });
}

#[test]
fn request_signature_fails_nonce_reused() {
    new_test_ext().execute_with(|| {
        setup_complete_dkg();

        let tx_data: BoundedVec<u8, MaxTxDataSize> = vec![1, 2, 3, 4].try_into().unwrap();
        let nonce = [42u8; 32];

        assert_ok!(FrostBridge::request_signature(
            RuntimeOrigin::signed(1),
            tx_data.clone(),
            nonce,
        ));

        // same nonce should fail
        assert_noop!(
            FrostBridge::request_signature(
                RuntimeOrigin::signed(1),
                tx_data,
                nonce,
            ),
            Error::<Test>::NonceReused
        );
    });
}

#[test]
fn submit_partial_signature_works() {
    new_test_ext().execute_with(|| {
        setup_complete_dkg();

        // create signing request
        let tx_data: BoundedVec<u8, MaxTxDataSize> = vec![1, 2, 3, 4].try_into().unwrap();
        assert_ok!(FrostBridge::request_signature(
            RuntimeOrigin::signed(1),
            tx_data,
            [42u8; 32],
        ));

        // submit partial sig
        let sig = test_signature(1);
        assert_ok!(FrostBridge::submit_partial_signature(
            RuntimeOrigin::signed(1),
            0, // request id
            1, // signer index
            sig.clone(),
        ));

        // check sig was stored
        let request = pallet_frost_bridge::SigningQueue::<Test>::get(0).unwrap();
        assert_eq!(request.partial_sigs.len(), 1);
        assert_eq!(request.partial_sigs[0], (1, sig));
    });
}

#[test]
fn submit_partial_signature_fails_not_registered() {
    new_test_ext().execute_with(|| {
        setup_complete_dkg();

        let tx_data: BoundedVec<u8, MaxTxDataSize> = vec![1, 2, 3, 4].try_into().unwrap();
        assert_ok!(FrostBridge::request_signature(
            RuntimeOrigin::signed(1),
            tx_data,
            [42u8; 32],
        ));

        // account 99 is not registered
        assert_noop!(
            FrostBridge::submit_partial_signature(
                RuntimeOrigin::signed(99),
                0,
                99,
                test_signature(99),
            ),
            Error::<Test>::NotRegistered
        );
    });
}

#[test]
fn submit_partial_signature_fails_already_signed() {
    new_test_ext().execute_with(|| {
        setup_complete_dkg();

        let tx_data: BoundedVec<u8, MaxTxDataSize> = vec![1, 2, 3, 4].try_into().unwrap();
        assert_ok!(FrostBridge::request_signature(
            RuntimeOrigin::signed(1),
            tx_data,
            [42u8; 32],
        ));

        // first sig succeeds
        assert_ok!(FrostBridge::submit_partial_signature(
            RuntimeOrigin::signed(1),
            0,
            1,
            test_signature(1),
        ));

        // second sig from same signer fails
        assert_noop!(
            FrostBridge::submit_partial_signature(
                RuntimeOrigin::signed(1),
                0,
                1,
                test_signature(1),
            ),
            Error::<Test>::AlreadySigned
        );
    });
}

#[test]
fn signing_completes_at_threshold() {
    new_test_ext().execute_with(|| {
        setup_complete_dkg();

        let tx_data: BoundedVec<u8, MaxTxDataSize> = vec![1, 2, 3, 4].try_into().unwrap();
        assert_ok!(FrostBridge::request_signature(
            RuntimeOrigin::signed(1),
            tx_data,
            [42u8; 32],
        ));

        // threshold is 2, so 2 sigs should complete
        assert_ok!(FrostBridge::submit_partial_signature(
            RuntimeOrigin::signed(1),
            0,
            1,
            test_signature(1),
        ));

        assert_ok!(FrostBridge::submit_partial_signature(
            RuntimeOrigin::signed(2),
            0,
            2,
            test_signature(2),
        ));

        // check signing completed
        let request = pallet_frost_bridge::SigningQueue::<Test>::get(0).unwrap();
        assert!(request.final_sig.is_some());
    });
}

// ============ liveness tests ============

#[test]
fn heartbeat_works() {
    new_test_ext().execute_with(|| {
        // register signer
        assert_ok!(FrostBridge::register_signer(
            RuntimeOrigin::signed(1),
            test_encryption_key(1),
        ));

        // submit heartbeat (placeholder verification)
        let challenge_response = [0u8; 64];
        assert_ok!(FrostBridge::submit_heartbeat(
            RuntimeOrigin::signed(1),
            challenge_response,
        ));

        // check heartbeat was recorded
        let last_heartbeat = pallet_frost_bridge::LastHeartbeat::<Test>::get(1);
        assert_eq!(last_heartbeat, 1); // block 1
    });
}

#[test]
fn heartbeat_reactivates_frozen_signer() {
    new_test_ext().execute_with(|| {
        // register signer
        assert_ok!(FrostBridge::register_signer(
            RuntimeOrigin::signed(1),
            test_encryption_key(1),
        ));

        // manually freeze signer
        let mut signer = pallet_frost_bridge::Signers::<Test>::get(1).unwrap();
        signer.status = SignerStatus::Frozen {
            since_block: 1,
            reason: FreezeReason::MissedSigning,
        };
        pallet_frost_bridge::Signers::<Test>::insert(1, signer);

        // submit heartbeat
        assert_ok!(FrostBridge::submit_heartbeat(
            RuntimeOrigin::signed(1),
            [0u8; 64],
        ));

        // should be reactivated
        let signer = pallet_frost_bridge::Signers::<Test>::get(1).unwrap();
        assert!(matches!(signer.status, SignerStatus::Active));
    });
}

// ============ circuit breaker tests ============

#[test]
fn halt_bridge_works() {
    new_test_ext().execute_with(|| {
        // initially active
        assert!(matches!(
            pallet_frost_bridge::CurrentBridgeState::<Test>::get(),
            BridgeState::Active
        ));

        // halt
        assert_ok!(FrostBridge::halt_bridge(RuntimeOrigin::root()));

        // should be halted
        assert!(matches!(
            pallet_frost_bridge::CurrentBridgeState::<Test>::get(),
            BridgeState::CircuitBroken { .. }
        ));
    });
}

#[test]
fn resume_bridge_works() {
    new_test_ext().execute_with(|| {
        // halt first
        assert_ok!(FrostBridge::halt_bridge(RuntimeOrigin::root()));

        // resume
        assert_ok!(FrostBridge::resume_bridge(RuntimeOrigin::root()));

        // should be active
        assert!(matches!(
            pallet_frost_bridge::CurrentBridgeState::<Test>::get(),
            BridgeState::Active
        ));
    });
}

#[test]
fn resume_bridge_fails_if_not_halted() {
    new_test_ext().execute_with(|| {
        // try to resume when already active
        assert_noop!(
            FrostBridge::resume_bridge(RuntimeOrigin::root()),
            Error::<Test>::NotHalted
        );
    });
}

#[test]
fn emergency_recovery_works() {
    new_test_ext().execute_with(|| {
        let recovery_address = [42u8; 64];

        assert_ok!(FrostBridge::initiate_emergency_recovery(
            RuntimeOrigin::root(),
            recovery_address,
        ));

        assert!(matches!(
            pallet_frost_bridge::CurrentBridgeState::<Test>::get(),
            BridgeState::EmergencyRecovery { .. }
        ));
    });
}

#[test]
fn emergency_recovery_fails_if_already_in_progress() {
    new_test_ext().execute_with(|| {
        let recovery_address = [42u8; 64];

        assert_ok!(FrostBridge::initiate_emergency_recovery(
            RuntimeOrigin::root(),
            recovery_address,
        ));

        assert_noop!(
            FrostBridge::initiate_emergency_recovery(
                RuntimeOrigin::root(),
                [99u8; 64],
            ),
            Error::<Test>::RecoveryInProgress
        );
    });
}

// ============ report missing signer tests ============

#[test]
fn report_missing_signer_fails_if_time_not_expired() {
    new_test_ext().execute_with(|| {
        setup_complete_dkg();

        // create signing request
        let tx_data: BoundedVec<u8, MaxTxDataSize> = vec![1, 2, 3, 4].try_into().unwrap();
        assert_ok!(FrostBridge::request_signature(
            RuntimeOrigin::signed(1),
            tx_data,
            [42u8; 32],
        ));

        // try to report immediately (deadline not passed)
        assert_noop!(
            FrostBridge::report_missing_signer(
                RuntimeOrigin::signed(1),
                2, // report signer 2
                0, // request id
            ),
            Error::<Test>::TimeNotExpired
        );
    });
}

#[test]
fn report_missing_signer_fails_if_signer_participated() {
    new_test_ext().execute_with(|| {
        setup_complete_dkg();

        // create signing request
        let tx_data: BoundedVec<u8, MaxTxDataSize> = vec![1, 2, 3, 4].try_into().unwrap();
        assert_ok!(FrostBridge::request_signature(
            RuntimeOrigin::signed(1),
            tx_data,
            [42u8; 32],
        ));

        // signer 2 participates
        assert_ok!(FrostBridge::submit_partial_signature(
            RuntimeOrigin::signed(2),
            0,
            2,
            test_signature(2),
        ));

        // advance past deadline
        System::set_block_number(100);

        // try to report signer 2 who participated
        assert_noop!(
            FrostBridge::report_missing_signer(
                RuntimeOrigin::signed(1),
                2,
                0,
            ),
            Error::<Test>::SignerNotMissing
        );
    });
}

// ============ DKG timeout tests ============

#[test]
fn dkg_times_out() {
    new_test_ext().execute_with(|| {
        // register 3 signers to start DKG
        for i in 1..=3u64 {
            assert_ok!(FrostBridge::register_signer(
                RuntimeOrigin::signed(i),
                test_encryption_key(i as u8),
            ));
        }

        // should be in round 1
        let phase = pallet_frost_bridge::CurrentDkgPhase::<Test>::get();
        let deadline = match phase {
            DkgPhase::Round1 { deadline } => deadline,
            _ => panic!("expected round 1"),
        };

        // advance past deadline
        let past_deadline = (deadline + 1) as u64;
        System::set_block_number(past_deadline);

        // trigger on_finalize
        FrostBridge::on_finalize(past_deadline);

        // should be failed
        assert!(matches!(
            pallet_frost_bridge::CurrentDkgPhase::<Test>::get(),
            DkgPhase::Failed { reason: DkgFailureReason::Timeout }
        ));
    });
}

// ============ bridge interface tests ============

#[test]
fn frost_bridge_interface_is_bridge_active() {
    new_test_ext().execute_with(|| {
        use pallet_frost_bridge::FrostBridgeInterface;

        // initially active
        assert!(FrostBridge::is_bridge_active());

        // halt
        assert_ok!(FrostBridge::halt_bridge(RuntimeOrigin::root()));

        // not active
        assert!(!FrostBridge::is_bridge_active());
    });
}

#[test]
fn frost_bridge_interface_custody_address() {
    new_test_ext().execute_with(|| {
        use pallet_frost_bridge::FrostBridgeInterface;

        // no custody address before DKG
        assert!(FrostBridge::custody_address().is_none());

        // complete DKG
        setup_complete_dkg();

        // should have custody address
        let addr = FrostBridge::custody_address();
        assert!(addr.is_some());
        assert!(matches!(addr.unwrap().address_type, BtcAddressType::P2TR));
    });
}

// ============ participation stats tests ============

#[test]
fn participation_stats_update_on_signing() {
    new_test_ext().execute_with(|| {
        setup_complete_dkg();

        // create signing request
        let tx_data: BoundedVec<u8, MaxTxDataSize> = vec![1, 2, 3, 4].try_into().unwrap();
        assert_ok!(FrostBridge::request_signature(
            RuntimeOrigin::signed(1),
            tx_data,
            [42u8; 32],
        ));

        // initial stats should be zero
        let stats_before = pallet_frost_bridge::SignerParticipation::<Test>::get(1);
        assert_eq!(stats_before.signing_rounds_participated, 0);

        // signer 1 and 2 sign (reaching threshold)
        assert_ok!(FrostBridge::submit_partial_signature(
            RuntimeOrigin::signed(1),
            0,
            1,
            test_signature(1),
        ));
        assert_ok!(FrostBridge::submit_partial_signature(
            RuntimeOrigin::signed(2),
            0,
            2,
            test_signature(2),
        ));

        // stats should be updated for participants
        let stats_after = pallet_frost_bridge::SignerParticipation::<Test>::get(1);
        assert_eq!(stats_after.signing_rounds_participated, 1);
        assert_eq!(stats_after.last_participation_block, 1);
    });
}
