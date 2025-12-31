//! tests for shielded-pool pallet

use crate::{self as pallet_shielded_pool, *};
use frame_support::{
    assert_noop, assert_ok, derive_impl,
    parameter_types,
    traits::{AsEnsureOriginWithArg, ConstU32},
};
use sp_core::H256;
use sp_runtime::{
    traits::{BlakeTwo256, IdentityLookup},
    BuildStorage,
};

type Block = frame_system::mocking::MockBlock<Test>;

frame_support::construct_runtime!(
    pub enum Test {
        System: frame_system,
        Balances: pallet_balances,
        Assets: pallet_assets,
        ShieldedPool: pallet_shielded_pool,
    }
);

#[derive_impl(frame_system::config_preludes::TestDefaultConfig)]
impl frame_system::Config for Test {
    type Block = Block;
    type AccountData = pallet_balances::AccountData<u128>;
}

parameter_types! {
    pub const ExistentialDeposit: u128 = 1;
}

#[derive_impl(pallet_balances::config_preludes::TestDefaultConfig)]
impl pallet_balances::Config for Test {
    type Balance = u128;
    type ExistentialDeposit = ExistentialDeposit;
    type AccountStore = System;
}

parameter_types! {
    pub const AssetDeposit: u128 = 0;
    pub const AssetAccountDeposit: u128 = 0;
    pub const ApprovalDeposit: u128 = 0;
    pub const StringLimit: u32 = 50;
    pub const MetadataDepositBase: u128 = 0;
    pub const MetadataDepositPerByte: u128 = 0;
}

#[derive_impl(pallet_assets::config_preludes::TestDefaultConfig)]
impl pallet_assets::Config for Test {
    type Balance = u128;
    type AssetId = u32;
    type AssetIdParameter = u32;
    type Currency = Balances;
    type CreateOrigin = AsEnsureOriginWithArg<frame_system::EnsureSigned<u64>>;
    type ForceOrigin = frame_system::EnsureRoot<u64>;
    type AssetDeposit = AssetDeposit;
    type AssetAccountDeposit = AssetAccountDeposit;
    type MetadataDepositBase = MetadataDepositBase;
    type MetadataDepositPerByte = MetadataDepositPerByte;
    type ApprovalDeposit = ApprovalDeposit;
    type StringLimit = StringLimit;
    type RemoveItemsLimit = ConstU32<1000>;
}

parameter_types! {
    pub const ZbtcAssetId: u32 = 1;
    pub const ZzecAssetId: u32 = 2;
    pub const MinShieldAmount: u64 = 1000;
    pub const RootHistorySize: u32 = 100;
}

impl pallet_shielded_pool::Config for Test {
    type RuntimeEvent = RuntimeEvent;
    type AssetId = u32;
    type Balance = u128;
    type Assets = Assets;
    type ZbtcAssetId = ZbtcAssetId;
    type ZzecAssetId = ZzecAssetId;
    type MinShieldAmount = MinShieldAmount;
    type RootHistorySize = RootHistorySize;
}

// helper to create test commitment
fn test_commitment(seed: u8) -> NoteCommitment {
    NoteCommitment(H256::from([seed; 32]))
}

// helper to create test nullifier
fn test_nullifier(seed: u8) -> Nullifier {
    Nullifier(H256::from([seed; 32]))
}

// helper to create test merkle root
fn test_root(seed: u8) -> MerkleRoot {
    MerkleRoot(H256::from([seed; 32]))
}

// build test externalities
pub fn new_test_ext() -> sp_io::TestExternalities {
    let mut t = frame_system::GenesisConfig::<Test>::default()
        .build_storage()
        .unwrap();

    pallet_balances::GenesisConfig::<Test> {
        balances: vec![(1, 1_000_000), (2, 1_000_000), (3, 1_000_000)],
        ..Default::default()
    }
    .assimilate_storage(&mut t)
    .unwrap();

    let mut ext = sp_io::TestExternalities::new(t);
    ext.execute_with(|| {
        System::set_block_number(1);

        // create zbtc asset (id=1)
        assert_ok!(Assets::force_create(
            RuntimeOrigin::root(),
            1,
            1, // admin
            true,
            1, // min balance
        ));

        // create zzec asset (id=2)
        assert_ok!(Assets::force_create(
            RuntimeOrigin::root(),
            2,
            1, // admin
            true,
            1, // min balance
        ));

        // mint some zbtc to account 1
        assert_ok!(Assets::mint(
            RuntimeOrigin::signed(1),
            1, // asset id
            1, // beneficiary
            100_000_000, // 1 btc in sats
        ));

        // mint some zbtc to account 2
        assert_ok!(Assets::mint(
            RuntimeOrigin::signed(1),
            1,
            2,
            50_000_000,
        ));
    });
    ext
}

