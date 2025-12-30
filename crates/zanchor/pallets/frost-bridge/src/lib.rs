//! FROST Bridge Pallet
//!
//! Threshold signature bridge using offchain workers for ZEC custody.
//! Relayers participate in FROST signing ceremonies coordinated on-chain.
//!
//! ## Design Principles (à la Gavin Wood)
//!
//! 1. **Offchain computation, on-chain coordination**
//!    Heavy crypto (DKG, signing) happens in OCW, chain just tracks state
//!
//! 2. **Validators ARE the signers**
//!    No external MPC network - relayers who stake become signers
//!
//! 3. **Economic security through slashing**
//!    Misbehavior is provable and punishable
//!
//! 4. **Emergent trust from coordination**
//!    The bridge exists as a property of validator agreement
//!
//! ## Architecture
//!
//! ```text
//! Registration → DKG Ceremony → Active Signing → Key Rotation
//!     ↓              ↓              ↓              ↓
//!   Stake         Generate      Process        Reshare
//!   Bond          Shares        Requests       Keys
//! ```

#![cfg_attr(not(feature = "std"), no_std)]

pub use pallet::*;

#[cfg(test)]
mod tests;

#[cfg(feature = "std")]
use sp_core::offchain::StorageKind;

use codec::{Decode, DecodeWithMemTracking, Encode, MaxEncodedLen};
use frame_support::{
    pallet_prelude::BoundedVec,
    traits::Get,
    Parameter,
};
use scale_info::TypeInfo;
use sp_runtime::SaturatedConversion;
use sp_std::prelude::*;

/// FROST signature on Pallas curve (Zcash Orchard)
#[derive(Clone, Encode, Decode, DecodeWithMemTracking, TypeInfo, MaxEncodedLen, Debug, PartialEq)]
pub struct FrostSignature {
    /// R component (commitment)
    pub r: [u8; 32],
    /// S component (response)
    pub s: [u8; 32],
}

/// Public key share for a signer
#[derive(Clone, Encode, Decode, TypeInfo, MaxEncodedLen, Debug, PartialEq)]
pub struct PublicKeyShare {
    /// Signer index (1-indexed per FROST spec)
    pub index: u16,
    /// Public key share on Pallas curve
    pub share: [u8; 32],
    /// Proof of secret key knowledge
    pub proof: [u8; 64],
}

/// Encrypted key share for DKG distribution
#[derive(Clone, Encode, Decode, TypeInfo, Debug, PartialEq)]
pub struct EncryptedShare {
    /// Recipient signer index
    pub to_index: u16,
    /// From signer index
    pub from_index: u16,
    /// Encrypted share data (x25519 box)
    pub ciphertext: Vec<u8>,
    /// Ephemeral public key for decryption
    pub ephemeral_pk: [u8; 32],
}

/// DKG ceremony state
#[derive(Clone, Encode, Decode, TypeInfo, MaxEncodedLen, Debug, PartialEq, Default)]
pub enum DkgPhase {
    /// No DKG in progress
    #[default]
    Idle,
    /// Round 1: Collecting commitments
    Round1 { deadline: u32 },
    /// Round 2: Sharing encrypted shares
    Round2 { deadline: u32 },
    /// Round 3: Verification and finalization
    Round3 { deadline: u32 },
    /// DKG failed, will retry
    Failed { reason: DkgFailureReason },
}

/// Why DKG failed
#[derive(Clone, Encode, Decode, DecodeWithMemTracking, TypeInfo, MaxEncodedLen, Debug, PartialEq)]
pub enum DkgFailureReason {
    Timeout,
    InvalidCommitment,
    InvalidShare,
    InsufficientParticipation,
}

/// Max tx data size in bytes
pub type MaxTxDataSize = frame_support::traits::ConstU32<8192>;
/// Max partial signatures (same as max signers)
pub type MaxPartialSigs = frame_support::traits::ConstU32<256>;
/// Max active signers
pub type MaxSignerCount = frame_support::traits::ConstU32<256>;
/// Max encrypted share size in bytes
pub type MaxEncryptedShareSize = frame_support::traits::ConstU32<512>;

/// Signing request queued for processing
#[derive(Clone, Encode, Decode, TypeInfo, MaxEncodedLen, Debug, PartialEq)]
#[scale_info(skip_type_params(S, P))]
pub struct SigningRequest<AccountId, S: Get<u32>, P: Get<u32>> {
    /// Unique request ID
    pub id: u64,
    /// Who requested (for fee payment)
    pub requester: AccountId,
    /// Zcash transaction to sign (serialized)
    pub tx_data: BoundedVec<u8, S>,
    /// Block when request was created
    pub created_at: u32,
    /// Deadline block for signing
    pub deadline: u32,
    /// Fee offered to signers (in satoshis, split among t-of-n who sign)
    pub signer_fee: u64,
    /// Signers who committed to sign (index -> block committed)
    pub committed_signers: BoundedVec<(u16, u32), P>,
    /// Collected partial signatures
    pub partial_sigs: BoundedVec<(u16, FrostSignature), P>,
    /// Final aggregated signature (when complete)
    pub final_sig: Option<FrostSignature>,
    /// Status of the signing request
    pub status: SigningRequestStatus,
}

/// Status of a signing request
#[derive(Clone, Copy, Encode, Decode, DecodeWithMemTracking, TypeInfo, MaxEncodedLen, Debug, PartialEq, Eq, Default)]
pub enum SigningRequestStatus {
    /// Waiting for signers to commit
    #[default]
    WaitingForCommitments,
    /// Enough signers committed, waiting for signatures
    Signing,
    /// Signing complete
    Complete,
    /// Failed (timeout or insufficient commitments)
    Failed,
}

/// Signer info
#[derive(Clone, Encode, Decode, TypeInfo, MaxEncodedLen, Debug, PartialEq)]
pub struct SignerInfo<AccountId> {
    /// Account ID
    pub account: AccountId,
    /// Signer index in current set
    pub index: u16,
    /// Public key share
    pub public_share: [u8; 32],
    /// X25519 public key for encrypted share delivery
    pub encryption_key: [u8; 32],
    /// Block when joined
    pub joined_at: u32,
    /// Current status
    pub status: SignerStatus,
}

/// Signer operational status - gentler than aggressive slashing
#[derive(Clone, Encode, Decode, TypeInfo, MaxEncodedLen, Debug, PartialEq, Default)]
pub enum SignerStatus {
    /// Active and participating
    #[default]
    Active,
    /// Temporarily frozen due to missed participation
    /// requires heartbeat/action to reactivate
    Frozen {
        /// block when frozen
        since_block: u32,
        /// reason for freeze
        reason: FreezeReason,
    },
    /// Voluntarily offline (signer-initiated)
    Offline {
        since_block: u32,
    },
    /// Pending removal from next epoch
    PendingRemoval,
}

