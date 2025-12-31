//! zcash orchard frost pallet
//!
//! threshold signature bridge for zcash orchard using frost with redpallas.
//! orchard uses pallas curve with randomized schnorr (reddsa) signatures.
//!
//! ## design
//!
//! orchard spendauth signatures are threshold signed via frost.
//! binding sigs are deterministic and computed locally from tx data.
//!
//! ```text
//! registration → dkg ceremony → active signing → key rotation
//!     ↓              ↓              ↓              ↓
//!   stake         generate      process        reshare
//!   bond          shares        requests       keys
//! ```

#![cfg_attr(not(feature = "std"), no_std)]

pub use pallet::*;

#[cfg(test)]
mod tests;

use codec::{Decode, DecodeWithMemTracking, Encode, MaxEncodedLen};
use frame_support::{pallet_prelude::BoundedVec, traits::Get};
use scale_info::TypeInfo;
use sp_runtime::SaturatedConversion;
use sp_std::prelude::*;

/// frost signature for zcash orchard (redpallas format)
#[derive(Clone, Encode, Decode, DecodeWithMemTracking, TypeInfo, MaxEncodedLen, Debug, PartialEq)]
pub struct OrchardSignature {
    /// r component (commitment point on pallas, compressed)
    pub r: [u8; 32],
    /// s component (scalar response)
    pub s: [u8; 32],
}

/// public key share for a signer
#[derive(Clone, Encode, Decode, DecodeWithMemTracking, TypeInfo, MaxEncodedLen, Debug, PartialEq)]
pub struct PublicKeyShare {
    pub index: u16,
    /// public key share on pallas curve
    pub share: [u8; 32],
    /// proof of secret key knowledge
    pub proof: [u8; 64],
}

/// dkg ceremony state
#[derive(Clone, Encode, Decode, DecodeWithMemTracking, TypeInfo, MaxEncodedLen, Debug, PartialEq, Default)]
pub enum DkgPhase {
    #[default]
    Idle,
    Round1 { deadline: u32 },
    Round2 { deadline: u32 },
    Round3 { deadline: u32 },
    Failed { reason: DkgFailureReason },
}

#[derive(Clone, Encode, Decode, DecodeWithMemTracking, TypeInfo, MaxEncodedLen, Debug, PartialEq)]
pub enum DkgFailureReason {
    Timeout,
    InvalidCommitment,
    InvalidShare,
    InsufficientParticipation,
}

pub type MaxTxDataSize = frame_support::traits::ConstU32<16384>;
pub type MaxPartialSigs = frame_support::traits::ConstU32<256>;
pub type MaxSignerCount = frame_support::traits::ConstU32<256>;
pub type MaxEncryptedShareSize = frame_support::traits::ConstU32<512>;

/// signing request
#[derive(Clone, Encode, Decode, DecodeWithMemTracking, TypeInfo, MaxEncodedLen, Debug, PartialEq)]
#[scale_info(skip_type_params(S, P))]
pub struct SigningRequest<AccountId, S: Get<u32>, P: Get<u32>> {
    pub id: u64,
    pub requester: AccountId,
    /// orchard action bundle to sign
    pub tx_data: BoundedVec<u8, S>,
    pub created_at: u32,
    pub deadline: u32,
    pub signer_fee: u64,
    pub committed_signers: BoundedVec<(u16, u32), P>,
    pub partial_sigs: BoundedVec<(u16, OrchardSignature), P>,
    pub final_sig: Option<OrchardSignature>,
    pub status: SigningRequestStatus,
}

#[derive(Clone, Copy, Encode, Decode, DecodeWithMemTracking, TypeInfo, MaxEncodedLen, Debug, PartialEq, Eq, Default)]
pub enum SigningRequestStatus {
    #[default]
    WaitingForCommitments,
    Signing,
    Complete,
    Failed,
}

/// signer info
#[derive(Clone, Encode, Decode, DecodeWithMemTracking, TypeInfo, MaxEncodedLen, Debug, PartialEq)]
pub struct SignerInfo<AccountId> {
    pub account: AccountId,
    pub index: u16,
    /// public key share on pallas curve
    pub public_share: [u8; 32],
    pub encryption_key: [u8; 32],
    pub joined_at: u32,
    pub status: SignerStatus,
}