// ============ shield tests ============

#[test]
fn shield_works() {
    new_test_ext().execute_with(|| {
        let commitment = test_commitment(1);

        // account 1 shields 10000 sats
        assert_ok!(ShieldedPool::shield(
            RuntimeOrigin::signed(1),
            AssetType::Btc,
            10_000,
            commitment,
        ));

        // check commitment was added to tree
        assert!(pallet_shielded_pool::CommitmentTree::<Test>::contains_key(TreePosition(0)));
        assert_eq!(
            pallet_shielded_pool::CommitmentTree::<Test>::get(TreePosition(0)),
            Some(commitment)
        );

        // check next position incremented
        assert_eq!(
            pallet_shielded_pool::NextTreePosition::<Test>::get(),
            TreePosition(1)
        );

        // check total shielded updated
        assert_eq!(
            pallet_shielded_pool::TotalShielded::<Test>::get(AssetType::Btc),
            10_000
        );

        // check zbtc was burned from account
        assert_eq!(
            Assets::balance(1, &1),
            100_000_000 - 10_000
        );
    });
}

#[test]
fn shield_fails_below_minimum() {
    new_test_ext().execute_with(|| {
        let commitment = test_commitment(1);

        // try to shield less than minimum (1000)
        assert_noop!(
            ShieldedPool::shield(
                RuntimeOrigin::signed(1),
                AssetType::Btc,
                500, // below minimum
                commitment,
            ),
            Error::<Test>::AmountBelowMinimum
        );
    });
}

#[test]
fn shield_fails_insufficient_balance() {
    new_test_ext().execute_with(|| {
        let commitment = test_commitment(1);

        // account 3 has no zbtc
        assert!(ShieldedPool::shield(
            RuntimeOrigin::signed(3),
            AssetType::Btc,
            10_000,
            commitment,
        ).is_err());
    });
}

#[test]
fn multiple_shields_increment_position() {
    new_test_ext().execute_with(|| {
        // shield 3 times
        for i in 0..3u8 {
            let commitment = test_commitment(i);
            assert_ok!(ShieldedPool::shield(
                RuntimeOrigin::signed(1),
                AssetType::Btc,
                10_000,
                commitment,
            ));
        }

        // check positions
        assert_eq!(
            pallet_shielded_pool::NextTreePosition::<Test>::get(),
            TreePosition(3)
        );

        // check all commitments stored
        for i in 0..3u8 {
            assert_eq!(
                pallet_shielded_pool::CommitmentTree::<Test>::get(TreePosition(i as u64)),
                Some(test_commitment(i))
            );
        }
    });
}

// ============ spend tests ============

#[test]
fn spend_works() {
    new_test_ext().execute_with(|| {
        // first shield
        let commitment1 = test_commitment(1);
        assert_ok!(ShieldedPool::shield(
            RuntimeOrigin::signed(1),
            AssetType::Btc,
            10_000,
            commitment1,
        ));

        // get current root (this is the anchor)
        let anchor = pallet_shielded_pool::CurrentRoot::<Test>::get();

        let nullifier = test_nullifier(1);
        let new_commitment = test_commitment(2);
        let spend_proof = SpendProof { proof: Default::default() };
        let output_proof = OutputProof { proof: Default::default() };

        // spend
        assert_ok!(ShieldedPool::spend(
            RuntimeOrigin::signed(1),
            anchor,
            nullifier,
            spend_proof,
            new_commitment,
            output_proof,
        ));

        // check nullifier is marked spent
        assert!(pallet_shielded_pool::SpentNullifiers::<Test>::contains_key(nullifier));

        // check new commitment added
        assert_eq!(
            pallet_shielded_pool::CommitmentTree::<Test>::get(TreePosition(1)),
            Some(new_commitment)
        );
    });
}

#[test]
fn spend_fails_double_spend() {
    new_test_ext().execute_with(|| {
        // shield first
        let commitment = test_commitment(1);
        assert_ok!(ShieldedPool::shield(
            RuntimeOrigin::signed(1),
            AssetType::Btc,
            10_000,
            commitment,
        ));

        let anchor = pallet_shielded_pool::CurrentRoot::<Test>::get();
        let nullifier = test_nullifier(1);
        let spend_proof = SpendProof { proof: Default::default() };
        let output_proof = OutputProof { proof: Default::default() };

        // first spend succeeds
        assert_ok!(ShieldedPool::spend(
            RuntimeOrigin::signed(1),
            anchor,
            nullifier,
            spend_proof.clone(),
            test_commitment(2),
            output_proof.clone(),
        ));

        // update anchor for second spend attempt
        let new_anchor = pallet_shielded_pool::CurrentRoot::<Test>::get();

        // second spend with same nullifier fails
        assert_noop!(
            ShieldedPool::spend(
                RuntimeOrigin::signed(1),
                new_anchor,
                nullifier, // same nullifier
                spend_proof,
                test_commitment(3),
                output_proof,
            ),
            Error::<Test>::NullifierAlreadySpent
        );
    });
}