/// Why a signer got frozen (not slashed, just paused)
#[derive(Clone, Encode, Decode, DecodeWithMemTracking, TypeInfo, MaxEncodedLen, Debug, PartialEq, Default)]
pub enum FreezeReason {
    /// Missed too many consecutive signing rounds
    #[default]
    MissedSigning,
    /// No heartbeat for too long
    MissedHeartbeat,
    /// Failed DKG participation
    DkgFailure,
}

/// Participation statistics for a signer
#[derive(Clone, Encode, Decode, TypeInfo, MaxEncodedLen, Debug, PartialEq, Default)]
pub struct ParticipationStats {
    /// Total signing rounds available to participate in
    pub signing_rounds_available: u32,
    /// Signing rounds actually participated in
    pub signing_rounds_participated: u32,
    /// Last block where signer participated
    pub last_participation_block: u32,
    /// Consecutive missed signing rounds in current epoch
    pub consecutive_misses: u32,
    /// Small penalty accrued this epoch (resets each epoch)
    pub epoch_penalty: u128,
    /// Total penalties ever (for reputation tracking)
    pub lifetime_penalty: u128,
    /// Times frozen (reputation metric)
    pub freeze_count: u32,
}

/// Bridge operational state
#[derive(Clone, Encode, Decode, TypeInfo, MaxEncodedLen, Debug, PartialEq, Default)]
pub enum BridgeState {
    /// Bridge is operating normally
    #[default]
    Active,
    /// Circuit breaker tripped
    CircuitBroken {
        reason: CircuitBreakReason,
        since_block: u32,
    },
    /// Emergency recovery in progress
    EmergencyRecovery {
        initiated_at: u32,
        recovery_address: [u8; 64],
    },
}

/// Why circuit breaker was triggered
#[derive(Clone, Encode, Decode, DecodeWithMemTracking, TypeInfo, MaxEncodedLen, Debug, PartialEq)]
pub enum CircuitBreakReason {
    /// Too many signers offline
    InsufficientLiveness,
    /// Signing failed repeatedly
    RepeatedSigningFailure,
    /// DKG failed repeatedly
    RepeatedDkgFailure,
    /// Manual intervention
    ManualHalt,
}

/// Penalty reason - small fees rather than aggressive slashing
#[derive(Clone, Encode, Decode, DecodeWithMemTracking, TypeInfo, MaxEncodedLen, Debug, PartialEq)]
pub enum PenaltyReason {
    /// Missed a signing round (small penalty per miss)
    MissedSigningRound,
    /// Submitted invalid partial signature
    InvalidPartialSignature,
    /// Late participation (arrived after deadline but before completion)
    LateParticipation,
}

/// Severe violation that results in freeze + potential removal
#[derive(Clone, Encode, Decode, DecodeWithMemTracking, TypeInfo, MaxEncodedLen, Debug, PartialEq)]
pub enum ViolationReason {
    /// Double signing (catastrophic - only case for real slashing)
    DoubleSign,
    /// Equivocation proof submitted
    Equivocation,
}

#[frame_support::pallet]
pub mod pallet {
    use super::*;
    use frame_support::pallet_prelude::*;
    use frame_system::pallet_prelude::*;

    #[pallet::pallet]
    pub struct Pallet<T>(_);

    #[pallet::config]
    pub trait Config: frame_system::Config {
        /// Runtime event type
        type RuntimeEvent: From<Event<Self>> + IsType<<Self as frame_system::Config>::RuntimeEvent>;

        /// Minimum signers for committee
        #[pallet::constant]
        type MinSigners: Get<u16>;

        /// Maximum signers for committee
        #[pallet::constant]
        type MaxSigners: Get<u16>;

        /// Signing threshold (t in t-of-n)
        #[pallet::constant]
        type Threshold: Get<u16>;

        /// DKG round timeout in blocks
        #[pallet::constant]
        type DkgTimeout: Get<u32>;

        /// Signing request timeout in blocks
        #[pallet::constant]
        type SigningTimeout: Get<u32>;

        /// Blocks between key rotations
        #[pallet::constant]
        type RotationPeriod: Get<u32>;

        /// Heartbeat interval in blocks
        #[pallet::constant]
        type HeartbeatInterval: Get<u32>;

        /// Blocks without heartbeat before considered offline
        #[pallet::constant]
        type OfflineThreshold: Get<u32>;

        /// Consecutive signing misses before slashing starts
        #[pallet::constant]
        type SlashingGracePeriod: Get<u32>;

        /// Minimum participation rate (percent, 0-100)
        #[pallet::constant]
        type MinParticipationRate: Get<u8>;

        /// Consecutive signing failures before circuit breaker
        #[pallet::constant]
        type CircuitBreakerThreshold: Get<u32>;
    }

    // ============ Storage ============

    /// Current signer set
    #[pallet::storage]
    pub type Signers<T: Config> = StorageMap<
        _,
        Blake2_128Concat,
        T::AccountId,
        SignerInfo<T::AccountId>,
    >;

    /// Ordered list of active signer accounts
    #[pallet::storage]
    pub type ActiveSignerList<T: Config> = StorageValue<_, BoundedVec<T::AccountId, MaxSignerCount>, ValueQuery>;

    /// Current DKG phase
    #[pallet::storage]
    pub type CurrentDkgPhase<T: Config> = StorageValue<_, DkgPhase, ValueQuery>;

    /// DKG round 1 commitments
    #[pallet::storage]
    pub type DkgCommitments<T: Config> = StorageMap<
        _,
        Blake2_128Concat,
        u16, // signer index
        [u8; 32], // commitment hash
    >;

    /// DKG round 2 encrypted shares
    #[pallet::storage]
    pub type DkgShares<T: Config> = StorageDoubleMap<
        _,
        Blake2_128Concat,
        u16, // from index
        Blake2_128Concat,
        u16, // to index
        BoundedVec<u8, MaxEncryptedShareSize>, // encrypted share
    >;

    /// Current group public key (the Zcash custody address)
    #[pallet::storage]
    pub type GroupPublicKey<T: Config> = StorageValue<_, [u8; 32]>;

    /// Next signing request ID
    #[pallet::storage]
    pub type NextRequestId<T: Config> = StorageValue<_, u64, ValueQuery>;