#[derive(Clone, Encode, Decode, DecodeWithMemTracking, TypeInfo, MaxEncodedLen, Debug, PartialEq, Default)]
pub enum SignerStatus {
    #[default]
    Active,
    Frozen { since_block: u32, reason: FreezeReason },
    Offline { since_block: u32 },
    PendingRemoval,
}

#[derive(Clone, Encode, Decode, DecodeWithMemTracking, TypeInfo, MaxEncodedLen, Debug, PartialEq, Default)]
pub enum FreezeReason {
    #[default]
    MissedSigning,
    MissedHeartbeat,
    DkgFailure,
}

#[derive(Clone, Encode, Decode, DecodeWithMemTracking, TypeInfo, MaxEncodedLen, Debug, PartialEq, Default)]
pub struct ParticipationStats {
    pub signing_rounds_available: u32,
    pub signing_rounds_participated: u32,
    pub last_participation_block: u32,
    pub consecutive_misses: u32,
}

#[derive(Clone, Encode, Decode, DecodeWithMemTracking, TypeInfo, MaxEncodedLen, Debug, PartialEq, Default)]
pub enum BridgeState {
    #[default]
    Active,
    CircuitBroken { reason: CircuitBreakReason, since_block: u32 },
    EmergencyRecovery { initiated_at: u32, recovery_address: [u8; 43] },
}

#[derive(Clone, Encode, Decode, DecodeWithMemTracking, TypeInfo, MaxEncodedLen, Debug, PartialEq)]
pub enum CircuitBreakReason {
    InsufficientLiveness,
    RepeatedSigningFailure,
    RepeatedDkgFailure,
    ManualHalt,
}

/// zcash orchard address (43 bytes raw)
#[derive(Clone, Encode, Decode, DecodeWithMemTracking, TypeInfo, MaxEncodedLen, Debug, PartialEq)]
pub struct OrchardAddress {
    pub inner: [u8; 43],
}

impl Default for OrchardAddress {
    fn default() -> Self {
        Self { inner: [0u8; 43] }
    }
}

impl OrchardAddress {
    pub fn is_zero(&self) -> bool {
        self.inner == [0u8; 43]
    }
}

#[frame_support::pallet(dev_mode)]
pub mod pallet {
    use super::*;
    use frame_support::pallet_prelude::*;
    use frame_system::pallet_prelude::*;

    #[pallet::pallet]
    pub struct Pallet<T>(_);

    #[pallet::config]
    pub trait Config: frame_system::Config<RuntimeEvent: From<Event<Self>>> {

        #[pallet::constant]
        type MinSigners: Get<u16>;

        #[pallet::constant]
        type MaxSigners: Get<u16>;

        #[pallet::constant]
        type Threshold: Get<u16>;

        #[pallet::constant]
        type DkgTimeout: Get<u32>;

        #[pallet::constant]
        type SigningTimeout: Get<u32>;

        #[pallet::constant]
        type HeartbeatInterval: Get<u32>;

        #[pallet::constant]
        type OfflineThreshold: Get<u32>;

        #[pallet::constant]
        type CircuitBreakerThreshold: Get<u32>;
    }

    // ============ storage ============

    #[pallet::storage]
    pub type Signers<T: Config> =
        StorageMap<_, Blake2_128Concat, T::AccountId, SignerInfo<T::AccountId>>;

    #[pallet::storage]
    pub type ActiveSignerList<T: Config> =
        StorageValue<_, BoundedVec<T::AccountId, MaxSignerCount>, ValueQuery>;

    #[pallet::storage]
    pub type CurrentDkgPhase<T: Config> = StorageValue<_, DkgPhase, ValueQuery>;

    #[pallet::storage]
    pub type DkgCommitments<T: Config> = StorageMap<_, Blake2_128Concat, u16, [u8; 32]>;

