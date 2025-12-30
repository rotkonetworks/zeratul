//! integration tests for zanchor runtime
//!
//! tests full flows across pallets:
//! - frost-bridge dkg + custody setup
//! - shield/spend flow in shielded pool
//! - custody deposit/withdrawal flow

use crate::*;
use frame_support::{
    assert_ok, assert_noop,
    traits::{fungibles::Mutate as FungiblesMutate, Hooks},
    pallet_prelude::*,
};
use sp_runtime::{BuildStorage, MultiAddress};
use sp_core::H256;
use pallet_frost_bridge::FrostBridgeInterface;

// ============ test setup ============

fn new_test_ext() -> sp_io::TestExternalities {
    let mut t = frame_system::GenesisConfig::<Runtime>::default()
        .build_storage()
        .unwrap();

    // fund some accounts
    pallet_balances::GenesisConfig::<Runtime> {
        balances: vec![
            (AccountId::from([1u8; 32]), 1_000_000 * UNIT),
            (AccountId::from([2u8; 32]), 1_000_000 * UNIT),
            (AccountId::from([3u8; 32]), 1_000_000 * UNIT),
            (AccountId::from([10u8; 32]), 1_000_000 * UNIT),
            (AccountId::from([11u8; 32]), 1_000_000 * UNIT),
            (AccountId::from([12u8; 32]), 1_000_000 * UNIT),
        ],
        ..Default::default()
    }
    .assimilate_storage(&mut t)
    .unwrap();

    let mut ext = sp_io::TestExternalities::new(t);
    ext.execute_with(|| {
        System::set_block_number(1);
    });
    ext
}

fn account(id: u8) -> AccountId {
    AccountId::from([id; 32])
}

fn test_encryption_key(seed: u8) -> [u8; 32] {
    [seed; 32]
}

fn test_commitment(seed: u8) -> pallet_shielded_pool::NoteCommitment {
    pallet_shielded_pool::NoteCommitment(H256::from([seed; 32]))
}

fn test_nullifier(seed: u8) -> pallet_shielded_pool::Nullifier {
    pallet_shielded_pool::Nullifier(H256::from([seed; 32]))
}

// ============ helper to setup DKG ============

fn setup_signers_and_dkg() {
    // register 3 signers
    for i in 10..=12u8 {
        assert_ok!(FrostBridge::register_signer(
            RuntimeOrigin::signed(account(i)),
            test_encryption_key(i),
        ));
    }

    // complete round 1 commitments
    for i in 10..=12u8 {
        assert_ok!(FrostBridge::submit_dkg_commitment(
            RuntimeOrigin::signed(account(i)),
            [i; 32],
        ));
    }

    // complete round 2 shares
    for from in 1..=3u16 {
        for to in 1..=3u16 {
            if from != to {
                let from_account = account(from as u8 + 9);
                let share: BoundedVec<u8, pallet_frost_bridge::MaxEncryptedShareSize> =
                    vec![from as u8, to as u8].try_into().unwrap();
                assert_ok!(FrostBridge::submit_dkg_share(
                    RuntimeOrigin::signed(from_account),
                    to,
                    share,
                ));
            }
        }
    }

    // complete round 3 verification (just one needed)
    assert_ok!(FrostBridge::submit_dkg_verification(
        RuntimeOrigin::signed(account(10)),
        [10u8; 32],
        [10u8; 64],
    ));

    // verify DKG complete
    assert!(pallet_frost_bridge::GroupPublicKey::<Runtime>::get().is_some());
}

// ============ helper to setup assets ============

fn setup_assets() {
    // create zbtc asset (id=1)
    assert_ok!(Assets::force_create(
        RuntimeOrigin::root(),
        1.into(),
        MultiAddress::Id(account(1)),
        true,
        1,
    ));

    // create zzec asset (id=2)
    assert_ok!(Assets::force_create(
        RuntimeOrigin::root(),
        2.into(),
        MultiAddress::Id(account(1)),
        true,
        1,
    ));
}

fn mint_zbtc_to(who: AccountId, amount: Balance) {
    let _ = <Assets as FungiblesMutate<AccountId>>::mint_into(1, &who, amount);
}

// ============ frost-bridge integration tests ============

#[test]
fn frost_bridge_dkg_flow_works() {
    new_test_ext().execute_with(|| {
        setup_signers_and_dkg();

        // check bridge is active
        assert!(FrostBridge::is_bridge_active());

        // check custody address is available
        let custody_addr = FrostBridge::custody_address();
        assert!(custody_addr.is_some());
    });
}

#[test]
fn frost_bridge_signing_request_works() {
    new_test_ext().execute_with(|| {
        setup_signers_and_dkg();

        // request a signature
        let tx_data: BoundedVec<u8, pallet_frost_bridge::MaxTxDataSize> =
            vec![1, 2, 3, 4].try_into().unwrap();

        assert_ok!(FrostBridge::request_signature(
            RuntimeOrigin::signed(account(1)),
            tx_data,
            [42u8; 32],
        ));

        // check request was created
        let request = pallet_frost_bridge::SigningQueue::<Runtime>::get(0);
        assert!(request.is_some());
    });
}

