//! Zanchor Runtime
//!
//! Zcash trust anchor parachain for Polkadot.
//! Provides trustless light client attestations with economic security.

#![cfg_attr(not(feature = "std"), no_std)]
#![recursion_limit = "256"]

#[cfg(feature = "std")]
include!(concat!(env!("OUT_DIR"), "/wasm_binary.rs"));

extern crate alloc;
use alloc::vec::Vec;

mod genesis_config_presets;

#[cfg(test)]
mod integration_tests;

use cumulus_pallet_parachain_system::RelayNumberMonotonicallyIncreases;
use cumulus_primitives_core::AggregateMessageOrigin;
use frame_support::{
    derive_impl,
    dispatch::DispatchClass,
    parameter_types,
    traits::{ConstBool, ConstU32, ConstU64, ConstU8, Everything},
    weights::{constants::WEIGHT_REF_TIME_PER_SECOND, ConstantMultiplier, Weight},
};
use frame_system::EnsureRoot;
use pallet_xcm::XcmPassthrough;
use polkadot_runtime_common::xcm_sender::NoPriceForMessageDelivery;
use sp_api::impl_runtime_apis;
use sp_consensus_aura::sr25519::AuthorityId as AuraId;
use sp_core::{crypto::KeyTypeId, OpaqueMetadata};
use sp_runtime::{
    create_runtime_str, generic, impl_opaque_keys,
    traits::{BlakeTwo256, Block as BlockT, IdentifyAccount, Verify},
    transaction_validity::{TransactionSource, TransactionValidity},
    ApplyExtrinsicResult, MultiSignature,
};
use sp_version::RuntimeVersion;
use xcm::latest::prelude::*;
use xcm_builder::{
    AccountId32Aliases, AllowTopLevelPaidExecutionFrom, EnsureXcmOrigin, FixedWeightBounds,
    FrameTransactionalProcessor, ParentIsPreset, RelayChainAsNative, SiblingParachainAsNative,
    SiblingParachainConvertsVia, SignedAccountId32AsNative, SignedToAccountId32,
    SovereignSignedViaLocation, TakeWeightCredit,
};
use xcm_executor::XcmExecutor;

pub type Signature = MultiSignature;
pub type AccountId = <<Signature as Verify>::Signer as IdentifyAccount>::AccountId;
pub type Balance = u128;
pub type Nonce = u32;
pub type Hash = sp_core::H256;
pub type BlockNumber = u32;
pub type Address = sp_runtime::MultiAddress<AccountId, ()>;
pub type Header = generic::Header<BlockNumber, BlakeTwo256>;
pub type Block = generic::Block<Header, UncheckedExtrinsic>;
pub type SignedBlock = generic::SignedBlock<Block>;

pub type SignedExtra = (
    frame_system::CheckNonZeroSender<Runtime>,
    frame_system::CheckSpecVersion<Runtime>,
    frame_system::CheckTxVersion<Runtime>,
    frame_system::CheckGenesis<Runtime>,
    frame_system::CheckEra<Runtime>,
    frame_system::CheckNonce<Runtime>,
    frame_system::CheckWeight<Runtime>,
    pallet_transaction_payment::ChargeTransactionPayment<Runtime>,
    cumulus_primitives_storage_weight_reclaim::StorageWeightReclaim<Runtime>,
    frame_metadata_hash_extension::CheckMetadataHash<Runtime>,
);

pub type UncheckedExtrinsic =
    generic::UncheckedExtrinsic<Address, RuntimeCall, Signature, SignedExtra>;

pub type Executive = frame_executive::Executive<
    Runtime,
    Block,
    frame_system::ChainContext<Runtime>,
    Runtime,
    AllPalletsWithSystem,
>;

pub mod opaque {
    use super::*;
    pub use sp_runtime::OpaqueExtrinsic as UncheckedExtrinsic;
    pub type Block = generic::Block<Header, UncheckedExtrinsic>;

    impl_opaque_keys! {
        pub struct SessionKeys {
            pub aura: Aura,
        }
    }
}

#[sp_version::runtime_version]
pub const VERSION: RuntimeVersion = RuntimeVersion {
    spec_name: create_runtime_str!("zanchor"),
    impl_name: create_runtime_str!("zanchor"),
    authoring_version: 1,
    spec_version: 100,
    impl_version: 0,
    apis: RUNTIME_API_VERSIONS,
    transaction_version: 1,
    system_version: 1,
};