    /// Pending signing requests
    #[pallet::storage]
    pub type SigningQueue<T: Config> = StorageMap<
        _,
        Blake2_128Concat,
        u64, // request ID
        SigningRequest<T::AccountId, MaxTxDataSize, MaxPartialSigs>,
    >;

    /// Last key rotation block
    #[pallet::storage]
    pub type LastRotation<T: Config> = StorageValue<_, u32, ValueQuery>;

    /// Nonces used for replay protection
    #[pallet::storage]
    pub type UsedNonces<T: Config> = StorageMap<
        _,
        Blake2_128Concat,
        [u8; 32],
        bool,
        ValueQuery,
    >;

    // ============ Liveness Tracking Storage ============

    /// Participation statistics per signer
    #[pallet::storage]
    pub type SignerParticipation<T: Config> = StorageMap<
        _,
        Blake2_128Concat,
        T::AccountId,
        ParticipationStats,
        ValueQuery,
    >;

    /// Last heartbeat block per signer
    #[pallet::storage]
    pub type LastHeartbeat<T: Config> = StorageMap<
        _,
        Blake2_128Concat,
        T::AccountId,
        u32,
        ValueQuery,
    >;

    /// Current bridge operational state
    #[pallet::storage]
    pub type CurrentBridgeState<T: Config> = StorageValue<_, BridgeState, ValueQuery>;

    /// Consecutive signing failures (for circuit breaker)
    #[pallet::storage]
    pub type ConsecutiveSigningFailures<T: Config> = StorageValue<_, u32, ValueQuery>;

    /// Expected heartbeat challenge (rotates each interval)
    #[pallet::storage]
    pub type HeartbeatChallenge<T: Config> = StorageValue<_, [u8; 32], ValueQuery>;

    /// Block when current epoch started (for participation calculation)
    #[pallet::storage]
    pub type EpochStartBlock<T: Config> = StorageValue<_, u32, ValueQuery>;

    // ============ Events ============

    #[pallet::event]
    #[pallet::generate_deposit(pub(super) fn deposit_event)]
    pub enum Event<T: Config> {
        /// Signer registered for committee
        SignerRegistered {
            who: T::AccountId,
            index: u16,
            encryption_key: [u8; 32],
        },

        /// DKG ceremony started
        DkgStarted {
            round: u8,
            participants: u16,
            deadline: u32,
        },

        /// DKG round completed
        DkgRoundCompleted {
            round: u8,
        },

        /// DKG succeeded, new group key active
        DkgCompleted {
            group_public_key: [u8; 32],
            threshold: u16,
            signers: u16,
        },

        /// DKG failed
        DkgFailed {
            reason: DkgFailureReason,
        },

        /// Signing request queued
        SigningRequestCreated {
            request_id: u64,
            requester: T::AccountId,
            deadline: u32,
        },

        /// Partial signature submitted
        PartialSignatureSubmitted {
            request_id: u64,
            signer_index: u16,
        },

        /// Signing completed successfully
        SigningCompleted {
            request_id: u64,
            signature: FrostSignature,
            /// how many signers participated (got rewarded)
            participants: u16,
        },

        /// Signing failed (timeout or invalid sigs)
        SigningFailed {
            request_id: u64,
        },

        /// Signing reward distributed to participant
        SigningRewardPaid {
            signer: T::AccountId,
            amount: u128,
        },

        /// Key rotation initiated
        RotationStarted {
            old_signers: u16,
            new_signers: u16,
        },

        /// Small penalty applied (not aggressive slashing)
        SignerPenalized {
            who: T::AccountId,
            amount: u128,
            reason: PenaltyReason,
        },

        /// Signer frozen (temporarily suspended, needs reactivation)
        SignerFrozen {
            who: T::AccountId,
            reason: FreezeReason,
        },

        /// Signer reactivated after freeze
        SignerReactivated {
            who: T::AccountId,
        },

        /// Severe violation detected (double-sign etc)
        SevereViolation {
            who: T::AccountId,
            reason: ViolationReason,
        },

        /// Heartbeat received
        HeartbeatReceived {
            who: T::AccountId,
            block: u32,
        },

        /// Signer went offline voluntarily
        SignerWentOffline {
            who: T::AccountId,
        },

        /// Circuit breaker triggered
        CircuitBreakerTriggered {
            reason: CircuitBreakReason,
        },

        /// Bridge resumed from circuit break
        BridgeResumed,

        /// Emergency recovery initiated
        EmergencyRecoveryInitiated {
            recovery_address: [u8; 64],
        },

        /// Missing signer reported
        MissingSignerReported {
            reporter: T::AccountId,
            missing: T::AccountId,
            signing_round: u64,
        },

        /// New heartbeat challenge set
        HeartbeatChallengeRotated {
            challenge: [u8; 32],
        },
    }

    // ============ Errors ============

    #[pallet::error]
    pub enum Error<T> {
        /// Already registered as signer
        AlreadyRegistered,
        /// Not registered as signer
        NotRegistered,
        /// Not enough signers for DKG
        InsufficientSigners,
        /// Too many signers
        TooManySigners,
        /// DKG not in expected phase
        WrongDkgPhase,
        /// DKG deadline passed
        DkgTimeout,
        /// Invalid commitment
        InvalidCommitment,
        /// Invalid share
        InvalidShare,
        /// No group key (DKG not complete)
        NoGroupKey,
        /// Invalid signing request
        InvalidRequest,
        /// Signing deadline passed
        SigningTimeout,
        /// Invalid partial signature
        InvalidPartialSignature,
        /// Already submitted partial sig
        AlreadySigned,
        /// Request not found
        RequestNotFound,
        /// Nonce already used
        NonceReused,
        /// Bridge is halted
        BridgeHalted,
        /// Invalid heartbeat response
        InvalidHeartbeat,
        /// Signer not missing (false report)
        SignerNotMissing,
        /// Signer is frozen (submit heartbeat to reactivate)
        SignerFrozen,
        /// Not authorized for this action
        NotAuthorized,
        /// Emergency recovery already in progress
        RecoveryInProgress,
        /// Cannot resume - not in halted state
        NotHalted,
        /// Request deadline has not expired
        TimeNotExpired,
    }

    // ============ Calls ============