#[test]
fn spend_fails_invalid_anchor() {
    new_test_ext().execute_with(|| {
        // shield first
        let commitment = test_commitment(1);
        assert_ok!(ShieldedPool::shield(
            RuntimeOrigin::signed(1),
            AssetType::Btc,
            10_000,
            commitment,
        ));

        // use invalid anchor
        let invalid_anchor = test_root(99);
        let nullifier = test_nullifier(1);
        let spend_proof = SpendProof { proof: Default::default() };
        let output_proof = OutputProof { proof: Default::default() };

        assert_noop!(
            ShieldedPool::spend(
                RuntimeOrigin::signed(1),
                invalid_anchor,
                nullifier,
                spend_proof,
                test_commitment(2),
                output_proof,
            ),
            Error::<Test>::InvalidAnchor
        );
    });
}

#[test]
fn historical_anchor_works() {
    new_test_ext().execute_with(|| {
        // shield once
        let commitment1 = test_commitment(1);
        assert_ok!(ShieldedPool::shield(
            RuntimeOrigin::signed(1),
            AssetType::Btc,
            10_000,
            commitment1,
        ));

        // save this anchor
        let old_anchor = pallet_shielded_pool::CurrentRoot::<Test>::get();

        // shield again (changes the root)
        let commitment2 = test_commitment(2);
        assert_ok!(ShieldedPool::shield(
            RuntimeOrigin::signed(1),
            AssetType::Btc,
            10_000,
            commitment2,
        ));

        // old anchor should still be valid (in historical roots)
        let nullifier = test_nullifier(1);
        let spend_proof = SpendProof { proof: Default::default() };
        let output_proof = OutputProof { proof: Default::default() };

        assert_ok!(ShieldedPool::spend(
            RuntimeOrigin::signed(1),
            old_anchor, // using old anchor
            nullifier,
            spend_proof,
            test_commitment(3),
            output_proof,
        ));
    });
}

// ============ mark_nullifier_spent tests ============

#[test]
fn mark_nullifier_spent_works() {
    new_test_ext().execute_with(|| {
        // first shield some value
        let commitment = test_commitment(1);
        assert_ok!(ShieldedPool::shield(
            RuntimeOrigin::signed(1),
            AssetType::Btc,
            50_000,
            commitment,
        ));

        let nullifier = test_nullifier(1);

        // mark nullifier spent (simulates withdrawal completion by signers)
        assert_ok!(ShieldedPool::mark_nullifier_spent(
            RuntimeOrigin::signed(1), // todo: should be signer origin
            nullifier,
            AssetType::Btc,
            50_000,
        ));

        // check nullifier is spent
        assert!(pallet_shielded_pool::SpentNullifiers::<Test>::contains_key(nullifier));

        // check total shielded decreased
        assert_eq!(
            pallet_shielded_pool::TotalShielded::<Test>::get(AssetType::Btc),
            0
        );
    });
}

#[test]
fn mark_nullifier_spent_fails_already_spent() {
    new_test_ext().execute_with(|| {
        let commitment = test_commitment(1);
        assert_ok!(ShieldedPool::shield(
            RuntimeOrigin::signed(1),
            AssetType::Btc,
            50_000,
            commitment,
        ));

        let nullifier = test_nullifier(1);

        // first mark succeeds
        assert_ok!(ShieldedPool::mark_nullifier_spent(
            RuntimeOrigin::signed(1),
            nullifier,
            AssetType::Btc,
            25_000,
        ));

        // second mark fails
        assert_noop!(
            ShieldedPool::mark_nullifier_spent(
                RuntimeOrigin::signed(1),
                nullifier,
                AssetType::Btc,
                25_000,
            ),
            Error::<Test>::NullifierAlreadySpent
        );
    });
}

// ============ root update tests ============

#[test]
fn root_updates_on_shield() {
    new_test_ext().execute_with(|| {
        let initial_root = pallet_shielded_pool::CurrentRoot::<Test>::get();

        let commitment = test_commitment(1);
        assert_ok!(ShieldedPool::shield(
            RuntimeOrigin::signed(1),
            AssetType::Btc,
            10_000,
            commitment,
        ));

        let new_root = pallet_shielded_pool::CurrentRoot::<Test>::get();
        assert_ne!(initial_root, new_root);

        // old root should be in history
        assert!(pallet_shielded_pool::HistoricalRoots::<Test>::contains_key(initial_root));
    });
}