#[cfg(feature = "std")]
pub fn native_version() -> sp_version::NativeVersion {
    sp_version::NativeVersion { runtime_version: VERSION, can_author_with: Default::default() }
}

const NORMAL_DISPATCH_RATIO: sp_runtime::Perbill = sp_runtime::Perbill::from_percent(75);
const MAXIMUM_BLOCK_WEIGHT: Weight = Weight::from_parts(
    WEIGHT_REF_TIME_PER_SECOND.saturating_mul(2),
    cumulus_primitives_core::relay_chain::MAX_POV_SIZE as u64,
);

pub const EXISTENTIAL_DEPOSIT: Balance = 10_000_000_000;
pub const UNIT: Balance = 1_000_000_000_000;
pub const MILLIUNIT: Balance = UNIT / 1_000;
pub const MICROUNIT: Balance = UNIT / 1_000_000;

parameter_types! {
    pub const BlockHashCount: BlockNumber = 4096;
    pub const Version: RuntimeVersion = VERSION;
    pub RuntimeBlockLength: frame_system::limits::BlockLength =
        frame_system::limits::BlockLength::max_with_normal_ratio(5 * 1024 * 1024, NORMAL_DISPATCH_RATIO);
    pub RuntimeBlockWeights: frame_system::limits::BlockWeights = frame_system::limits::BlockWeights::builder()
        .base_block(Weight::from_parts(390_000_000, 0))
        .for_class(DispatchClass::all(), |weights| {
            weights.base_extrinsic = Weight::from_parts(125_000_000, 0);
        })
        .for_class(DispatchClass::Normal, |weights| {
            weights.max_total = Some(NORMAL_DISPATCH_RATIO * MAXIMUM_BLOCK_WEIGHT);
        })
        .for_class(DispatchClass::Operational, |weights| {
            weights.max_total = Some(MAXIMUM_BLOCK_WEIGHT);
            weights.reserved = Some(
                MAXIMUM_BLOCK_WEIGHT - NORMAL_DISPATCH_RATIO * MAXIMUM_BLOCK_WEIGHT
            );
        })
        .avg_block_initialization(sp_runtime::Perbill::from_percent(5))
        .build_or_panic();
    pub const SS58Prefix: u16 = 42;
}

#[derive_impl(frame_system::config_preludes::ParaChainDefaultConfig)]
impl frame_system::Config for Runtime {
    type BaseCallFilter = Everything;
    type BlockWeights = RuntimeBlockWeights;
    type BlockLength = RuntimeBlockLength;
    type RuntimeOrigin = RuntimeOrigin;
    type RuntimeCall = RuntimeCall;
    type Nonce = Nonce;
    type Hash = Hash;
    type Hashing = BlakeTwo256;
    type AccountId = AccountId;
    type Lookup = sp_runtime::traits::AccountIdLookup<AccountId, ()>;
    type Block = Block;
    type RuntimeEvent = RuntimeEvent;
    type BlockHashCount = BlockHashCount;
    type DbWeight = ();
    type Version = Version;
    type PalletInfo = PalletInfo;
    type AccountData = pallet_balances::AccountData<Balance>;
    type OnNewAccount = ();
    type OnKilledAccount = ();
    type SystemWeightInfo = ();
    type SS58Prefix = SS58Prefix;
    type OnSetCode = cumulus_pallet_parachain_system::ParachainSetCode<Self>;
    type MaxConsumers = ConstU32<16>;
}

impl pallet_timestamp::Config for Runtime {
    type Moment = u64;
    type OnTimestampSet = ();  // Skip Aura's slot assertion check for dev mode
    type MinimumPeriod = ConstU64<3000>;
    type WeightInfo = ();
}

impl pallet_aura::Config for Runtime {
    type AuthorityId = AuraId;
    type DisabledValidators = ();
    type MaxAuthorities = ConstU32<100_000>;
    type AllowMultipleBlocksPerSlot = ConstBool<true>;
    type SlotDuration = ConstU64<6000>;
}

parameter_types! {
    pub const ExistentialDeposit: Balance = EXISTENTIAL_DEPOSIT;
}