    #[pallet::storage]
    pub type DkgShares<T: Config> = StorageDoubleMap<
        _,
        Blake2_128Concat,
        u16,
        Blake2_128Concat,
        u16,
        BoundedVec<u8, MaxEncryptedShareSize>,
    >;

    /// orchard group public key (pallas curve point)
    #[pallet::storage]
    pub type GroupPublicKey<T: Config> = StorageValue<_, [u8; 32]>;

    #[pallet::storage]
    pub type NextRequestId<T: Config> = StorageValue<_, u64, ValueQuery>;

    #[pallet::storage]
    pub type SigningQueue<T: Config> = StorageMap<
        _,
        Blake2_128Concat,
        u64,
        SigningRequest<T::AccountId, MaxTxDataSize, MaxPartialSigs>,
    >;

    #[pallet::storage]
    pub type SignerParticipation<T: Config> =
        StorageMap<_, Blake2_128Concat, T::AccountId, ParticipationStats, ValueQuery>;

    #[pallet::storage]
    pub type LastHeartbeat<T: Config> =
        StorageMap<_, Blake2_128Concat, T::AccountId, u32, ValueQuery>;

    #[pallet::storage]
    pub type CurrentBridgeState<T: Config> = StorageValue<_, BridgeState, ValueQuery>;

    #[pallet::storage]
    pub type ConsecutiveSigningFailures<T: Config> = StorageValue<_, u32, ValueQuery>;

    #[pallet::storage]
    pub type HeartbeatChallenge<T: Config> = StorageValue<_, [u8; 32], ValueQuery>;

    #[pallet::storage]
    pub type UsedNonces<T: Config> = StorageMap<_, Blake2_128Concat, [u8; 32], bool, ValueQuery>;

    // ============ events ============

    #[pallet::event]
    #[pallet::generate_deposit(pub(super) fn deposit_event)]
    pub enum Event<T: Config> {
        SignerRegistered {
            who: T::AccountId,
            index: u16,
            encryption_key: [u8; 32],
        },
        DkgStarted {
            round: u8,
            participants: u16,
            deadline: u32,
        },
        DkgRoundCompleted {
            round: u8,
        },
        DkgCompleted {
            group_public_key: [u8; 32],
            threshold: u16,
            signers: u16,
        },
        DkgFailed {
            reason: DkgFailureReason,
        },
        SigningRequestCreated {
            request_id: u64,
            requester: T::AccountId,
            deadline: u32,
        },
        PartialSignatureSubmitted {
            request_id: u64,
            signer_index: u16,
        },
        SigningCompleted {
            request_id: u64,
            signature: OrchardSignature,
            participants: u16,
        },
        SigningFailed {
            request_id: u64,
        },
        SignerFrozen {
            who: T::AccountId,
            reason: FreezeReason,
        },
        SignerReactivated {
            who: T::AccountId,
        },
        HeartbeatReceived {
            who: T::AccountId,
            block: u32,
        },
        CircuitBreakerTriggered {
            reason: CircuitBreakReason,
        },
        BridgeResumed,
    }

    // ============ errors ============

    #[pallet::error]
    pub enum Error<T> {
        AlreadyRegistered,
        NotRegistered,
        InsufficientSigners,
        TooManySigners,
        WrongDkgPhase,
        DkgTimeout,
        InvalidCommitment,
        InvalidShare,
        NoGroupKey,
        InvalidRequest,
        SigningTimeout,
        InvalidPartialSignature,
        AlreadySigned,
        RequestNotFound,
        NonceReused,
        BridgeHalted,
        InvalidHeartbeat,
        SignerFrozen,
        NotAuthorized,
        NotHalted,
    }

    // ============ calls ============