// ============ shielded pool integration tests ============

#[test]
fn shielded_pool_shield_flow_works() {
    new_test_ext().execute_with(|| {
        setup_assets();

        // mint zbtc to user
        let user = account(1);
        mint_zbtc_to(user.clone(), 100_000_000);

        // verify balance
        assert_eq!(Assets::balance(1, &user), 100_000_000);

        // shield some btc
        let commitment = test_commitment(1);
        assert_ok!(ShieldedPool::shield(
            RuntimeOrigin::signed(user.clone()),
            pallet_shielded_pool::AssetType::Btc,
            50_000,
            commitment,
        ));

        // check commitment was added
        assert!(pallet_shielded_pool::CommitmentTree::<Runtime>::contains_key(
            pallet_shielded_pool::TreePosition(0)
        ));

        // check balance decreased
        assert_eq!(Assets::balance(1, &user), 100_000_000 - 50_000);

        // check total shielded increased
        assert_eq!(
            pallet_shielded_pool::TotalShielded::<Runtime>::get(pallet_shielded_pool::AssetType::Btc),
            50_000
        );
    });
}

#[test]
fn shielded_pool_spend_flow_works() {
    new_test_ext().execute_with(|| {
        setup_assets();

        let user = account(1);
        mint_zbtc_to(user.clone(), 100_000_000);

        // shield first
        let commitment1 = test_commitment(1);
        assert_ok!(ShieldedPool::shield(
            RuntimeOrigin::signed(user.clone()),
            pallet_shielded_pool::AssetType::Btc,
            50_000,
            commitment1,
        ));

        // get anchor
        let anchor = pallet_shielded_pool::CurrentRoot::<Runtime>::get();

        // spend
        let nullifier = test_nullifier(1);
        let new_commitment = test_commitment(2);
        let spend_proof = pallet_shielded_pool::SpendProof { proof: Default::default() };
        let output_proof = pallet_shielded_pool::OutputProof { proof: Default::default() };

        assert_ok!(ShieldedPool::spend(
            RuntimeOrigin::signed(user.clone()),
            anchor,
            nullifier,
            spend_proof,
            new_commitment,
            output_proof,
        ));

        // check nullifier spent
        assert!(pallet_shielded_pool::SpentNullifiers::<Runtime>::contains_key(nullifier));

        // check new commitment added at position 1
        assert!(pallet_shielded_pool::CommitmentTree::<Runtime>::contains_key(
            pallet_shielded_pool::TreePosition(1)
        ));
    });
}

#[test]
fn shielded_pool_prevents_double_spend() {
    new_test_ext().execute_with(|| {
        setup_assets();

        let user = account(1);
        mint_zbtc_to(user.clone(), 100_000_000);

        // shield
        assert_ok!(ShieldedPool::shield(
            RuntimeOrigin::signed(user.clone()),
            pallet_shielded_pool::AssetType::Btc,
            50_000,
            test_commitment(1),
        ));

        let anchor = pallet_shielded_pool::CurrentRoot::<Runtime>::get();
        let nullifier = test_nullifier(1);
        let spend_proof = pallet_shielded_pool::SpendProof { proof: Default::default() };
        let output_proof = pallet_shielded_pool::OutputProof { proof: Default::default() };

        // first spend
        assert_ok!(ShieldedPool::spend(
            RuntimeOrigin::signed(user.clone()),
            anchor,
            nullifier,
            spend_proof.clone(),
            test_commitment(2),
            output_proof.clone(),
        ));

        // get new anchor
        let new_anchor = pallet_shielded_pool::CurrentRoot::<Runtime>::get();

        // second spend with same nullifier fails
        assert_noop!(
            ShieldedPool::spend(
                RuntimeOrigin::signed(user.clone()),
                new_anchor,
                nullifier, // same nullifier
                spend_proof,
                test_commitment(3),
                output_proof,
            ),
            pallet_shielded_pool::Error::<Runtime>::NullifierAlreadySpent
        );
    });
}

// ============ full flow integration test ============