impl pallet_balances::Config for Runtime {
    type MaxLocks = ConstU32<50>;
    type Balance = Balance;
    type RuntimeEvent = RuntimeEvent;
    type DustRemoval = ();
    type ExistentialDeposit = ExistentialDeposit;
    type AccountStore = System;
    type WeightInfo = ();
    type MaxReserves = ConstU32<50>;
    type ReserveIdentifier = [u8; 8];
    type RuntimeHoldReason = RuntimeHoldReason;
    type RuntimeFreezeReason = RuntimeFreezeReason;
    type FreezeIdentifier = ();
    type MaxFreezes = ConstU32<0>;
    type DoneSlashHandler = ();
}

parameter_types! {
    pub const TransactionByteFee: Balance = MICROUNIT;
}

impl pallet_transaction_payment::Config for Runtime {
    type RuntimeEvent = RuntimeEvent;
    type OnChargeTransaction = pallet_transaction_payment::FungibleAdapter<Balances, ()>;
    type WeightToFee = ConstantMultiplier<Balance, ConstU128<{ MICROUNIT }>>;
    type LengthToFee = ConstantMultiplier<Balance, TransactionByteFee>;
    type FeeMultiplierUpdate = ();
    type OperationalFeeMultiplier = ConstU8<5>;
    type WeightInfo = ();
}

use sp_runtime::traits::ConstU128;

impl pallet_sudo::Config for Runtime {
    type RuntimeEvent = RuntimeEvent;
    type RuntimeCall = RuntimeCall;
    type WeightInfo = ();
}

parameter_types! {
    pub const ReservedXcmpWeight: Weight = MAXIMUM_BLOCK_WEIGHT.saturating_div(4);
    pub const ReservedDmpWeight: Weight = MAXIMUM_BLOCK_WEIGHT.saturating_div(4);
    pub const RelayOrigin: AggregateMessageOrigin = AggregateMessageOrigin::Parent;
}

impl cumulus_pallet_parachain_system::Config for Runtime {
    type RuntimeEvent = RuntimeEvent;
    type OnSystemEvent = ();
    type SelfParaId = parachain_info::Pallet<Runtime>;
    type OutboundXcmpMessageSource = ();
    type DmpQueue = frame_support::traits::EnqueueWithOrigin<MessageQueue, RelayOrigin>;
    type ReservedDmpWeight = ReservedDmpWeight;
    type XcmpMessageHandler = ();
    type ReservedXcmpWeight = ReservedXcmpWeight;
    type CheckAssociatedRelayNumber = RelayNumberMonotonicallyIncreases;
    type ConsensusHook = cumulus_pallet_aura_ext::FixedVelocityConsensusHook<
        Runtime,
        { 6000 },
        { 1 },
        { 1 },
    >;
    type WeightInfo = ();
    type RelayParentOffset = ConstU32<0>;
}

impl parachain_info::Config for Runtime {}

impl cumulus_pallet_aura_ext::Config for Runtime {}

parameter_types! {
    pub MessageQueueServiceWeight: Weight = Weight::from_parts(35_000_000_000, 1_000_000);
}

impl pallet_message_queue::Config for Runtime {
    type RuntimeEvent = RuntimeEvent;
    type WeightInfo = ();
    type MessageProcessor = xcm_builder::ProcessXcmMessage<
        AggregateMessageOrigin,
        XcmExecutor<XcmConfig>,
        RuntimeCall,
    >;
    type Size = u32;
    type QueueChangeHandler = ();
    type QueuePausedQuery = ();
    type HeapSize = ConstU32<{ 64 * 1024 }>;
    type MaxStale = ConstU32<8>;
    type ServiceWeight = MessageQueueServiceWeight;
    type IdleMaxServiceWeight = ();
}

// XCM configuration
parameter_types! {
    pub const RelayNetwork: Option<NetworkId> = None;
    pub RelayChainOrigin: RuntimeOrigin = cumulus_pallet_xcm::Origin::Relay.into();
    pub UniversalLocation: InteriorLocation = [Parachain(ParachainInfo::parachain_id().into())].into();
}

