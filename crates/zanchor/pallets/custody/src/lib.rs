//! custody pallet
//!
//! manages btc/zec deposits and withdrawals for zanchor bridge.
//! works with frost-bridge for threshold signatures.
//!
//! ## design
//!
//! 1. **deposits**: user sends btc/zec to custody address, proves via spv, gets minted zbtc/zzec
//! 2. **withdrawals**: user burns zbtc/zzec, batched into checkpoints, frost-signed, broadcast
//! 3. **checkpoints**: periodic batching of pending withdrawals into single bitcoin tx
//!
//! unlike interlay's vault model, this uses collective frost custody with mandatory participation.

#![cfg_attr(not(feature = "std"), no_std)]

pub use pallet::*;

use codec::{Decode, Encode, MaxEncodedLen};
use frame_support::pallet_prelude::BoundedVec;
use frame_support::traits::fungibles::{Create, Inspect, Mutate};
use frame_support::traits::Get;
use scale_info::TypeInfo;
use sp_core::H256;
use sp_std::prelude::*;

/// max withdrawals per checkpoint
pub type MaxWithdrawals = frame_support::traits::ConstU32<256>;
/// max btc tx size
pub type MaxBtcTxSize = frame_support::traits::ConstU32<65536>;
/// max reason length
pub type MaxReasonLen = frame_support::traits::ConstU32<128>;

// re-export btc address from frost-bridge
pub use pallet_frost_bridge::{BtcAddress, BtcAddressType};

/// deposit request
#[derive(Clone, Encode, Decode, TypeInfo, MaxEncodedLen, Debug, PartialEq)]
pub struct DepositRequest<AccountId, Balance, BlockNumber> {
    /// unique deposit id
    pub id: H256,
    /// who is depositing
    pub depositor: AccountId,
    /// expected amount (0 if unknown)
    pub expected_amount: Balance,
    /// confirmed btc txid
    pub btc_txid: Option<H256>,
    /// actual amount received
    pub confirmed_amount: Balance,
    /// block when request created
    pub created_at: BlockNumber,
    /// block when btc confirmed
    pub confirmed_at: Option<BlockNumber>,
    /// block when zbtc minted
    pub minted_at: Option<BlockNumber>,
    /// current status
    pub status: DepositStatus,
}

#[derive(Clone, Encode, Decode, TypeInfo, MaxEncodedLen, Debug, PartialEq, Default)]
pub enum DepositStatus {
    /// waiting for btc deposit
    #[default]
    Pending,
    /// btc deposit confirmed via spv
    Confirmed,
    /// zbtc minted to depositor
    Minted,
    /// request expired
    Expired,
    /// cancelled by user
    Cancelled,
}

/// withdrawal request
#[derive(Clone, Encode, Decode, TypeInfo, MaxEncodedLen, Debug, PartialEq)]
pub struct WithdrawalRequest<AccountId, Balance, BlockNumber> {
    /// unique withdrawal id
    pub id: H256,
    /// who is withdrawing
    pub requester: AccountId,
    /// destination btc address
    pub btc_dest: BtcAddress,
    /// amount to withdraw (after fee)
    pub amount: Balance,
    /// fee paid
    pub fee: Balance,
    /// block when request created
    pub created_at: BlockNumber,
    /// checkpoint this withdrawal is in (if any)
    pub checkpoint_id: Option<u64>,
    /// btc txid once broadcast
    pub btc_txid: Option<H256>,
    /// current status
    pub status: WithdrawalStatus,
}

#[derive(Clone, Encode, Decode, TypeInfo, MaxEncodedLen, Debug, PartialEq, Default)]
pub enum WithdrawalStatus {
    /// waiting to be included in checkpoint
    #[default]
    Pending,
    /// included in checkpoint, waiting for signature
    InCheckpoint,
    /// checkpoint signed, waiting for broadcast
    Signed,
    /// btc tx broadcast
    Broadcast,
    /// btc tx confirmed
    Completed,
    /// cancelled/refunded
    Cancelled,
}