#[test]
fn full_deposit_shield_spend_flow() {
    new_test_ext().execute_with(|| {
        // 1. setup DKG so bridge is operational
        setup_signers_and_dkg();
        setup_assets();

        let user = account(1);

        // 2. simulate deposit: mint zbtc (normally done by custody pallet on deposit confirmation)
        mint_zbtc_to(user.clone(), 100_000_000); // 1 btc in sats

        // 3. user shields their btc for privacy
        let commitment1 = test_commitment(1);
        assert_ok!(ShieldedPool::shield(
            RuntimeOrigin::signed(user.clone()),
            pallet_shielded_pool::AssetType::Btc,
            50_000_000, // 0.5 btc
            commitment1,
        ));

        // 4. verify shield worked
        assert_eq!(Assets::balance(1, &user), 50_000_000); // remaining 0.5 btc
        assert_eq!(
            pallet_shielded_pool::TotalShielded::<Runtime>::get(pallet_shielded_pool::AssetType::Btc),
            50_000_000
        );

        // 5. user spends shielded note privately
        let anchor = pallet_shielded_pool::CurrentRoot::<Runtime>::get();
        let nullifier = test_nullifier(1);
        let spend_proof = pallet_shielded_pool::SpendProof { proof: Default::default() };
        let output_proof = pallet_shielded_pool::OutputProof { proof: Default::default() };
        let new_commitment = test_commitment(2);

        assert_ok!(ShieldedPool::spend(
            RuntimeOrigin::signed(user.clone()),
            anchor,
            nullifier,
            spend_proof,
            new_commitment, // new note
            output_proof,
        ));

        // 6. verify spend
        assert!(pallet_shielded_pool::SpentNullifiers::<Runtime>::contains_key(nullifier));

        // 7. simulate withdrawal: signers mark nullifier spent
        let withdrawal_nullifier = test_nullifier(100);
        assert_ok!(ShieldedPool::mark_nullifier_spent(
            RuntimeOrigin::signed(account(10)), // signer
            withdrawal_nullifier,
            pallet_shielded_pool::AssetType::Btc,
            10_000_000, // 0.1 btc withdrawal
        ));

        // note: in real flow, signers would FROST sign the btc tx
        // then broadcast, then mark nullifier spent on-chain
    });
}

// ============ cross-pallet interaction tests ============

#[test]
fn frost_bridge_interface_works_from_other_pallets() {
    new_test_ext().execute_with(|| {
        use pallet_frost_bridge::FrostBridgeInterface;

        // before DKG, bridge has no custody address
        assert!(FrostBridge::custody_address().is_none());

        // setup DKG
        setup_signers_and_dkg();

        // now we have custody address
        let addr = FrostBridge::custody_address();
        assert!(addr.is_some());
        assert!(matches!(
            addr.unwrap().address_type,
            pallet_frost_bridge::BtcAddressType::P2TR
        ));

        // bridge is active
        assert!(FrostBridge::is_bridge_active());

        // can request signature via interface
        let request_id = <FrostBridge as FrostBridgeInterface<AccountId>>::request_signature(
            account(1),
            vec![1, 2, 3, 4],
            100, // deadline
        );
        assert!(request_id.is_ok());
    });
}

#[test]
fn shielded_pool_enforces_minimum_shield() {
    new_test_ext().execute_with(|| {
        setup_assets();

        let user = account(1);
        mint_zbtc_to(user.clone(), 100_000_000);

        // try to shield below minimum
        assert_noop!(
            ShieldedPool::shield(
                RuntimeOrigin::signed(user.clone()),
                pallet_shielded_pool::AssetType::Btc,
                1000, // below MinShieldAmount (10_000)
                test_commitment(1),
            ),
            pallet_shielded_pool::Error::<Runtime>::AmountBelowMinimum
        );
    });
}

#[test]
fn shielded_pool_historical_anchor_works() {
    new_test_ext().execute_with(|| {
        setup_assets();

        let user = account(1);
        mint_zbtc_to(user.clone(), 100_000_000);

        // shield once
        assert_ok!(ShieldedPool::shield(
            RuntimeOrigin::signed(user.clone()),
            pallet_shielded_pool::AssetType::Btc,
            50_000,
            test_commitment(1),
        ));

        let old_anchor = pallet_shielded_pool::CurrentRoot::<Runtime>::get();

        // shield again (changes root)
        assert_ok!(ShieldedPool::shield(
            RuntimeOrigin::signed(user.clone()),
            pallet_shielded_pool::AssetType::Btc,
            50_000,
            test_commitment(2),
        ));

        // old anchor should still be valid for spend
        let nullifier = test_nullifier(1);
        let spend_proof = pallet_shielded_pool::SpendProof { proof: Default::default() };
        let output_proof = pallet_shielded_pool::OutputProof { proof: Default::default() };

        assert_ok!(ShieldedPool::spend(
            RuntimeOrigin::signed(user.clone()),
            old_anchor, // using old anchor
            nullifier,
            spend_proof,
            test_commitment(3),
            output_proof,
        ));
    });
}

// ============ bridge state tests ============

#[test]
fn bridge_halt_prevents_operations() {
    new_test_ext().execute_with(|| {
        setup_signers_and_dkg();

        // halt bridge
        assert_ok!(FrostBridge::halt_bridge(RuntimeOrigin::root()));

        // bridge should not be active
        assert!(!FrostBridge::is_bridge_active());

        // resume
        assert_ok!(FrostBridge::resume_bridge(RuntimeOrigin::root()));

        // bridge active again
        assert!(FrostBridge::is_bridge_active());
    });
}