pub type LocationToAccountId = (
    ParentIsPreset<AccountId>,
    SiblingParachainConvertsVia<polkadot_parachain_primitives::primitives::Sibling, AccountId>,
    AccountId32Aliases<RelayNetwork, AccountId>,
);

pub type XcmOriginToTransactDispatchOrigin = (
    SovereignSignedViaLocation<LocationToAccountId, RuntimeOrigin>,
    RelayChainAsNative<RelayChainOrigin, RuntimeOrigin>,
    SiblingParachainAsNative<cumulus_pallet_xcm::Origin, RuntimeOrigin>,
    SignedAccountId32AsNative<RelayNetwork, RuntimeOrigin>,
    XcmPassthrough<RuntimeOrigin>,
);

parameter_types! {
    pub const MaxInstructions: u32 = 100;
    pub const MaxAssetsIntoHolding: u32 = 64;
    pub const UnitWeightCost: Weight = Weight::from_parts(1_000_000_000, 64 * 1024);
}

pub struct XcmConfig;
impl xcm_executor::Config for XcmConfig {
    type RuntimeCall = RuntimeCall;
    type XcmSender = XcmRouter;
    type AssetTransactor = ();
    type OriginConverter = XcmOriginToTransactDispatchOrigin;
    type IsReserve = ();
    type IsTeleporter = ();
    type UniversalLocation = UniversalLocation;
    type Barrier = (TakeWeightCredit, AllowTopLevelPaidExecutionFrom<Everything>);
    type Weigher = FixedWeightBounds<UnitWeightCost, RuntimeCall, MaxInstructions>;
    type Trader = ();
    type ResponseHandler = PolkadotXcm;
    type AssetTrap = PolkadotXcm;
    type AssetClaims = PolkadotXcm;
    type SubscriptionService = PolkadotXcm;
    type PalletInstancesInfo = AllPalletsWithSystem;
    type MaxAssetsIntoHolding = MaxAssetsIntoHolding;
    type AssetLocker = ();
    type AssetExchanger = ();
    type FeeManager = ();
    type MessageExporter = ();
    type UniversalAliases = ();
    type CallDispatcher = RuntimeCall;
    type SafeCallFilter = Everything;
    type Aliasers = ();
    type TransactionalProcessor = FrameTransactionalProcessor;
    type HrmpNewChannelOpenRequestHandler = ();
    type HrmpChannelAcceptedHandler = ();
    type HrmpChannelClosingHandler = ();
    type XcmRecorder = PolkadotXcm;
    type XcmEventEmitter = PolkadotXcm;
}

pub type LocalOriginToLocation = SignedToAccountId32<RuntimeOrigin, AccountId, RelayNetwork>;

pub type XcmRouter = cumulus_primitives_utility::ParentAsUmp<
    ParachainSystem,
    PolkadotXcm,
    NoPriceForMessageDelivery<()>,
>;

impl pallet_xcm::Config for Runtime {
    type RuntimeEvent = RuntimeEvent;
    type SendXcmOrigin = EnsureXcmOrigin<RuntimeOrigin, LocalOriginToLocation>;
    type XcmRouter = XcmRouter;
    type ExecuteXcmOrigin = EnsureXcmOrigin<RuntimeOrigin, LocalOriginToLocation>;
    type XcmExecuteFilter = Everything;
    type XcmExecutor = XcmExecutor<XcmConfig>;
    type XcmTeleportFilter = Everything;
    type XcmReserveTransferFilter = Everything;
    type Weigher = FixedWeightBounds<UnitWeightCost, RuntimeCall, MaxInstructions>;
    type UniversalLocation = UniversalLocation;
    type RuntimeOrigin = RuntimeOrigin;
    type RuntimeCall = RuntimeCall;
    const VERSION_DISCOVERY_QUEUE_SIZE: u32 = 100;
    type AdvertisedXcmVersion = pallet_xcm::CurrentXcmVersion;
    type Currency = Balances;
    type CurrencyMatcher = ();
    type TrustedLockers = ();
    type SovereignAccountOf = LocationToAccountId;
    type MaxLockers = ConstU32<8>;
    type WeightInfo = pallet_xcm::TestWeightInfo;
    type AdminOrigin = EnsureRoot<AccountId>;
    type MaxRemoteLockConsumers = ConstU32<0>;
    type RemoteLockConsumerIdentifier = ();
    type AuthorizedAliasConsideration = ();
}