/// checkpoint batching multiple withdrawals
#[derive(Clone, Encode, Decode, TypeInfo, MaxEncodedLen, Debug, PartialEq)]
#[scale_info(skip_type_params(W, T))]
pub struct Checkpoint<BlockNumber, W: Get<u32>, T: Get<u32>> {
    /// unique checkpoint id
    pub id: u64,
    /// withdrawals included
    pub withdrawal_ids: BoundedVec<H256, W>,
    /// total amount being withdrawn
    pub total_amount: u128,
    /// unsigned bitcoin tx
    pub btc_tx_unsigned: BoundedVec<u8, T>,
    /// signed bitcoin tx (after frost signing)
    pub btc_tx_signed: Option<BoundedVec<u8, T>>,
    /// frost signing request id
    pub signing_request_id: Option<u64>,
    /// block when checkpoint created
    pub created_at: BlockNumber,
    /// block when signed
    pub signed_at: Option<BlockNumber>,
    /// block when broadcast confirmed
    pub broadcast_at: Option<BlockNumber>,
    /// btc block when confirmed
    pub btc_confirmed_at: Option<u32>,
    /// status
    pub status: CheckpointStatus,
}

#[derive(Clone, Encode, Decode, TypeInfo, MaxEncodedLen, Debug, PartialEq, Default)]
pub enum CheckpointStatus {
    /// being constructed
    #[default]
    Building,
    /// waiting for frost signatures
    PendingSignature,
    /// signed, ready for broadcast
    Signed,
    /// broadcast to bitcoin network
    Broadcast,
    /// confirmed on bitcoin
    Confirmed,
    /// failed (will retry)
    Failed,
}

#[frame_support::pallet]
pub mod pallet {
    use super::*;
    use frame_support::pallet_prelude::*;
    use frame_system::pallet_prelude::*;
    use pallet_frost_bridge::FrostBridgeInterface;
    use sp_runtime::Saturating;

    #[pallet::pallet]
    pub struct Pallet<T>(_);

    #[pallet::config]
    pub trait Config: frame_system::Config {
        /// runtime event type
        type RuntimeEvent: From<Event<Self>> + IsType<<Self as frame_system::Config>::RuntimeEvent>;

        /// asset id type (u32 for pallet-assets compatibility)
        type AssetId: Member + Parameter + MaxEncodedLen + Copy + Default + Ord;

        /// balance type
        type Balance: Member
            + Parameter
            + MaxEncodedLen
            + Copy
            + Default
            + sp_runtime::traits::Zero
            + sp_runtime::traits::Saturating
            + sp_runtime::traits::AtLeast32BitUnsigned
            + From<u128>
            + Into<u128>;

        /// fungible assets pallet (pallet-assets)
        type Assets: Inspect<Self::AccountId, AssetId = Self::AssetId, Balance = Self::Balance>
            + Mutate<Self::AccountId>
            + Create<Self::AccountId>;

        /// frost bridge pallet for signing
        type FrostBridge: pallet_frost_bridge::FrostBridgeInterface<Self::AccountId>;

        /// asset id for wrapped btc (zbtc)
        #[pallet::constant]
        type ZbtcAssetId: Get<Self::AssetId>;

        /// asset id for wrapped zec (zzec)
        #[pallet::constant]
        type ZzecAssetId: Get<Self::AssetId>;

        /// minimum deposit amount (dust threshold)
        #[pallet::constant]
        type MinDepositAmount: Get<Self::Balance>;

        /// minimum withdrawal amount
        #[pallet::constant]
        type MinWithdrawalAmount: Get<Self::Balance>;

        /// withdrawal fee rate (basis points, e.g., 30 = 0.3%)
        #[pallet::constant]
        type WithdrawalFeeBps: Get<u32>;

        /// blocks before deposit request expires
        #[pallet::constant]
        type DepositExpiry: Get<BlockNumberFor<Self>>;

        /// max withdrawals per checkpoint
        #[pallet::constant]
        type MaxWithdrawalsPerCheckpoint: Get<u32>;

        /// blocks between checkpoint creation
        #[pallet::constant]
        type CheckpointInterval: Get<BlockNumberFor<Self>>;