    #[pallet::call]
    impl<T: Config> Pallet<T> {
        /// Register as a signer with encryption key for DKG
        #[pallet::call_index(0)]
        #[pallet::weight(10_000)]
        pub fn register_signer(
            origin: OriginFor<T>,
            encryption_key: [u8; 32],
        ) -> DispatchResult {
            let who = ensure_signed(origin)?;

            ensure!(!Signers::<T>::contains_key(&who), Error::<T>::AlreadyRegistered);

            let mut signers = ActiveSignerList::<T>::get();
            ensure!(signers.len() < T::MaxSigners::get() as usize, Error::<T>::TooManySigners);

            let index = (signers.len() + 1) as u16;
            signers.try_push(who.clone()).map_err(|_| Error::<T>::TooManySigners)?;
            ActiveSignerList::<T>::put(signers);

            let info = SignerInfo {
                account: who.clone(),
                index,
                public_share: [0u8; 32], // Set after DKG
                encryption_key,
                joined_at: frame_system::Pallet::<T>::block_number().saturated_into(),
                status: SignerStatus::default(), // Pending DKG completion to become Active
            };
            Signers::<T>::insert(&who, info);

            Self::deposit_event(Event::SignerRegistered {
                who,
                index,
                encryption_key,
            });

            // Check if we should start DKG
            Self::maybe_start_dkg();

            Ok(())
        }

        /// Submit DKG round 1 commitment
        #[pallet::call_index(1)]
        #[pallet::weight(10_000)]
        pub fn submit_dkg_commitment(
            origin: OriginFor<T>,
            commitment: [u8; 32],
        ) -> DispatchResult {
            let who = ensure_signed(origin)?;

            let signer = Signers::<T>::get(&who).ok_or(Error::<T>::NotRegistered)?;
            ensure!(matches!(CurrentDkgPhase::<T>::get(), DkgPhase::Round1 { .. }), Error::<T>::WrongDkgPhase);

            DkgCommitments::<T>::insert(signer.index, commitment);

            // Check if all commitments received
            Self::check_dkg_round1_complete();

            Ok(())
        }

        /// Submit DKG round 2 encrypted share
        #[pallet::call_index(2)]
        #[pallet::weight(10_000)]
        pub fn submit_dkg_share(
            origin: OriginFor<T>,
            to_index: u16,
            encrypted_share: BoundedVec<u8, MaxEncryptedShareSize>,
        ) -> DispatchResult {
            let who = ensure_signed(origin)?;

            let signer = Signers::<T>::get(&who).ok_or(Error::<T>::NotRegistered)?;
            ensure!(matches!(CurrentDkgPhase::<T>::get(), DkgPhase::Round2 { .. }), Error::<T>::WrongDkgPhase);

            DkgShares::<T>::insert(signer.index, to_index, encrypted_share);

            // Check if all shares received
            Self::check_dkg_round2_complete();

            Ok(())
        }

        /// Submit DKG round 3 verification result and public share
        #[pallet::call_index(3)]
        #[pallet::weight(10_000)]
        pub fn submit_dkg_verification(
            origin: OriginFor<T>,
            public_share: [u8; 32],
            proof: [u8; 64],
        ) -> DispatchResult {
            let who = ensure_signed(origin)?;

            let mut signer = Signers::<T>::get(&who).ok_or(Error::<T>::NotRegistered)?;
            ensure!(matches!(CurrentDkgPhase::<T>::get(), DkgPhase::Round3 { .. }), Error::<T>::WrongDkgPhase);

            // TODO: Verify proof of secret key knowledge

            signer.public_share = public_share;
            signer.status = SignerStatus::Active;
            Signers::<T>::insert(&who, signer);

            // Check if DKG complete
            Self::check_dkg_complete();

            Ok(())
        }

        /// Request a signature on Zcash transaction
        #[pallet::call_index(4)]
        #[pallet::weight(10_000)]
        pub fn request_signature(
            origin: OriginFor<T>,
            tx_data: BoundedVec<u8, MaxTxDataSize>,
            nonce: [u8; 32],
        ) -> DispatchResult {
            let who = ensure_signed(origin)?;

            ensure!(!UsedNonces::<T>::get(&nonce), Error::<T>::NonceReused);
            ensure!(GroupPublicKey::<T>::get().is_some(), Error::<T>::NoGroupKey);

            UsedNonces::<T>::insert(&nonce, true);

            let id = NextRequestId::<T>::get();
            NextRequestId::<T>::put(id + 1);

            let now: u32 = frame_system::Pallet::<T>::block_number().saturated_into();
            let deadline = now + T::SigningTimeout::get();

            let request = SigningRequest {
                id,
                requester: who.clone(),
                tx_data,
                created_at: now,
                deadline,
                signer_fee: 0, // todo: fee from request params
                committed_signers: BoundedVec::new(),
                partial_sigs: BoundedVec::new(),
                final_sig: None,
                status: SigningRequestStatus::WaitingForCommitments,
            };

            SigningQueue::<T>::insert(id, request);

            Self::deposit_event(Event::SigningRequestCreated {
                request_id: id,
                requester: who,
                deadline,
            });

            Ok(())
        }

        /// Submit partial signature (called from OCW via unsigned tx)
        #[pallet::call_index(5)]
        #[pallet::weight(10_000)]
        pub fn submit_partial_signature(
            origin: OriginFor<T>,
            request_id: u64,
            signer_index: u16,
            partial_sig: FrostSignature,
        ) -> DispatchResult {
            let who = ensure_signed(origin)?;

            let signer = Signers::<T>::get(&who).ok_or(Error::<T>::NotRegistered)?;
            ensure!(signer.index == signer_index, Error::<T>::NotRegistered);
            ensure!(matches!(signer.status, SignerStatus::Active), Error::<T>::SignerFrozen);

            let mut request = SigningQueue::<T>::get(request_id)
                .ok_or(Error::<T>::RequestNotFound)?;

            // Check not already signed
            ensure!(
                !request.partial_sigs.iter().any(|(idx, _)| *idx == signer_index),
                Error::<T>::AlreadySigned
            );

            // Check deadline
            let now: u32 = frame_system::Pallet::<T>::block_number().saturated_into();
            ensure!(now <= request.deadline, Error::<T>::SigningTimeout);

            // TODO: Verify partial signature is valid for this signer

            // BoundedVec try_push - we've already checked signer count so this should succeed
            let _ = request.partial_sigs.try_push((signer_index, partial_sig));

            Self::deposit_event(Event::PartialSignatureSubmitted {
                request_id,
                signer_index,
            });

            // Check if we have threshold - finalize immediately when reached
            let threshold = T::Threshold::get() as usize;
            if request.partial_sigs.len() >= threshold && request.final_sig.is_none() {
                // Aggregate signatures
                if let Some(final_sig) = Self::aggregate_signatures(&request.partial_sigs) {
                    request.final_sig = Some(final_sig.clone());

                    // Reward participants who signed in time
                    Self::distribute_signing_rewards(&request.partial_sigs);

                    // Update participation stats for signers who made it
                    for (idx, _) in request.partial_sigs.iter() {
                        if let Some(account) = Self::signer_by_index(*idx) {
                            SignerParticipation::<T>::mutate(&account, |stats| {
                                stats.signing_rounds_participated += 1;
                                stats.consecutive_misses = 0; // reset on successful participation
                                stats.last_participation_block = now;
                            });
                        }
                    }

                    Self::deposit_event(Event::SigningCompleted {
                        request_id,
                        signature: final_sig,
                        participants: request.partial_sigs.len() as u16,
                    });
                }
            }

            SigningQueue::<T>::insert(request_id, request);

            Ok(())
        }