impl cumulus_pallet_xcm::Config for Runtime {
    type RuntimeEvent = RuntimeEvent;
    type XcmExecutor = XcmExecutor<XcmConfig>;
}

// Zcash Light Client configuration
parameter_types! {
    pub const MinRelayerStake: Balance = 1000 * UNIT;
    pub const ChallengePeriod: BlockNumber = 100; // ~10 minutes at 6s blocks
    pub const MinAttestationsForFinality: u32 = 1; // MVP: single relayer for testing
    pub const FraudSlashPercent: u8 = 100; // 100% slash for fraud
    pub const ProofReward: Balance = 10 * UNIT; // Reward for submitting valid ligerito proof
}

impl pallet_zcash_light::Config for Runtime {
    type Currency = Balances;
    type MinRelayerStake = MinRelayerStake;
    type ProofReward = ProofReward;
    type ChallengePeriod = ChallengePeriod;
    type MinAttestationsForFinality = MinAttestationsForFinality;
    type FraudSlashPercent = FraudSlashPercent;
    type WeightInfo = ();
}

// OSST Threshold Custody configuration
// uses strict BFT: t = floor(2n/3) + 1
// n=4 → t=3 (tolerates 1), n=7 → t=5 (tolerates 2), n=10 → t=7 (tolerates 3)
parameter_types! {
    pub const OsstMinCustodians: u32 = 4;  // minimum for 1 failure tolerance
    pub const OsstMaxCustodians: u32 = 100;
    pub const OsstThresholdNumerator: u32 = 2;
    pub const OsstThresholdDenominator: u32 = 3;
    pub const OsstReshareTimeout: u32 = 100; // ~10 minutes at 6s blocks
    pub const OsstLivenessValidity: u32 = 1000; // ~100 minutes
    pub const OsstEpochDuration: u32 = 14400; // ~24 hours at 6s blocks
}

impl pallet_osst_threshold::Config for Runtime {
    type MinCustodians = OsstMinCustodians;
    type MaxCustodians = OsstMaxCustodians;
    type ThresholdNumerator = OsstThresholdNumerator;
    type ThresholdDenominator = OsstThresholdDenominator;
    type ReshareTimeout = OsstReshareTimeout;
    type LivenessValidity = OsstLivenessValidity;
    type EpochDuration = OsstEpochDuration;
}

// pallet-assets configuration for wrapped assets (zBTC, zZEC)
parameter_types! {
    pub const AssetDeposit: Balance = 100 * UNIT;
    pub const AssetAccountDeposit: Balance = EXISTENTIAL_DEPOSIT;
    pub const ApprovalDeposit: Balance = EXISTENTIAL_DEPOSIT;
    pub const StringLimit: u32 = 50;
    pub const MetadataDepositBase: Balance = UNIT;
    pub const MetadataDepositPerByte: Balance = MICROUNIT;
}

impl pallet_assets::Config for Runtime {
    type RuntimeEvent = RuntimeEvent;
    type Balance = Balance;
    type AssetId = u32;
    type AssetIdParameter = codec::Compact<u32>;
    type Currency = Balances;
    type CreateOrigin = frame_support::traits::AsEnsureOriginWithArg<frame_system::EnsureSigned<AccountId>>;
    type ForceOrigin = EnsureRoot<AccountId>;
    type AssetDeposit = AssetDeposit;
    type AssetAccountDeposit = AssetAccountDeposit;
    type MetadataDepositBase = MetadataDepositBase;
    type MetadataDepositPerByte = MetadataDepositPerByte;
    type ApprovalDeposit = ApprovalDeposit;
    type StringLimit = StringLimit;
    type Freezer = ();
    type Extra = ();
    type CallbackHandle = ();
    type WeightInfo = ();
    type RemoveItemsLimit = ConstU32<1000>;
    type Holder = ();
    #[cfg(feature = "runtime-benchmarks")]
    type BenchmarkHelper = ();
}