    #[pallet::call]
    impl<T: Config> Pallet<T> {
        /// register as a signer
        #[pallet::call_index(0)]
        #[pallet::weight(10_000)]
        pub fn register_signer(
            origin: OriginFor<T>,
            encryption_key: [u8; 32],
        ) -> DispatchResult {
            let who = ensure_signed(origin)?;
            ensure!(!Signers::<T>::contains_key(&who), Error::<T>::AlreadyRegistered);

            let mut signers = ActiveSignerList::<T>::get();
            ensure!(
                signers.len() < T::MaxSigners::get() as usize,
                Error::<T>::TooManySigners
            );

            let index = (signers.len() + 1) as u16;
            signers
                .try_push(who.clone())
                .map_err(|_| Error::<T>::TooManySigners)?;
            ActiveSignerList::<T>::put(signers);

            let info = SignerInfo {
                account: who.clone(),
                index,
                public_share: [0u8; 32],
                encryption_key,
                joined_at: frame_system::Pallet::<T>::block_number().saturated_into(),
                status: SignerStatus::default(),
            };
            Signers::<T>::insert(&who, info);

            Self::deposit_event(Event::SignerRegistered {
                who,
                index,
                encryption_key,
            });

            Self::maybe_start_dkg();
            Ok(())
        }

        /// submit dkg round 1 commitment
        #[pallet::call_index(1)]
        #[pallet::weight(10_000)]
        pub fn submit_dkg_commitment(
            origin: OriginFor<T>,
            commitment: [u8; 32],
        ) -> DispatchResult {
            let who = ensure_signed(origin)?;
            let signer = Signers::<T>::get(&who).ok_or(Error::<T>::NotRegistered)?;
            ensure!(
                matches!(CurrentDkgPhase::<T>::get(), DkgPhase::Round1 { .. }),
                Error::<T>::WrongDkgPhase
            );

            DkgCommitments::<T>::insert(signer.index, commitment);
            Self::check_dkg_round1_complete();
            Ok(())
        }

        /// submit dkg round 2 encrypted share
        #[pallet::call_index(2)]
        #[pallet::weight(10_000)]
        pub fn submit_dkg_share(
            origin: OriginFor<T>,
            to_index: u16,
            encrypted_share: BoundedVec<u8, MaxEncryptedShareSize>,
        ) -> DispatchResult {
            let who = ensure_signed(origin)?;
            let signer = Signers::<T>::get(&who).ok_or(Error::<T>::NotRegistered)?;
            ensure!(
                matches!(CurrentDkgPhase::<T>::get(), DkgPhase::Round2 { .. }),
                Error::<T>::WrongDkgPhase
            );

            DkgShares::<T>::insert(signer.index, to_index, encrypted_share);
            Self::check_dkg_round2_complete();
            Ok(())
        }

        /// submit dkg round 3 verification
        #[pallet::call_index(3)]
        #[pallet::weight(10_000)]
        pub fn submit_dkg_verification(
            origin: OriginFor<T>,
            public_share: [u8; 32],
            proof: [u8; 64],
        ) -> DispatchResult {
            let who = ensure_signed(origin)?;
            let mut signer = Signers::<T>::get(&who).ok_or(Error::<T>::NotRegistered)?;
            ensure!(
                matches!(CurrentDkgPhase::<T>::get(), DkgPhase::Round3 { .. }),
                Error::<T>::WrongDkgPhase
            );

            // todo: verify proof
            let _ = proof;

            signer.public_share = public_share;
            signer.status = SignerStatus::Active;
            Signers::<T>::insert(&who, signer);

            Self::check_dkg_complete();
            Ok(())
        }

        /// request signature on orchard action bundle
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
                signer_fee: 0,
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