        /// btc confirmations required for deposit
        #[pallet::constant]
        type RequiredConfirmations: Get<u32>;
    }

    // ============ storage ============

    /// deposit requests by id
    #[pallet::storage]
    pub type DepositRequests<T: Config> = StorageMap<
        _,
        Blake2_128Concat,
        H256,
        DepositRequest<T::AccountId, T::Balance, BlockNumberFor<T>>,
    >;

    /// withdrawal requests by id
    #[pallet::storage]
    pub type WithdrawalRequests<T: Config> = StorageMap<
        _,
        Blake2_128Concat,
        H256,
        WithdrawalRequest<T::AccountId, T::Balance, BlockNumberFor<T>>,
    >;

    /// pending withdrawal ids (not yet in checkpoint)
    #[pallet::storage]
    pub type PendingWithdrawals<T: Config> = StorageValue<_, BoundedVec<H256, MaxWithdrawals>, ValueQuery>;

    /// checkpoints by id
    #[pallet::storage]
    pub type Checkpoints<T: Config> = StorageMap<
        _,
        Blake2_128Concat,
        u64,
        Checkpoint<BlockNumberFor<T>, MaxWithdrawals, MaxBtcTxSize>,
    >;

    /// next checkpoint id
    #[pallet::storage]
    pub type NextCheckpointId<T: Config> = StorageValue<_, u64, ValueQuery>;

    /// last checkpoint block
    #[pallet::storage]
    pub type LastCheckpointBlock<T: Config> = StorageValue<_, BlockNumberFor<T>, ValueQuery>;

    /// total zbtc supply (for sanity checks)
    #[pallet::storage]
    pub type TotalSupply<T: Config> = StorageValue<_, T::Balance, ValueQuery>;

    /// total btc in custody (tracked)
    #[pallet::storage]
    pub type TotalCustody<T: Config> = StorageValue<_, T::Balance, ValueQuery>;

    /// nonce for generating unique ids
    #[pallet::storage]
    pub type Nonce<T: Config> = StorageValue<_, u64, ValueQuery>;

    // ============ events ============

    #[pallet::event]
    #[pallet::generate_deposit(pub(super) fn deposit_event)]
    pub enum Event<T: Config> {
        /// deposit requested
        DepositRequested {
            deposit_id: H256,
            depositor: T::AccountId,
            custody_address: BtcAddress,
        },

        /// btc deposit confirmed via spv
        DepositConfirmed {
            deposit_id: H256,
            btc_txid: H256,
            amount: T::Balance,
        },

        /// zbtc minted to depositor
        ZbtcMinted {
            deposit_id: H256,
            depositor: T::AccountId,
            amount: T::Balance,
        },

        /// deposit cancelled/expired
        DepositCancelled {
            deposit_id: H256,
            reason: BoundedVec<u8, MaxReasonLen>,
        },

        /// withdrawal requested
        WithdrawalRequested {
            withdrawal_id: H256,
            requester: T::AccountId,
            btc_dest: BtcAddress,
            amount: T::Balance,
            fee: T::Balance,
        },

        /// withdrawal added to checkpoint
        WithdrawalQueued {
            withdrawal_id: H256,
            checkpoint_id: u64,
        },

        /// withdrawal completed
        WithdrawalCompleted {
            withdrawal_id: H256,
            btc_txid: H256,
        },

        /// checkpoint created
        CheckpointCreated {
            checkpoint_id: u64,
            withdrawal_count: u32,
            total_amount: u128,
        },

        /// checkpoint signed by frost
        CheckpointSigned {
            checkpoint_id: u64,
            signing_request_id: u64,
        },

        /// checkpoint broadcast to bitcoin
        CheckpointBroadcast {
            checkpoint_id: u64,
            btc_txid: H256,
        },

        /// checkpoint confirmed on bitcoin
        CheckpointConfirmed {
            checkpoint_id: u64,
            btc_block: u32,
        },
    }

    // ============ errors ============