// frost-bridge configuration for threshold signatures
parameter_types! {
    pub const FrostMinSigners: u16 = 3;
    pub const FrostMaxSigners: u16 = 100;
    pub const FrostThreshold: u16 = 2;  // t-of-n threshold
    pub const FrostDkgTimeout: u32 = 100;  // blocks
    pub const FrostSigningTimeout: u32 = 50;  // blocks
    pub const FrostRotationPeriod: u32 = 28800;  // ~48 hours at 6s blocks
    pub const FrostHeartbeatInterval: u32 = 100;  // blocks
    pub const FrostOfflineThreshold: u32 = 300;  // ~30 minutes without heartbeat
    pub const FrostSlashingGracePeriod: u32 = 3;  // 3 consecutive misses before penalty
    pub const FrostMinParticipationRate: u8 = 80;  // 80% minimum participation
    pub const FrostCircuitBreakerThreshold: u32 = 5;  // 5 failures triggers circuit breaker
}

impl pallet_frost_bridge::Config for Runtime {
    type RuntimeEvent = RuntimeEvent;
    type MinSigners = FrostMinSigners;
    type MaxSigners = FrostMaxSigners;
    type Threshold = FrostThreshold;
    type DkgTimeout = FrostDkgTimeout;
    type SigningTimeout = FrostSigningTimeout;
    type RotationPeriod = FrostRotationPeriod;
    type HeartbeatInterval = FrostHeartbeatInterval;
    type OfflineThreshold = FrostOfflineThreshold;
    type SlashingGracePeriod = FrostSlashingGracePeriod;
    type MinParticipationRate = FrostMinParticipationRate;
    type CircuitBreakerThreshold = FrostCircuitBreakerThreshold;
}

// custody pallet configuration for btc/zec deposits and withdrawals
parameter_types! {
    pub const ZbtcAssetId: u32 = 1;  // asset id for wrapped BTC
    pub const ZzecAssetId: u32 = 2;  // asset id for wrapped ZEC
    pub const CustodyMinDepositAmount: Balance = 10_000;  // ~0.0001 BTC in satoshis
    pub const CustodyMinWithdrawalAmount: Balance = 50_000;  // ~0.0005 BTC
    pub const CustodyWithdrawalFeeBps: u32 = 30;  // 0.3% fee
    pub const CustodyDepositExpiry: BlockNumber = 14400;  // ~24 hours
    pub const CustodyMaxWithdrawalsPerCheckpoint: u32 = 256;
    pub const CustodyCheckpointInterval: BlockNumber = 100;  // ~10 minutes
    pub const CustodyRequiredConfirmations: u32 = 6;  // btc confirmations
}

impl pallet_custody::Config for Runtime {
    type RuntimeEvent = RuntimeEvent;
    type AssetId = u32;
    type Balance = Balance;
    type Assets = Assets;
    type FrostBridge = FrostBridge;
    type ZbtcAssetId = ZbtcAssetId;
    type ZzecAssetId = ZzecAssetId;
    type MinDepositAmount = CustodyMinDepositAmount;
    type MinWithdrawalAmount = CustodyMinWithdrawalAmount;
    type WithdrawalFeeBps = CustodyWithdrawalFeeBps;
    type DepositExpiry = CustodyDepositExpiry;
    type MaxWithdrawalsPerCheckpoint = CustodyMaxWithdrawalsPerCheckpoint;
    type CheckpointInterval = CustodyCheckpointInterval;
    type RequiredConfirmations = CustodyRequiredConfirmations;
}

// shielded pool config
parameter_types! {
    pub const ShieldedMinShieldAmount: u64 = 10_000;  // 0.0001 btc dust limit
    pub const ShieldedRootHistorySize: u32 = 256;  // keep ~256 historical roots valid
}

impl pallet_shielded_pool::Config for Runtime {
    type RuntimeEvent = RuntimeEvent;
    type AssetId = u32;
    type Balance = Balance;
    type Assets = Assets;
    type ZbtcAssetId = ZbtcAssetId;
    type ZzecAssetId = ZzecAssetId;
    type MinShieldAmount = ShieldedMinShieldAmount;
    type RootHistorySize = ShieldedRootHistorySize;
}