        /// submit partial signature
        #[pallet::call_index(5)]
        #[pallet::weight(10_000)]
        pub fn submit_partial_signature(
            origin: OriginFor<T>,
            request_id: u64,
            signer_index: u16,
            partial_sig: OrchardSignature,
        ) -> DispatchResult {
            let who = ensure_signed(origin)?;

            let signer = Signers::<T>::get(&who).ok_or(Error::<T>::NotRegistered)?;
            ensure!(signer.index == signer_index, Error::<T>::NotRegistered);
            ensure!(
                matches!(signer.status, SignerStatus::Active),
                Error::<T>::SignerFrozen
            );

            let mut request =
                SigningQueue::<T>::get(request_id).ok_or(Error::<T>::RequestNotFound)?;

            ensure!(
                !request
                    .partial_sigs
                    .iter()
                    .any(|(idx, _)| *idx == signer_index),
                Error::<T>::AlreadySigned
            );

            let now: u32 = frame_system::Pallet::<T>::block_number().saturated_into();
            ensure!(now <= request.deadline, Error::<T>::SigningTimeout);

            let _ = request.partial_sigs.try_push((signer_index, partial_sig));

            Self::deposit_event(Event::PartialSignatureSubmitted {
                request_id,
                signer_index,
            });

            let threshold = T::Threshold::get() as usize;
            if request.partial_sigs.len() >= threshold && request.final_sig.is_none() {
                if let Some(final_sig) = Self::aggregate_signatures(&request.partial_sigs) {
                    request.final_sig = Some(final_sig.clone());
                    request.status = SigningRequestStatus::Complete;

                    for (idx, _) in request.partial_sigs.iter() {
                        if let Some(account) = Self::signer_by_index(*idx) {
                            SignerParticipation::<T>::mutate(&account, |stats| {
                                stats.signing_rounds_participated += 1;
                                stats.consecutive_misses = 0;
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

        /// submit heartbeat
        #[pallet::call_index(6)]
        #[pallet::weight(10_000)]
        pub fn submit_heartbeat(
            origin: OriginFor<T>,
            challenge_response: [u8; 64],
        ) -> DispatchResult {
            let who = ensure_signed(origin)?;
            let mut signer = Signers::<T>::get(&who).ok_or(Error::<T>::NotRegistered)?;

            let challenge = HeartbeatChallenge::<T>::get();
            ensure!(
                Self::verify_heartbeat_signature(&who, &challenge, &challenge_response),
                Error::<T>::InvalidHeartbeat
            );

            let now: u32 = frame_system::Pallet::<T>::block_number().saturated_into();
            LastHeartbeat::<T>::insert(&who, now);

            if matches!(signer.status, SignerStatus::Frozen { .. }) {
                signer.status = SignerStatus::Active;
                Signers::<T>::insert(&who, signer);

                SignerParticipation::<T>::mutate(&who, |stats| {
                    stats.consecutive_misses = 0;
                });

                Self::deposit_event(Event::SignerReactivated { who: who.clone() });
            }

            Self::deposit_event(Event::HeartbeatReceived { who, block: now });
            Ok(())
        }

        /// halt bridge (privileged)
        #[pallet::call_index(7)]
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

        /// resume bridge (privileged)
        #[pallet::call_index(8)]
        #[pallet::weight(10_000)]
        pub fn resume_bridge(origin: OriginFor<T>) -> DispatchResult {
            ensure_root(origin)?;

            ensure!(
                matches!(
                    CurrentBridgeState::<T>::get(),
                    BridgeState::CircuitBroken { .. }
                ),
                Error::<T>::NotHalted
            );

            CurrentBridgeState::<T>::put(BridgeState::Active);
            ConsecutiveSigningFailures::<T>::put(0);

            Self::deposit_event(Event::BridgeResumed);
            Ok(())
        }
    }

    // ============ hooks ============

    #[pallet::hooks]
    impl<T: Config> Hooks<BlockNumberFor<T>> for Pallet<T> {
        fn on_finalize(block_number: BlockNumberFor<T>) {
            let now: u32 = block_number.saturated_into();

            match CurrentDkgPhase::<T>::get() {
                DkgPhase::Round1 { deadline }
                | DkgPhase::Round2 { deadline }
                | DkgPhase::Round3 { deadline } => {
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

            Self::process_expired_signing_rounds(now);
            Self::check_signer_liveness(now);

            if now % T::HeartbeatInterval::get() == 0 {
                Self::rotate_heartbeat_challenge(now);
            }
        }
    }

    // ============ internal ============

    impl<T: Config> Pallet<T> {
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

        fn check_dkg_round1_complete() {
            let signers = ActiveSignerList::<T>::get();
            let n = signers.len() as u16;
            let count = (1..=n)
                .filter(|i| DkgCommitments::<T>::contains_key(i))
                .count();

            if count == signers.len() {
                let now: u32 = frame_system::Pallet::<T>::block_number().saturated_into();
                let deadline = now + T::DkgTimeout::get();

                CurrentDkgPhase::<T>::put(DkgPhase::Round2 { deadline });

                Self::deposit_event(Event::DkgRoundCompleted { round: 1 });
                Self::deposit_event(Event::DkgStarted {
                    round: 2,
                    participants: n,
                    deadline,
                });
            }
        }

        fn check_dkg_round2_complete() {
            let signers = ActiveSignerList::<T>::get();
            let n = signers.len() as u16;
            let expected = (n * (n - 1)) as usize;
            let mut count = 0;

            for from in 1..=n {
                for to in 1..=n {
                    if from != to && DkgShares::<T>::contains_key(from, to) {
                        count += 1;
                    }
                }
            }

            if count == expected {
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

        fn check_dkg_complete() {
            let signers = ActiveSignerList::<T>::get();
            let active_count = signers
                .iter()
                .filter_map(|account| Signers::<T>::get(account))
                .filter(|info| matches!(info.status, SignerStatus::Active))
                .count();

            if active_count == signers.len() {
                let group_key = Self::compute_group_public_key();
                GroupPublicKey::<T>::put(group_key);

                CurrentDkgPhase::<T>::put(DkgPhase::Idle);

                Self::deposit_event(Event::DkgCompleted {
                    group_public_key: group_key,
                    threshold: T::Threshold::get(),
                    signers: signers.len() as u16,
                });
            }
        }

        fn compute_group_public_key() -> [u8; 32] {
            // in real impl: sum of public key shares on pallas curve
            let signers = ActiveSignerList::<T>::get();
            let mut result = [0u8; 32];

            for account in signers.iter() {
                if let Some(info) = Signers::<T>::get(account) {
                    for (i, byte) in info.public_share.iter().enumerate() {
                        result[i] ^= byte;
                    }
                }
            }

            result
        }

        fn aggregate_signatures(
            partials: &BoundedVec<(u16, OrchardSignature), MaxPartialSigs>,
        ) -> Option<OrchardSignature> {
            if partials.is_empty() {
                return None;
            }

            // in real impl: lagrange interpolation on pallas curve
            let mut r = [0u8; 32];
            let mut s = [0u8; 32];

            for (_, sig) in partials {
                for i in 0..32 {
                    r[i] ^= sig.r[i];
                    s[i] ^= sig.s[i];
                }
            }

            Some(OrchardSignature { r, s })
        }

        fn verify_heartbeat_signature(
            _who: &T::AccountId,
            _challenge: &[u8; 32],
            _response: &[u8; 64],
        ) -> bool {
            true
        }

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

        fn freeze_signer(who: &T::AccountId, reason: FreezeReason, now: u32) {
            if let Some(mut signer) = Signers::<T>::get(who) {
                signer.status = SignerStatus::Frozen {
                    since_block: now,
                    reason: reason.clone(),
                };
                Signers::<T>::insert(who, signer);

                Self::deposit_event(Event::SignerFrozen {
                    who: who.clone(),
                    reason,
                });
            }
        }

        fn check_signer_liveness(now: u32) {
            let signers = ActiveSignerList::<T>::get();
            let offline_threshold = T::OfflineThreshold::get();
            let mut offline_count = 0u32;

            for account in signers.iter() {
                let signer = match Signers::<T>::get(account) {
                    Some(s) => s,
                    None => continue,
                };

                if !matches!(signer.status, SignerStatus::Active) {
                    offline_count += 1;
                    continue;
                }

                let last = LastHeartbeat::<T>::get(account);
                let since = now.saturating_sub(last);

                if since > offline_threshold {
                    offline_count += 1;

                    if since > offline_threshold * 2 {
                        Self::freeze_signer(account, FreezeReason::MissedHeartbeat, now);
                    }
                }
            }

            let total = signers.len() as u32;
            let threshold = T::Threshold::get() as u32;
            let active = total.saturating_sub(offline_count);

            if active < threshold {
                Self::trigger_circuit_breaker(CircuitBreakReason::InsufficientLiveness, now);
            }
        }

        fn trigger_circuit_breaker(reason: CircuitBreakReason, now: u32) {
            if matches!(CurrentBridgeState::<T>::get(), BridgeState::Active) {
                CurrentBridgeState::<T>::put(BridgeState::CircuitBroken {
                    reason: reason.clone(),
                    since_block: now,
                });

                Self::deposit_event(Event::CircuitBreakerTriggered { reason });
            }
        }

        fn process_expired_signing_rounds(now: u32) {
            let mut failures = 0u32;

            for (request_id, request) in SigningQueue::<T>::iter() {
                if now > request.deadline && request.final_sig.is_none() {
                    failures += 1;

                    let signers = ActiveSignerList::<T>::get();
                    for account in signers.iter() {
                        if let Some(info) = Signers::<T>::get(account) {
                            let participated = request
                                .partial_sigs
                                .iter()
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
                    SigningQueue::<T>::remove(request_id);
                }
            }

            if failures > 0 {
                let total = ConsecutiveSigningFailures::<T>::mutate(|n| {
                    *n = n.saturating_add(failures);
                    *n
                });

                if total >= T::CircuitBreakerThreshold::get() {
                    Self::trigger_circuit_breaker(CircuitBreakReason::RepeatedSigningFailure, now);
                }
            }
        }

        fn rotate_heartbeat_challenge(now: u32) {
            let parent = frame_system::Pallet::<T>::parent_hash();
            let mut challenge = [0u8; 32];
            let hash_bytes = parent.as_ref();
            for (i, byte) in hash_bytes.iter().take(32).enumerate() {
                challenge[i] = *byte;
            }
            let now_bytes = now.to_le_bytes();
            for (i, byte) in now_bytes.iter().enumerate() {
                challenge[i] ^= byte;
            }

            HeartbeatChallenge::<T>::put(challenge);
        }
    }
}

// ============ interface ============

/// interface for other pallets
pub trait OrchardBridgeInterface<AccountId> {
    fn is_bridge_active() -> bool;
    fn custody_address() -> Option<OrchardAddress>;
    fn request_signature(
        requester: AccountId,
        data: Vec<u8>,
        deadline: u32,
    ) -> Result<u64, &'static str>;
}

impl<T: pallet::Config> OrchardBridgeInterface<T::AccountId> for pallet::Pallet<T> {
    fn is_bridge_active() -> bool {
        matches!(pallet::CurrentBridgeState::<T>::get(), BridgeState::Active)
    }

    fn custody_address() -> Option<OrchardAddress> {
        pallet::GroupPublicKey::<T>::get().map(|pubkey| {
            // derive orchard address from group pubkey
            // in real impl: use full viewing key derivation
            let mut addr = [0u8; 43];
            addr[..32].copy_from_slice(&pubkey);
            OrchardAddress { inner: addr }
        })
    }

    fn request_signature(
        requester: T::AccountId,
        data: Vec<u8>,
        deadline: u32,
    ) -> Result<u64, &'static str> {
        if !Self::is_bridge_active() {
            return Err("bridge not active");
        }

        if pallet::GroupPublicKey::<T>::get().is_none() {
            return Err("no group key");
        }

        let id = pallet::NextRequestId::<T>::get();
        pallet::NextRequestId::<T>::put(id + 1);

        let now: u32 = frame_system::Pallet::<T>::block_number().saturated_into();

        let tx_data: BoundedVec<u8, MaxTxDataSize> =
            data.try_into().map_err(|_| "tx data too large")?;

        let request = SigningRequest {
            id,
            requester,
            tx_data,
            created_at: now,
            deadline,
            signer_fee: 0,
            committed_signers: BoundedVec::new(),
            partial_sigs: BoundedVec::new(),
            final_sig: None,
            status: SigningRequestStatus::WaitingForCommitments,
        };

        pallet::SigningQueue::<T>::insert(id, request);
        Ok(id)
    }
}