    #[pallet::error]
    pub enum Error<T> {
        /// deposit not found
        DepositNotFound,
        /// withdrawal not found
        WithdrawalNotFound,
        /// checkpoint not found
        CheckpointNotFound,
        /// invalid deposit status
        InvalidDepositStatus,
        /// invalid withdrawal status
        InvalidWithdrawalStatus,
        /// amount below minimum
        AmountBelowMinimum,
        /// insufficient balance
        InsufficientBalance,
        /// invalid btc address
        InvalidBtcAddress,
        /// btc verification failed
        BtcVerificationFailed,
        /// insufficient confirmations
        InsufficientConfirmations,
        /// bridge not active
        BridgeNotActive,
        /// no custody address (dkg not complete)
        NoCustodyAddress,
        /// arithmetic overflow
        Overflow,
        /// deposit expired
        DepositExpired,
        /// not authorized
        NotAuthorized,
    }

    // ============ calls ============

    #[pallet::call]
    impl<T: Config> Pallet<T> {
        /// request a deposit - returns custody address to send btc to
        #[pallet::call_index(0)]
        #[pallet::weight(10_000)]
        pub fn request_deposit(
            origin: OriginFor<T>,
            expected_amount: T::Balance,
        ) -> DispatchResult {
            let depositor = ensure_signed(origin)?;

            // check bridge is active
            ensure!(
                T::FrostBridge::is_bridge_active(),
                Error::<T>::BridgeNotActive
            );

            // get custody address
            let custody_address = T::FrostBridge::custody_address()
                .ok_or(Error::<T>::NoCustodyAddress)?;

            // generate unique id
            let deposit_id = Self::generate_id(&depositor);
            let now = frame_system::Pallet::<T>::block_number();

            let request = DepositRequest {
                id: deposit_id,
                depositor: depositor.clone(),
                expected_amount,
                btc_txid: None,
                confirmed_amount: T::Balance::default(),
                created_at: now,
                confirmed_at: None,
                minted_at: None,
                status: DepositStatus::Pending,
            };

            DepositRequests::<T>::insert(deposit_id, request);

            Self::deposit_event(Event::DepositRequested {
                deposit_id,
                depositor,
                custody_address,
            });

            Ok(())
        }

        /// confirm deposit with spv proof (anyone can call)
        #[pallet::call_index(1)]
        #[pallet::weight(50_000)]
        pub fn confirm_deposit(
            origin: OriginFor<T>,
            deposit_id: H256,
            btc_txid: H256,
            amount: T::Balance,
            // merkle_proof: Vec<u8>,  // todo: actual spv proof
        ) -> DispatchResult {
            let _ = ensure_signed(origin)?;

            let mut request = DepositRequests::<T>::get(deposit_id)
                .ok_or(Error::<T>::DepositNotFound)?;

            ensure!(
                request.status == DepositStatus::Pending,
                Error::<T>::InvalidDepositStatus
            );

            // check not expired
            let now = frame_system::Pallet::<T>::block_number();
            let expiry = request.created_at.saturating_add(T::DepositExpiry::get());
            ensure!(now <= expiry, Error::<T>::DepositExpired);

            // todo: verify spv proof via btc-relay
            // T::BtcRelay::verify_transaction_inclusion(btc_txid, merkle_proof)?;

            // check amount
            ensure!(
                amount >= T::MinDepositAmount::get(),
                Error::<T>::AmountBelowMinimum
            );

            request.btc_txid = Some(btc_txid);
            request.confirmed_amount = amount;
            request.confirmed_at = Some(now);
            request.status = DepositStatus::Confirmed;

            DepositRequests::<T>::insert(deposit_id, request);

            Self::deposit_event(Event::DepositConfirmed {
                deposit_id,
                btc_txid,
                amount,
            });

            Ok(())
        }