// Escrow arbitration config (shielded P2P trading)
parameter_types! {
    pub const EscrowMinAgentStake: Balance = 100 * UNIT;
    pub const EscrowMinArbitratorStake: Balance = 500 * UNIT;
    pub const EscrowDefaultFundingDeadline: u32 = 100;  // ~10 min at 6s blocks
    pub const EscrowDefaultPaymentTimeout: u32 = 1000;  // ~1.6 hours
    pub const EscrowDisputeVotingPeriod: u32 = 500;     // ~50 min
    pub const EscrowMinArbitratorsForDispute: u32 = 3;
    pub const EscrowMinAgentsPerEscrow: u32 = 1;
    pub const EscrowSigningTimeout: u32 = 100;          // ~10 min for FROST signing
    pub const EscrowMinChainServiceDeposit: Balance = UNIT;
}

impl pallet_escrow_arbitration::Config for Runtime {
    type Currency = Balances;
    type MinAgentStake = EscrowMinAgentStake;
    type MinArbitratorStake = EscrowMinArbitratorStake;
    type DefaultFundingDeadline = EscrowDefaultFundingDeadline;
    type DefaultPaymentTimeout = EscrowDefaultPaymentTimeout;
    type DisputeVotingPeriod = EscrowDisputeVotingPeriod;
    type MinArbitratorsForDispute = EscrowMinArbitratorsForDispute;
    type MinAgentsPerEscrow = EscrowMinAgentsPerEscrow;
    type SigningTimeout = EscrowSigningTimeout;
    type MinChainServiceDeposit = EscrowMinChainServiceDeposit;
    type WeightInfo = pallet_escrow_arbitration::weights::SubstrateWeight<Runtime>;
}

// Construct runtime
frame_support::construct_runtime!(
    pub enum Runtime {
        // System
        System: frame_system = 0,
        Timestamp: pallet_timestamp = 1,

        // Parachain
        ParachainSystem: cumulus_pallet_parachain_system = 2,
        ParachainInfo: parachain_info = 3,

        // Monetary
        Balances: pallet_balances = 10,
        TransactionPayment: pallet_transaction_payment = 11,

        // Governance
        Sudo: pallet_sudo = 15,

        // Consensus
        Aura: pallet_aura = 23,
        AuraExt: cumulus_pallet_aura_ext = 24,

        // XCM
        PolkadotXcm: pallet_xcm = 30,
        CumulusXcm: cumulus_pallet_xcm = 31,
        MessageQueue: pallet_message_queue = 32,

        // Zcash Light Client (the main event!)
        ZcashLight: pallet_zcash_light = 50,

        // OSST Threshold Custody for zZEC
        OsstThreshold: pallet_osst_threshold = 51,

        // Wrapped assets (zBTC, zZEC)
        Assets: pallet_assets = 52,

        // FROST threshold signature bridge
        FrostBridge: pallet_frost_bridge = 53,

        // BTC/ZEC custody and deposit/withdrawal management
        Custody: pallet_custody = 54,

        // privacy-preserving shielded pool with ligerito proofs
        ShieldedPool: pallet_shielded_pool = 55,

        // P2P escrow arbitration (shielded LocalCryptos)
        EscrowArbitration: pallet_escrow_arbitration = 56,
    }
);