        // ============ Liveness Calls ============

        /// Submit heartbeat proving signer is online and responsive
        #[pallet::call_index(6)]
        #[pallet::weight(10_000)]
        pub fn submit_heartbeat(
            origin: OriginFor<T>,
            challenge_response: [u8; 64],
        ) -> DispatchResult {
            let who = ensure_signed(origin)?;

            let mut signer = Signers::<T>::get(&who).ok_or(Error::<T>::NotRegistered)?;

            // verify challenge response (signature over current challenge)
            let challenge = HeartbeatChallenge::<T>::get();
            ensure!(
                Self::verify_heartbeat_signature(&who, &challenge, &challenge_response),
                Error::<T>::InvalidHeartbeat
            );

            let now: u32 = frame_system::Pallet::<T>::block_number().saturated_into();
            LastHeartbeat::<T>::insert(&who, now);

            // if frozen, reactivate on valid heartbeat (this is how they come back)
            if matches!(signer.status, SignerStatus::Frozen { .. }) {
                signer.status = SignerStatus::Active;
                Signers::<T>::insert(&who, signer);

                // reset consecutive misses on reactivation
                SignerParticipation::<T>::mutate(&who, |stats| {
                    stats.consecutive_misses = 0;
                });

                Self::deposit_event(Event::SignerReactivated {
                    who: who.clone(),
                });
            }

            Self::deposit_event(Event::HeartbeatReceived {
                who,
                block: now,
            });

            Ok(())
        }

        /// Report a signer who failed to participate in a signing round
        #[pallet::call_index(7)]
        #[pallet::weight(10_000)]
        pub fn report_missing_signer(
            origin: OriginFor<T>,
            missing_signer: T::AccountId,
            signing_round_id: u64,
        ) -> DispatchResult {
            let reporter = ensure_signed(origin)?;

            // reporter must be a signer
            ensure!(Signers::<T>::contains_key(&reporter), Error::<T>::NotRegistered);

            // verify the signer actually missed the round
            let request = SigningQueue::<T>::get(signing_round_id)
                .ok_or(Error::<T>::RequestNotFound)?;

            let signer_info = Signers::<T>::get(&missing_signer)
                .ok_or(Error::<T>::NotRegistered)?;

            // check request is past deadline
            let now: u32 = frame_system::Pallet::<T>::block_number().saturated_into();
            ensure!(now > request.deadline, Error::<T>::TimeNotExpired);

            // check signer didn't participate
            let participated = request.partial_sigs.iter()
                .any(|(idx, _)| *idx == signer_info.index);
            ensure!(!participated, Error::<T>::SignerNotMissing);

            // update participation stats
            SignerParticipation::<T>::mutate(&missing_signer, |stats| {
                stats.signing_rounds_available += 1;
                stats.consecutive_misses += 1;
            });

            // apply small penalty per miss (not aggressive slashing)
            Self::apply_penalty(&missing_signer, PenaltyReason::MissedSigningRound);

            // freeze if too many consecutive misses
            let stats = SignerParticipation::<T>::get(&missing_signer);
            if stats.consecutive_misses >= Self::max_consecutive_misses() {
                Self::freeze_signer(&missing_signer, FreezeReason::MissedSigning, now);
            }

            Self::deposit_event(Event::MissingSignerReported {
                reporter,
                missing: missing_signer,
                signing_round: signing_round_id,
            });

            Ok(())
        }

        /// Initiate emergency recovery (privileged - governance or sudo)
        #[pallet::call_index(8)]
        #[pallet::weight(100_000)]
        pub fn initiate_emergency_recovery(
            origin: OriginFor<T>,
            recovery_address: [u8; 64],
        ) -> DispatchResult {
            ensure_root(origin)?;

            ensure!(
                !matches!(CurrentBridgeState::<T>::get(), BridgeState::EmergencyRecovery { .. }),
                Error::<T>::RecoveryInProgress
            );

            let now: u32 = frame_system::Pallet::<T>::block_number().saturated_into();

            CurrentBridgeState::<T>::put(BridgeState::EmergencyRecovery {
                initiated_at: now,
                recovery_address,
            });

            Self::deposit_event(Event::EmergencyRecoveryInitiated { recovery_address });

            Ok(())
        }

        /// Resume bridge after circuit break (privileged)
        #[pallet::call_index(9)]
        #[pallet::weight(10_000)]
        pub fn resume_bridge(origin: OriginFor<T>) -> DispatchResult {
            ensure_root(origin)?;

            ensure!(
                matches!(CurrentBridgeState::<T>::get(), BridgeState::CircuitBroken { .. }),
                Error::<T>::NotHalted
            );

            CurrentBridgeState::<T>::put(BridgeState::Active);
            ConsecutiveSigningFailures::<T>::put(0);

            Self::deposit_event(Event::BridgeResumed);

            Ok(())
        }

        /// Manually trigger circuit breaker (privileged)
        #[pallet::call_index(10)]
        #[pallet::weight(10_000)]
        pub fn halt_bridge(origin: OriginFor<T>) -> DispatchResult {
            ensure_root(origin)?;

            let now: u32 = frame_system::Pallet::<T>::block_number().saturated_into();

            CurrentBridgeState::<T>::put(BridgeState::CircuitBroken {
                reason: CircuitBreakReason::ManualHalt,
                since_block: now,
            });

            Self::deposit_event(Event::CircuitBreakerTriggered {
                reason: CircuitBreakReason::ManualHalt,
            });

            Ok(())
        }
    }

    // ============ Hooks ============

    #[pallet::hooks]
    impl<T: Config> Hooks<BlockNumberFor<T>> for Pallet<T> {
        /// Offchain worker entry point
        fn offchain_worker(block_number: BlockNumberFor<T>) {
            // Process DKG if in progress
            Self::ocw_process_dkg();

            // Process signing requests
            Self::ocw_process_signing();

            // Check for rotation
            Self::ocw_check_rotation(block_number);
        }