        /// mint zbtc after sufficient confirmations (anyone can call)
        #[pallet::call_index(2)]
        #[pallet::weight(20_000)]
        pub fn mint_zbtc(origin: OriginFor<T>, deposit_id: H256) -> DispatchResult {
            let _ = ensure_signed(origin)?;

            let mut request = DepositRequests::<T>::get(deposit_id)
                .ok_or(Error::<T>::DepositNotFound)?;

            ensure!(
                request.status == DepositStatus::Confirmed,
                Error::<T>::InvalidDepositStatus
            );

            // todo: check btc has enough confirmations via btc-relay
            // let btc_txid = request.btc_txid.ok_or(Error::<T>::BtcVerificationFailed)?;
            // ensure!(
            //     T::BtcRelay::get_confirmations(btc_txid)? >= T::RequiredConfirmations::get(),
            //     Error::<T>::InsufficientConfirmations
            // );

            let now = frame_system::Pallet::<T>::block_number();
            let amount = request.confirmed_amount;

            // mint zbtc tokens to depositor via pallet-assets
            T::Assets::mint_into(
                T::ZbtcAssetId::get(),
                &request.depositor,
                amount,
            )?;

            // update tracking
            TotalSupply::<T>::mutate(|s| *s = s.saturating_add(amount));
            TotalCustody::<T>::mutate(|c| *c = c.saturating_add(amount));

            request.minted_at = Some(now);
            request.status = DepositStatus::Minted;
            DepositRequests::<T>::insert(deposit_id, request.clone());

            Self::deposit_event(Event::ZbtcMinted {
                deposit_id,
                depositor: request.depositor,
                amount,
            });

            Ok(())
        }

        /// request withdrawal - burns zbtc immediately
        #[pallet::call_index(3)]
        #[pallet::weight(30_000)]
        pub fn request_withdrawal(
            origin: OriginFor<T>,
            btc_dest: BtcAddress,
            amount: T::Balance,
        ) -> DispatchResult {
            let requester = ensure_signed(origin)?;

            // check bridge is active
            ensure!(
                T::FrostBridge::is_bridge_active(),
                Error::<T>::BridgeNotActive
            );

            // validate address
            ensure!(!btc_dest.is_zero(), Error::<T>::InvalidBtcAddress);

            // calculate fee
            let fee_bps = T::WithdrawalFeeBps::get();
            let fee = amount.saturating_mul(fee_bps.into()) / 10_000u32.into();
            let net_amount = amount.saturating_sub(fee);

            // check minimum
            ensure!(
                net_amount >= T::MinWithdrawalAmount::get(),
                Error::<T>::AmountBelowMinimum
            );

            // check balance
            let balance = T::Assets::balance(T::ZbtcAssetId::get(), &requester);
            ensure!(balance >= amount, Error::<T>::InsufficientBalance);

            // burn zbtc tokens (full amount including fee)
            T::Assets::burn_from(
                T::ZbtcAssetId::get(),
                &requester,
                amount,
                frame_support::traits::tokens::Preservation::Expendable,
                frame_support::traits::tokens::Precision::Exact,
                frame_support::traits::tokens::Fortitude::Polite,
            )?;

            let withdrawal_id = Self::generate_id(&requester);
            let now = frame_system::Pallet::<T>::block_number();

            let request = WithdrawalRequest {
                id: withdrawal_id,
                requester: requester.clone(),
                btc_dest: btc_dest.clone(),
                amount: net_amount,
                fee,
                created_at: now,
                checkpoint_id: None,
                btc_txid: None,
                status: WithdrawalStatus::Pending,
            };

            WithdrawalRequests::<T>::insert(withdrawal_id, request);
            PendingWithdrawals::<T>::mutate(|v| {
                let _ = v.try_push(withdrawal_id);
            });

            // update tracking
            TotalSupply::<T>::mutate(|s| *s = s.saturating_sub(amount));

            // fee is burned with the rest - goes to protocol treasury via custody pool
            // the net_amount will be sent to btc_dest when checkpoint is processed

            Self::deposit_event(Event::WithdrawalRequested {
                withdrawal_id,
                requester,
                btc_dest,
                amount: net_amount,
                fee,
            });

            Ok(())
        }

        /// manually trigger checkpoint creation (privileged)
        #[pallet::call_index(4)]
        #[pallet::weight(100_000)]
        pub fn force_checkpoint(origin: OriginFor<T>) -> DispatchResult {
            ensure_root(origin)?;
            Self::create_checkpoint()
        }
    }

    // ============ hooks ============