impl_runtime_apis! {
    impl sp_api::Core<Block> for Runtime {
        fn version() -> RuntimeVersion {
            VERSION
        }

        fn execute_block(block: Block) {
            Executive::execute_block(block)
        }

        fn initialize_block(header: &<Block as BlockT>::Header) -> sp_runtime::ExtrinsicInclusionMode {
            Executive::initialize_block(header)
        }
    }

    impl sp_api::Metadata<Block> for Runtime {
        fn metadata() -> OpaqueMetadata {
            OpaqueMetadata::new(Runtime::metadata().into())
        }

        fn metadata_at_version(version: u32) -> Option<OpaqueMetadata> {
            Runtime::metadata_at_version(version)
        }

        fn metadata_versions() -> Vec<u32> {
            Runtime::metadata_versions()
        }
    }

    impl sp_block_builder::BlockBuilder<Block> for Runtime {
        fn apply_extrinsic(extrinsic: <Block as BlockT>::Extrinsic) -> ApplyExtrinsicResult {
            Executive::apply_extrinsic(extrinsic)
        }

        fn finalize_block() -> <Block as BlockT>::Header {
            Executive::finalize_block()
        }

        fn inherent_extrinsics(data: sp_inherents::InherentData) -> Vec<<Block as BlockT>::Extrinsic> {
            data.create_extrinsics()
        }

        fn check_inherents(
            block: Block,
            data: sp_inherents::InherentData,
        ) -> sp_inherents::CheckInherentsResult {
            data.check_extrinsics(&block)
        }
    }

    impl sp_transaction_pool::runtime_api::TaggedTransactionQueue<Block> for Runtime {
        fn validate_transaction(
            source: TransactionSource,
            tx: <Block as BlockT>::Extrinsic,
            block_hash: <Block as BlockT>::Hash,
        ) -> TransactionValidity {
            Executive::validate_transaction(source, tx, block_hash)
        }
    }

    impl sp_offchain::OffchainWorkerApi<Block> for Runtime {
        fn offchain_worker(header: &<Block as BlockT>::Header) {
            Executive::offchain_worker(header)
        }
    }

    impl sp_consensus_aura::AuraApi<Block, AuraId> for Runtime {
        fn slot_duration() -> sp_consensus_aura::SlotDuration {
            sp_consensus_aura::SlotDuration::from_millis(6000)
        }

        fn authorities() -> Vec<AuraId> {
            pallet_aura::Authorities::<Runtime>::get().into_inner()
        }
    }

    impl sp_session::SessionKeys<Block> for Runtime {
        fn generate_session_keys(seed: Option<Vec<u8>>) -> Vec<u8> {
            opaque::SessionKeys::generate(seed)
        }

        fn decode_session_keys(
            encoded: Vec<u8>,
        ) -> Option<Vec<(Vec<u8>, KeyTypeId)>> {
            opaque::SessionKeys::decode_into_raw_public_keys(&encoded)
        }
    }

    impl cumulus_primitives_core::CollectCollationInfo<Block> for Runtime {
        fn collect_collation_info(header: &<Block as BlockT>::Header) -> cumulus_primitives_core::CollationInfo {
            ParachainSystem::collect_collation_info(header)
        }
    }

    impl frame_system_rpc_runtime_api::AccountNonceApi<Block, AccountId, Nonce> for Runtime {
        fn account_nonce(account: AccountId) -> Nonce {
            System::account_nonce(account)
        }
    }

    impl cumulus_primitives_aura::AuraUnincludedSegmentApi<Block> for Runtime {
        fn can_build_upon(
            _included_hash: <Block as BlockT>::Hash,
            _slot: cumulus_primitives_aura::Slot,
        ) -> bool {
            true
        }
    }

    impl sp_genesis_builder::GenesisBuilder<Block> for Runtime {
        fn build_state(config: Vec<u8>) -> sp_genesis_builder::Result {
            frame_support::genesis_builder_helper::build_state::<RuntimeGenesisConfig>(config)
        }

        fn get_preset(id: &Option<sp_genesis_builder::PresetId>) -> Option<Vec<u8>> {
            frame_support::genesis_builder_helper::get_preset::<RuntimeGenesisConfig>(
                id,
                crate::genesis_config_presets::get_preset
            )
        }

        fn preset_names() -> Vec<sp_genesis_builder::PresetId> {
            crate::genesis_config_presets::preset_names()
        }
    }

    impl pallet_transaction_payment_rpc_runtime_api::TransactionPaymentApi<Block, Balance> for Runtime {
        fn query_info(
            uxt: <Block as BlockT>::Extrinsic,
            len: u32,
        ) -> pallet_transaction_payment_rpc_runtime_api::RuntimeDispatchInfo<Balance> {
            TransactionPayment::query_info(uxt, len)
        }

        fn query_fee_details(
            uxt: <Block as BlockT>::Extrinsic,
            len: u32,
        ) -> pallet_transaction_payment::FeeDetails<Balance> {
            TransactionPayment::query_fee_details(uxt, len)
        }

        fn query_weight_to_fee(weight: Weight) -> Balance {
            TransactionPayment::weight_to_fee(weight)
        }

        fn query_length_to_fee(length: u32) -> Balance {
            TransactionPayment::length_to_fee(length)
        }
    }
}

cumulus_pallet_parachain_system::register_validate_block! {
    Runtime = Runtime,
    BlockExecutor = cumulus_pallet_aura_ext::BlockExecutor::<Runtime, Executive>,
}