        /// On block finalization - check timeouts
        fn on_finalize(block_number: BlockNumberFor<T>) {
            let now: u32 = block_number.saturated_into();

            // Check DKG timeout
            match CurrentDkgPhase::<T>::get() {
                DkgPhase::Round1 { deadline } |
                DkgPhase::Round2 { deadline } |
                DkgPhase::Round3 { deadline } => {
                    if now > deadline {
                        CurrentDkgPhase::<T>::put(DkgPhase::Failed {
                            reason: DkgFailureReason::Timeout,
                        });
                        Self::deposit_event(Event::DkgFailed {
                            reason: DkgFailureReason::Timeout,
                        });
                    }
                }
                _ => {}
            }

            // process expired signing rounds and track participation
            Self::process_expired_signing_rounds(now);

            // check signer liveness based on heartbeats
            Self::check_signer_liveness(now);

            // rotate heartbeat challenge periodically
            if now % T::HeartbeatInterval::get() == 0 {
                Self::rotate_heartbeat_challenge(now);
            }
        }
    }

    // ============ Internal Functions ============

    impl<T: Config> Pallet<T> {
        /// Check if we should start DKG
        fn maybe_start_dkg() {
            if !matches!(CurrentDkgPhase::<T>::get(), DkgPhase::Idle) {
                return;
            }

            let signers = ActiveSignerList::<T>::get();
            if signers.len() >= T::MinSigners::get() as usize {
                let now: u32 = frame_system::Pallet::<T>::block_number().saturated_into();
                let deadline = now + T::DkgTimeout::get();

                CurrentDkgPhase::<T>::put(DkgPhase::Round1 { deadline });

                Self::deposit_event(Event::DkgStarted {
                    round: 1,
                    participants: signers.len() as u16,
                    deadline,
                });
            }
        }

        /// Check if DKG round 1 is complete
        fn check_dkg_round1_complete() {
            let signers = ActiveSignerList::<T>::get();
            let commitment_count = (1..=signers.len() as u16)
                .filter(|i| DkgCommitments::<T>::contains_key(i))
                .count();

            if commitment_count == signers.len() {
                let now: u32 = frame_system::Pallet::<T>::block_number().saturated_into();
                let deadline = now + T::DkgTimeout::get();

                CurrentDkgPhase::<T>::put(DkgPhase::Round2 { deadline });

                Self::deposit_event(Event::DkgRoundCompleted { round: 1 });
                Self::deposit_event(Event::DkgStarted {
                    round: 2,
                    participants: signers.len() as u16,
                    deadline,
                });
            }
        }

        /// Check if DKG round 2 is complete
        fn check_dkg_round2_complete() {
            let signers = ActiveSignerList::<T>::get();
            let n = signers.len() as u16;

            // Each signer sends to all others (n * (n-1) shares)
            let expected_shares = (n * (n - 1)) as usize;
            let mut share_count = 0;

            for from in 1..=n {
                for to in 1..=n {
                    if from != to && DkgShares::<T>::contains_key(from, to) {
                        share_count += 1;
                    }
                }
            }

            if share_count == expected_shares {
                let now: u32 = frame_system::Pallet::<T>::block_number().saturated_into();
                let deadline = now + T::DkgTimeout::get();

                CurrentDkgPhase::<T>::put(DkgPhase::Round3 { deadline });

                Self::deposit_event(Event::DkgRoundCompleted { round: 2 });
                Self::deposit_event(Event::DkgStarted {
                    round: 3,
                    participants: n,
                    deadline,
                });
            }
        }

        /// Check if DKG is complete
        fn check_dkg_complete() {
            let signers = ActiveSignerList::<T>::get();
            let active_count = signers.iter()
                .filter_map(|account| Signers::<T>::get(account))
                .filter(|info| matches!(info.status, SignerStatus::Active))
                .count();

            if active_count == signers.len() {
                // Compute group public key from shares
                let group_key = Self::compute_group_public_key();
                GroupPublicKey::<T>::put(group_key);

                CurrentDkgPhase::<T>::put(DkgPhase::Idle);

                let now: u32 = frame_system::Pallet::<T>::block_number().saturated_into();
                LastRotation::<T>::put(now);

                Self::deposit_event(Event::DkgCompleted {
                    group_public_key: group_key,
                    threshold: T::Threshold::get(),
                    signers: signers.len() as u16,
                });
            }
        }

        /// Compute group public key from individual shares
        fn compute_group_public_key() -> [u8; 32] {
            // In real implementation: sum of public key shares
            // For now, placeholder
            let signers = ActiveSignerList::<T>::get();
            let mut result = [0u8; 32];

            for account in signers.iter() {
                if let Some(info) = Signers::<T>::get(account) {
                    // XOR together (placeholder for actual EC addition)
                    for (i, byte) in info.public_share.iter().enumerate() {
                        result[i] ^= byte;
                    }
                }
            }

            result
        }

        /// Aggregate partial signatures into final signature
        fn aggregate_signatures(partials: &[(u16, FrostSignature)]) -> Option<FrostSignature> {
            if partials.is_empty() {
                return None;
            }

            // In real implementation: Lagrange interpolation of s values
            // R values are summed
            // For now, placeholder aggregation
            let mut r = [0u8; 32];
            let mut s = [0u8; 32];

            for (_, sig) in partials {
                for i in 0..32 {
                    r[i] ^= sig.r[i];
                    s[i] ^= sig.s[i];
                }
            }

            Some(FrostSignature { r, s })
        }

        // ============ Liveness Functions ============

        /// Verify heartbeat signature
        fn verify_heartbeat_signature(
            _who: &T::AccountId,
            _challenge: &[u8; 32],
            _response: &[u8; 64],
        ) -> bool {
            // todo: verify sr25519/ed25519 signature over challenge
            // for now, accept all (placeholder)
            true
        }

        /// Apply small penalty for missed participation (not aggressive slashing)
        fn apply_penalty(who: &T::AccountId, reason: PenaltyReason) {
            let amount = Self::calculate_penalty_amount(&reason);

            SignerParticipation::<T>::mutate(who, |stats| {
                stats.epoch_penalty = stats.epoch_penalty.saturating_add(amount);
                stats.lifetime_penalty = stats.lifetime_penalty.saturating_add(amount);
            });

            // todo: deduct from staked balance (small amount)
            // this is a fee, not punishment

            Self::deposit_event(Event::SignerPenalized {
                who: who.clone(),
                amount,
                reason,
            });
        }