    #[pallet::hooks]
    impl<T: Config> Hooks<BlockNumberFor<T>> for Pallet<T> {
        fn on_finalize(n: BlockNumberFor<T>) {
            // check if time for checkpoint
            let last = LastCheckpointBlock::<T>::get();
            let pending = PendingWithdrawals::<T>::get();

            let should_checkpoint =
                !pending.is_empty() &&
                (pending.len() >= T::MaxWithdrawalsPerCheckpoint::get() as usize ||
                 n.saturating_sub(last) >= T::CheckpointInterval::get());

            if should_checkpoint {
                let _ = Self::create_checkpoint();
            }
        }
    }

    // ============ internal functions ============

    impl<T: Config> Pallet<T> {
        /// generate unique id
        fn generate_id(account: &T::AccountId) -> H256 {
            let nonce = Nonce::<T>::mutate(|n| {
                *n = n.wrapping_add(1);
                *n
            });

            let parent_hash = frame_system::Pallet::<T>::parent_hash();

            // hash account + nonce + parent_hash
            let mut data = account.encode();
            data.extend_from_slice(&nonce.to_le_bytes());
            data.extend_from_slice(parent_hash.as_ref());

            sp_io::hashing::sha2_256(&data).into()
        }

        /// create checkpoint from pending withdrawals
        fn create_checkpoint() -> DispatchResult {
            let pending = PendingWithdrawals::<T>::take();
            if pending.is_empty() {
                return Ok(());
            }

            let checkpoint_id = NextCheckpointId::<T>::mutate(|id| {
                *id = id.wrapping_add(1);
                *id
            });

            let now = frame_system::Pallet::<T>::block_number();
            let mut total_amount: u128 = 0;
            let mut outputs: Vec<(BtcAddress, u128)> = Vec::new();

            // collect withdrawal info
            for withdrawal_id in pending.iter() {
                if let Some(mut request) = WithdrawalRequests::<T>::get(withdrawal_id) {
                    let amount_u128: u128 = request.amount.try_into().unwrap_or(0);
                    outputs.push((request.btc_dest.clone(), amount_u128));
                    total_amount = total_amount.saturating_add(amount_u128);

                    request.checkpoint_id = Some(checkpoint_id);
                    request.status = WithdrawalStatus::InCheckpoint;
                    WithdrawalRequests::<T>::insert(withdrawal_id, request);
                }
            }

            // todo: construct actual bitcoin tx
            let btc_tx_unsigned = Self::construct_btc_tx(&outputs);

            let checkpoint = Checkpoint {
                id: checkpoint_id,
                withdrawal_ids: pending.clone(),
                total_amount,
                btc_tx_unsigned: btc_tx_unsigned.clone(),
                btc_tx_signed: None,
                signing_request_id: None,
                created_at: now,
                signed_at: None,
                broadcast_at: None,
                btc_confirmed_at: None,
                status: CheckpointStatus::PendingSignature,
            };

            Checkpoints::<T>::insert(checkpoint_id, checkpoint);
            LastCheckpointBlock::<T>::put(now);

            // request frost signing
            // todo: integrate with frost-bridge
            // let signing_request_id = T::FrostBridge::request_signature(btc_tx_unsigned)?;

            Self::deposit_event(Event::CheckpointCreated {
                checkpoint_id,
                withdrawal_count: pending.len() as u32,
                total_amount,
            });

            Ok(())
        }

        /// construct bitcoin tx from outputs
        fn construct_btc_tx(outputs: &[(BtcAddress, u128)]) -> BoundedVec<u8, MaxBtcTxSize> {
            // todo: actual bitcoin tx construction
            // this would:
            // 1. get current utxo from frost custody
            // 2. add outputs for each withdrawal
            // 3. add change output back to custody
            // 4. calculate fee
            // 5. serialize unsigned tx

            // placeholder
            let mut tx: Vec<u8> = Vec::new();
            tx.extend_from_slice(b"UNSIGNED_BTC_TX:");
            for (addr, amount) in outputs {
                tx.extend_from_slice(&addr.hash);
                tx.extend_from_slice(&amount.to_le_bytes());
            }
            BoundedVec::try_from(tx).unwrap_or_default()
        }
    }
}