        /// Freeze a signer (suspend until they reactivate)
        fn freeze_signer(who: &T::AccountId, reason: FreezeReason, now: u32) {
            if let Some(mut signer) = Signers::<T>::get(who) {
                signer.status = SignerStatus::Frozen {
                    since_block: now,
                    reason: reason.clone(),
                };
                Signers::<T>::insert(who, signer);

                SignerParticipation::<T>::mutate(who, |stats| {
                    stats.freeze_count = stats.freeze_count.saturating_add(1);
                });

                Self::deposit_event(Event::SignerFrozen {
                    who: who.clone(),
                    reason,
                });
            }
        }

        /// Calculate penalty amount - much smaller than slashing
        fn calculate_penalty_amount(reason: &PenaltyReason) -> u128 {
            // amounts in basis points of stake (very small)
            match reason {
                PenaltyReason::MissedSigningRound => 1,  // 0.01% per miss
                PenaltyReason::InvalidPartialSignature => 5,  // 0.05%
                PenaltyReason::LateParticipation => 0,  // warning only, no penalty
            }
        }

        /// Max consecutive misses before freeze
        fn max_consecutive_misses() -> u32 {
            5  // freeze after 5 consecutive misses
        }

        /// Look up signer account by index
        fn signer_by_index(index: u16) -> Option<T::AccountId> {
            let signers = ActiveSignerList::<T>::get();
            for account in signers.iter() {
                if let Some(info) = Signers::<T>::get(account) {
                    if info.index == index {
                        return Some(account.clone());
                    }
                }
            }
            None
        }

        /// Distribute signing rewards to participants
        /// Only those who submitted partial sigs before threshold was reached get paid
        fn distribute_signing_rewards(
            participants: &BoundedVec<(u16, FrostSignature), MaxPartialSigs>,
        ) {
            if participants.is_empty() {
                return;
            }

            // todo: get reward pool amount from config or fee collection
            // for now use a fixed amount per signing round
            let total_reward: u128 = 1000; // placeholder - integrate with actual reward pool
            let per_signer = total_reward / (participants.len() as u128);

            for (idx, _) in participants.iter() {
                if let Some(account) = Self::signer_by_index(*idx) {
                    // todo: actually transfer/mint reward tokens
                    // this would integrate with pallet-balances or orml_tokens

                    Self::deposit_event(Event::SigningRewardPaid {
                        signer: account,
                        amount: per_signer,
                    });
                }
            }
        }

        /// Check signer liveness and apply freeze if needed
        fn check_signer_liveness(now: u32) {
            let signers = ActiveSignerList::<T>::get();
            let offline_threshold = T::OfflineThreshold::get();
            let mut offline_count = 0u32;

            for account in signers.iter() {
                let signer = match Signers::<T>::get(account) {
                    Some(s) => s,
                    None => continue,
                };

                // skip already frozen/offline signers
                if !matches!(signer.status, SignerStatus::Active) {
                    offline_count += 1;
                    continue;
                }

                let last_heartbeat = LastHeartbeat::<T>::get(account);
                let blocks_since = now.saturating_sub(last_heartbeat);

                if blocks_since > offline_threshold {
                    offline_count += 1;

                    // freeze if no heartbeat for too long (not slash!)
                    if blocks_since > offline_threshold * 2 {
                        Self::freeze_signer(account, FreezeReason::MissedHeartbeat, now);
                    }
                }
            }

            // trigger circuit breaker if too many offline
            let total_signers = signers.len() as u32;
            let threshold = T::Threshold::get() as u32;
            let active_signers = total_signers.saturating_sub(offline_count);

            if active_signers < threshold {
                Self::trigger_circuit_breaker(CircuitBreakReason::InsufficientLiveness, now);
            }
        }

        /// Trigger circuit breaker
        fn trigger_circuit_breaker(reason: CircuitBreakReason, now: u32) {
            if matches!(CurrentBridgeState::<T>::get(), BridgeState::Active) {
                CurrentBridgeState::<T>::put(BridgeState::CircuitBroken {
                    reason: reason.clone(),
                    since_block: now,
                });

                Self::deposit_event(Event::CircuitBreakerTriggered { reason });
            }
        }

        /// Process expired signing rounds and update stats
        fn process_expired_signing_rounds(now: u32) {
            // iterate signing queue (in production, use drain_filter or similar)
            let mut failures_this_block = 0u32;

            for (request_id, request) in SigningQueue::<T>::iter() {
                if now > request.deadline && request.final_sig.is_none() {
                    // signing failed
                    failures_this_block += 1;

                    // mark non-participants
                    let signers = ActiveSignerList::<T>::get();
                    for account in signers.iter() {
                        if let Some(info) = Signers::<T>::get(account) {
                            let participated = request.partial_sigs.iter()
                                .any(|(idx, _)| *idx == info.index);

                            SignerParticipation::<T>::mutate(account, |stats| {
                                stats.signing_rounds_available += 1;
                                if participated {
                                    stats.signing_rounds_participated += 1;
                                    stats.consecutive_misses = 0;
                                    stats.last_participation_block = now;
                                } else {
                                    stats.consecutive_misses += 1;
                                }
                            });
                        }
                    }

                    Self::deposit_event(Event::SigningFailed { request_id });

                    // clean up
                    SigningQueue::<T>::remove(request_id);
                }
            }

            // update consecutive failure counter
            if failures_this_block > 0 {
                let total = ConsecutiveSigningFailures::<T>::mutate(|n| {
                    *n = n.saturating_add(failures_this_block);
                    *n
                });

                if total >= T::CircuitBreakerThreshold::get() {
                    Self::trigger_circuit_breaker(CircuitBreakReason::RepeatedSigningFailure, now);
                }
            }
        }

        /// Rotate heartbeat challenge
        fn rotate_heartbeat_challenge(now: u32) {
            // generate new challenge based on block hash
            let parent_hash = frame_system::Pallet::<T>::parent_hash();
            let mut challenge = [0u8; 32];
            let hash_bytes = parent_hash.as_ref();
            for (i, byte) in hash_bytes.iter().take(32).enumerate() {
                challenge[i] = *byte;
            }
            // mix in block number
            let now_bytes = now.to_le_bytes();
            for (i, byte) in now_bytes.iter().enumerate() {
                challenge[i] ^= byte;
            }

            HeartbeatChallenge::<T>::put(challenge);

            Self::deposit_event(Event::HeartbeatChallengeRotated { challenge });
        }

        // ============ OCW Functions ============

        /// OCW: Process DKG participation
        #[cfg(feature = "std")]
        fn ocw_process_dkg() {
            use sp_runtime::offchain::storage::StorageValueRef;

            let phase = CurrentDkgPhase::<T>::get();

            match phase {
                DkgPhase::Round1 { .. } => {
                    // Check if we have local key share
                    let storage = StorageValueRef::persistent(b"frost_bridge::local_index");
                    if let Ok(Some(index)) = storage.get::<u16>() {
                        // Generate commitment
                        Self::ocw_generate_and_submit_commitment(index);
                    }
                }
                DkgPhase::Round2 { .. } => {
                    let storage = StorageValueRef::persistent(b"frost_bridge::local_index");
                    if let Ok(Some(index)) = storage.get::<u16>() {
                        // Generate and distribute shares
                        Self::ocw_generate_and_submit_shares(index);
                    }
                }
                DkgPhase::Round3 { .. } => {
                    let storage = StorageValueRef::persistent(b"frost_bridge::local_index");
                    if let Ok(Some(index)) = storage.get::<u16>() {
                        // Verify received shares and compute public key
                        Self::ocw_verify_and_finalize(index);
                    }
                }
                _ => {}
            }
        }

        #[cfg(not(feature = "std"))]
        fn ocw_process_dkg() {}

        /// OCW: Process pending signing requests
        #[cfg(feature = "std")]
        fn ocw_process_signing() {
            use sp_runtime::offchain::storage::StorageValueRef;

            let storage = StorageValueRef::persistent(b"frost_bridge::local_index");
            let Ok(Some(index)) = storage.get::<u16>() else {
                return;
            };

            // Get local secret share
            let share_storage = StorageValueRef::persistent(b"frost_bridge::secret_share");
            let Ok(Some(secret_share)) = share_storage.get::<[u8; 32]>() else {
                return;
            };

            // Iterate signing queue and create partial sigs
            // (In production, this would iterate actual storage)
        }

        #[cfg(not(feature = "std"))]
        fn ocw_process_signing() {}

        /// OCW: Check if rotation needed
        fn ocw_check_rotation<N>(_block_number: N) {
            // Check if rotation period elapsed
            // If so, initiate key resharing
        }

        #[cfg(feature = "std")]
        fn ocw_generate_and_submit_commitment(_index: u16) {
            // Generate random polynomial coefficients
            // Compute commitment
            // Submit on-chain
        }

        #[cfg(feature = "std")]
        fn ocw_generate_and_submit_shares(_index: u16) {
            // Evaluate polynomial at each participant's index
            // Encrypt shares to recipient's encryption key
            // Submit on-chain
        }

        #[cfg(feature = "std")]
        fn ocw_verify_and_finalize(_index: u16) {
            // Decrypt received shares
            // Verify against commitments
            // Compute secret share sum
            // Derive public key share
            // Submit verification
        }
    }
}

// ============ OCW Key Storage ============

/// Keys stored in OCW local storage
#[cfg(feature = "std")]
pub mod ocw_storage {
    /// Local signer index
    pub const KEY_LOCAL_INDEX: &[u8] = b"frost_bridge::local_index";
    /// Secret key share (SENSITIVE!)
    pub const KEY_SECRET_SHARE: &[u8] = b"frost_bridge::secret_share";
    /// Polynomial coefficients for current DKG
    pub const KEY_DKG_POLY: &[u8] = b"frost_bridge::dkg_poly";
    /// Received shares from other participants
    pub const KEY_RECEIVED_SHARES: &[u8] = b"frost_bridge::received_shares";
}

// ============ External Interface ============

/// btc address type (shared with custody pallet)
#[derive(Clone, Encode, Decode, DecodeWithMemTracking, TypeInfo, MaxEncodedLen, Debug, PartialEq, Default)]
pub struct BtcAddress {
    pub address_type: BtcAddressType,
    pub hash: [u8; 32],
}

impl BtcAddress {
    /// check if address is zero/empty
    pub fn is_zero(&self) -> bool {
        self.hash == [0u8; 32]
    }
}

#[derive(Clone, Encode, Decode, DecodeWithMemTracking, TypeInfo, MaxEncodedLen, Debug, PartialEq, Default)]
pub enum BtcAddressType {
    #[default]
    P2PKH,
    P2SH,
    P2WPKH,
    P2WSH,
    P2TR,
}

/// interface for other pallets to interact with frost-bridge
pub trait FrostBridgeInterface<AccountId> {
    /// check if bridge is active (not halted)
    fn is_bridge_active() -> bool;

    /// get current custody address derived from group pubkey
    fn custody_address() -> Option<BtcAddress>;

    /// request signature on data, returns request id
    fn request_signature(requester: AccountId, data: Vec<u8>, deadline: u32) -> Result<u64, &'static str>;
}

/// implement interface for the pallet
impl<T: pallet::Config> FrostBridgeInterface<T::AccountId> for pallet::Pallet<T> {
    fn is_bridge_active() -> bool {
        matches!(
            pallet::CurrentBridgeState::<T>::get(),
            BridgeState::Active
        )
    }

    fn custody_address() -> Option<BtcAddress> {
        pallet::GroupPublicKey::<T>::get().map(|pubkey| {
            // derive p2tr address from group pubkey
            // todo: proper taproot address derivation
            BtcAddress {
                address_type: BtcAddressType::P2TR,
                hash: pubkey,
            }
        })
    }

    fn request_signature(requester: T::AccountId, data: Vec<u8>, deadline: u32) -> Result<u64, &'static str> {
        // check bridge active
        if !Self::is_bridge_active() {
            return Err("bridge not active");
        }

        // check we have group key
        if pallet::GroupPublicKey::<T>::get().is_none() {
            return Err("no group key");
        }

        let id = pallet::NextRequestId::<T>::get();
        pallet::NextRequestId::<T>::put(id + 1);

        let now: u32 = frame_system::Pallet::<T>::block_number().saturated_into();

        // convert Vec to BoundedVec, truncating if too large
        let tx_data: BoundedVec<u8, MaxTxDataSize> = data
            .try_into()
            .map_err(|_| "tx data too large")?;

        let request = SigningRequest {
            id,
            requester,
            tx_data,
            created_at: now,
            deadline,
            signer_fee: 0, // todo: pass fee from caller
            committed_signers: BoundedVec::new(),
            partial_sigs: BoundedVec::new(),
            final_sig: None,
            status: SigningRequestStatus::WaitingForCommitments,
        };

        pallet::SigningQueue::<T>::insert(id, request);

        Ok(id)
    }
}
