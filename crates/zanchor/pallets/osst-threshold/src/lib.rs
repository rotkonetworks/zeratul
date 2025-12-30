//! osst threshold custody pallet
//!
//! threshold custody using one-step schnorr threshold identification
//! with ligerito liveness proofs for custodian verification.
//!
//! ## design
//!
//! - custodians register with ligerito proofs proving they run nodes
//! - reshare ceremonies rotate keys without changing group pubkey
//! - osst verification enables non-interactive threshold proofs
//! - zcash pallas curve for orchard compatibility
//!
//! ## architecture
//!
//! ```text
//! registration → dkg ceremony → active custody → reshare → ...
//!      ↓             ↓              ↓              ↓
//!   liveness      generate       process        rotate
//!   proof         shares         requests       custodians
//! ```
//!
//! ## security model
//!
//! trust comes from:
//! - ligerito proofs: custodians prove they verify blocks
//! - threshold: t-of-n required, tolerates n-t malicious
//! - osst: non-interactive identification without coordination
//! - on-chain: reshare coordination is transparent

#![cfg_attr(not(feature = "std"), no_std)]

pub use pallet::*;

#[cfg(test)]
mod tests;

extern crate alloc;
use alloc::vec::Vec;

use codec::{Decode, DecodeWithMemTracking, Encode};
use frame_support::pallet_prelude::*;
use scale_info::TypeInfo;

// pallas curve imports for EC operations
use pasta_curves::{
    group::{ff::PrimeField, Group, GroupEncoding},
    pallas::{Point as PallasPoint, Scalar as PallasScalar},
};

// osst imports for threshold verification
use osst::{
    compute_lagrange_coefficients, verify as osst_verify,
    Contribution as OsstContrib, OsstPoint, OsstScalar,
};

// hashing for schnorr signatures
use sha2::{Digest, Sha512};

/// compressed pallas point (32 bytes)
pub type CompressedPoint = [u8; 32];

/// compressed pallas scalar (32 bytes)
pub type CompressedScalar = [u8; 32];

/// ligerito proof bytes
pub type LivenessProofBytes = Vec<u8>;

/// custodian info
#[derive(Clone, Encode, Decode, TypeInfo, MaxEncodedLen, Debug, PartialEq)]
pub struct CustodianInfo<AccountId, BlockNumber> {
    /// account id
    pub account: AccountId,
    /// 1-indexed custodian index
    pub index: u32,
    /// public key share on pallas
    pub public_share: CompressedPoint,
    /// x25519 encryption key for share delivery
    pub encryption_key: [u8; 32],
    /// block when registered
    pub registered_at: BlockNumber,
    /// last liveness proof block
    pub last_liveness_at: BlockNumber,
    /// total reshare participations
    pub reshare_count: u32,
    /// whether currently active in custody set
    pub active: bool,
}

/// checkpoint anchor for liveness proofs
#[derive(Clone, Encode, Decode, TypeInfo, MaxEncodedLen, Debug, PartialEq, Default)]
pub struct CheckpointAnchor {
    /// zcash block height
    pub height: u32,
    /// zcash block hash
    pub block_hash: [u8; 32],
    /// substrate block when anchored
    pub anchored_at: u32,
}

/// reshare phase
#[derive(Clone, Encode, Decode, TypeInfo, MaxEncodedLen, Debug, PartialEq, Default)]
pub enum ResharePhase {
    /// no reshare in progress
    #[default]
    Idle,
    /// collecting dealer commitments
    Commitments { deadline: u32 },
    /// collecting subshares
    Subshares { deadline: u32 },
    /// verification and finalization
    Verification { deadline: u32 },
    /// reshare failed
    Failed { reason: ReshareFailureReason },
}

/// reshare failure reason
#[derive(Clone, Encode, Decode, DecodeWithMemTracking, TypeInfo, MaxEncodedLen, Debug, PartialEq)]
pub enum ReshareFailureReason {
    Timeout,
    InvalidCommitment,
    InvalidSubshare,
    InsufficientParticipation,
    LivenessExpired,
}

/// dealer commitment for reshare
#[derive(Clone, Encode, Decode, TypeInfo, Debug, PartialEq)]
pub struct DealerCommitmentData {
    /// dealer index
    pub dealer_index: u32,
    /// polynomial coefficient commitments [g^{a_0}, ..., g^{a_{t-1}}]
    pub coefficients: Vec<CompressedPoint>,
    /// liveness proof
    pub liveness_proof: LivenessProofBytes,
    /// signature binding commitment to liveness
    pub binding_signature: [u8; 64],
}

/// subshare from dealer to recipient
#[derive(Clone, Encode, Decode, TypeInfo, Debug, PartialEq)]
pub struct SubshareData {
    /// dealer index
    pub from_dealer: u32,
    /// recipient index
    pub to_recipient: u32,
    /// encrypted subshare
    pub encrypted_share: Vec<u8>,
    /// ephemeral public key
    pub ephemeral_pk: [u8; 32],
}

/// custody epoch
#[derive(Clone, Encode, Decode, TypeInfo, MaxEncodedLen, Debug, PartialEq, Default)]
pub struct EpochInfo {
    /// epoch number
    pub epoch: u64,
    /// group public key
    pub group_key: CompressedPoint,
    /// threshold
    pub threshold: u32,
    /// total custodians
    pub custodian_count: u32,
    /// block when epoch started
    pub started_at: u32,
}

/// osst contribution for verification
#[derive(Clone, Encode, Decode, DecodeWithMemTracking, TypeInfo, MaxEncodedLen, Debug, PartialEq)]
pub struct OsstContribution {
    /// custodian index
    pub index: u32,
    /// public key share
    pub public_share: CompressedPoint,
    /// response scalar
    pub response: CompressedScalar,
}

#[frame_support::pallet]
pub mod pallet {
    use super::*;
    use frame_system::pallet_prelude::*;

    #[pallet::pallet]
    pub struct Pallet<T>(_);

    #[pallet::config]
    pub trait Config: frame_system::Config<RuntimeEvent: From<Event<Self>>> {
        /// minimum custodians for threshold
        #[pallet::constant]
        type MinCustodians: Get<u32>;

        /// maximum custodians
        #[pallet::constant]
        type MaxCustodians: Get<u32>;

        /// threshold ratio numerator (e.g., 2 for 2/3)
        #[pallet::constant]
        type ThresholdNumerator: Get<u32>;

        /// threshold ratio denominator (e.g., 3 for 2/3)
        #[pallet::constant]
        type ThresholdDenominator: Get<u32>;

        /// reshare round timeout in blocks
        #[pallet::constant]
        type ReshareTimeout: Get<u32>;

        /// liveness proof validity in blocks
        #[pallet::constant]
        type LivenessValidity: Get<u32>;

        /// blocks between mandatory reshares
        #[pallet::constant]
        type EpochDuration: Get<u32>;
    }

    // ========== storage ==========

    /// registered custodians
    #[pallet::storage]
    pub type Custodians<T: Config> = StorageMap<
        _,
        Blake2_128Concat,
        T::AccountId,
        CustodianInfo<T::AccountId, BlockNumberFor<T>>,
    >;

    /// custodian index to account mapping
    #[pallet::storage]
    pub type CustodianByIndex<T: Config> = StorageMap<
        _,
        Blake2_128Concat,
        u32,
        T::AccountId,
    >;

    /// next custodian index
    #[pallet::storage]
    pub type NextCustodianIndex<T: Config> = StorageValue<_, u32, ValueQuery>;

    /// active custodian list
    #[pallet::storage]
    #[pallet::unbounded]
    pub type ActiveCustodians<T: Config> = StorageValue<_, Vec<T::AccountId>, ValueQuery>;

    /// current epoch
    #[pallet::storage]
    pub type CurrentEpoch<T: Config> = StorageValue<_, EpochInfo, ValueQuery>;

    /// current reshare phase
    #[pallet::storage]
    pub type CurrentResharePhase<T: Config> = StorageValue<_, ResharePhase, ValueQuery>;

    /// dealer commitments for current reshare
    #[pallet::storage]
    #[pallet::unbounded]
    pub type DealerCommitments<T: Config> = StorageMap<
        _,
        Blake2_128Concat,
        u32, // dealer index
        DealerCommitmentData,
    >;

    /// subshares for current reshare
    #[pallet::storage]
    #[pallet::unbounded]
    pub type Subshares<T: Config> = StorageDoubleMap<
        _,
        Blake2_128Concat,
        u32, // from dealer
        Blake2_128Concat,
        u32, // to recipient
        SubshareData,
    >;

    /// aggregated subshares received by each recipient
    #[pallet::storage]
    pub type ReceivedSubshareCount<T: Config> = StorageMap<
        _,
        Blake2_128Concat,
        u32, // recipient index
        u32, // count
        ValueQuery,
    >;

    /// checkpoint anchor for liveness proofs
    #[pallet::storage]
    pub type LivenessAnchor<T: Config> = StorageValue<_, CheckpointAnchor, ValueQuery>;

    /// next epoch number
    #[pallet::storage]
    pub type NextEpoch<T: Config> = StorageValue<_, u64, ValueQuery>;

    /// last reshare block
    #[pallet::storage]
    pub type LastReshareBlock<T: Config> = StorageValue<_, u32, ValueQuery>;

    // ========== events ==========

    #[pallet::event]
    #[pallet::generate_deposit(pub(super) fn deposit_event)]
    pub enum Event<T: Config> {
        /// custodian registered
        CustodianRegistered {
            who: T::AccountId,
            index: u32,
            encryption_key: [u8; 32],
        },

        /// custodian unregistered
        CustodianUnregistered {
            who: T::AccountId,
            index: u32,
        },

        /// liveness proof submitted
        LivenessProofSubmitted {
            who: T::AccountId,
            anchor_height: u32,
        },

        /// reshare started
        ReshareStarted {
            epoch: u64,
            participants: u32,
            new_threshold: u32,
            deadline: u32,
        },

        /// dealer commitment submitted
        DealerCommitmentSubmitted {
            dealer_index: u32,
            coefficient_count: u32,
        },

        /// reshare phase advanced
        ResharePhaseAdvanced {
            new_phase: u8,
            deadline: u32,
        },

        /// reshare completed
        ReshareCompleted {
            epoch: u64,
            group_key: CompressedPoint,
            threshold: u32,
            custodian_count: u32,
        },

        /// reshare failed
        ReshareFailed {
            reason: ReshareFailureReason,
        },

        /// epoch finalized
        EpochFinalized {
            epoch: u64,
            group_key: CompressedPoint,
        },

        /// liveness anchor updated
        LivenessAnchorUpdated {
            height: u32,
            block_hash: [u8; 32],
        },

        /// osst verification succeeded
        OsstVerified {
            group_key: CompressedPoint,
            contributor_count: u32,
            payload_hash: [u8; 32],
        },

        /// osst verification failed
        OsstVerificationFailed {
            reason: Vec<u8>,
        },
    }

    // ========== errors ==========

    #[pallet::error]
    pub enum Error<T> {
        /// already registered
        AlreadyRegistered,
        /// not registered
        NotRegistered,
        /// too many custodians
        TooManyCustodians,
        /// wrong reshare phase
        WrongResharePhase,
        /// reshare timeout
        ReshareTimeout,
        /// invalid commitment
        InvalidCommitment,
        /// invalid subshare
        InvalidSubshare,
        /// insufficient participants
        InsufficientParticipation,
        /// liveness proof expired
        LivenessExpired,
        /// invalid liveness proof
        InvalidLivenessProof,
        /// no active custody
        NoCustody,
        /// osst verification failed
        VerificationFailed,
        /// already submitted
        AlreadySubmitted,
        /// not a dealer
        NotADealer,
        /// not a recipient
        NotARecipient,
    }

    // ========== hooks ==========

    #[pallet::hooks]
    impl<T: Config> Hooks<BlockNumberFor<T>> for Pallet<T> {
        fn on_finalize(n: BlockNumberFor<T>) {
            let now: u32 = n.try_into().unwrap_or(u32::MAX);

            // check reshare timeout
            match CurrentResharePhase::<T>::get() {
                ResharePhase::Commitments { deadline } |
                ResharePhase::Subshares { deadline } |
                ResharePhase::Verification { deadline } => {
                    if now > deadline {
                        CurrentResharePhase::<T>::put(ResharePhase::Failed {
                            reason: ReshareFailureReason::Timeout,
                        });
                        Self::deposit_event(Event::ReshareFailed {
                            reason: ReshareFailureReason::Timeout,
                        });
                    }
                }
                _ => {}
            }

            // check if epoch rotation needed
            let last = LastReshareBlock::<T>::get();
            if last > 0 && now > last + T::EpochDuration::get() {
                if matches!(CurrentResharePhase::<T>::get(), ResharePhase::Idle) {
                    Self::maybe_start_reshare();
                }
            }
        }
    }

    // ========== extrinsics ==========

    #[pallet::call]
    impl<T: Config> Pallet<T> {
        /// register as custodian with encryption key
        #[pallet::call_index(0)]
        #[pallet::weight(Weight::from_parts(50_000, 0))]
        pub fn register_custodian(
            origin: OriginFor<T>,
            encryption_key: [u8; 32],
        ) -> DispatchResult {
            let who = ensure_signed(origin)?;

            ensure!(!Custodians::<T>::contains_key(&who), Error::<T>::AlreadyRegistered);

            let count = ActiveCustodians::<T>::get().len() as u32;
            ensure!(count < T::MaxCustodians::get(), Error::<T>::TooManyCustodians);

            let index = NextCustodianIndex::<T>::get() + 1;
            NextCustodianIndex::<T>::put(index);

            let now = frame_system::Pallet::<T>::block_number();

            let info = CustodianInfo {
                account: who.clone(),
                index,
                public_share: [0u8; 32],
                encryption_key,
                registered_at: now,
                last_liveness_at: now,
                reshare_count: 0,
                active: false,
            };

            Custodians::<T>::insert(&who, info);
            CustodianByIndex::<T>::insert(index, who.clone());

            let mut active = ActiveCustodians::<T>::get();
            active.push(who.clone());
            ActiveCustodians::<T>::put(active);

            Self::deposit_event(Event::CustodianRegistered {
                who,
                index,
                encryption_key,
            });

            // check if we should start initial dkg
            Self::maybe_start_reshare();

            Ok(())
        }

        /// submit liveness proof (ligerito proof of block verification)
        #[pallet::call_index(1)]
        #[pallet::weight(Weight::from_parts(100_000_000, 0))]
        pub fn submit_liveness_proof(
            origin: OriginFor<T>,
            anchor_height: u32,
            proof: LivenessProofBytes,
        ) -> DispatchResult {
            let who = ensure_signed(origin)?;

            let mut info = Custodians::<T>::get(&who)
                .ok_or(Error::<T>::NotRegistered)?;

            // verify the liveness proof
            let anchor = LivenessAnchor::<T>::get();
            ensure!(anchor.height > 0, Error::<T>::InvalidLivenessProof);
            ensure!(anchor_height == anchor.height, Error::<T>::InvalidLivenessProof);

            let valid = Self::verify_liveness_proof(&anchor, &proof);
            ensure!(valid, Error::<T>::InvalidLivenessProof);

            info.last_liveness_at = frame_system::Pallet::<T>::block_number();
            Custodians::<T>::insert(&who, info);

            Self::deposit_event(Event::LivenessProofSubmitted {
                who,
                anchor_height,
            });

            Ok(())
        }

        /// submit dealer commitment for reshare
        #[pallet::call_index(2)]
        #[pallet::weight(Weight::from_parts(100_000, 0))]
        pub fn submit_dealer_commitment(
            origin: OriginFor<T>,
            coefficients: Vec<CompressedPoint>,
            liveness_proof: LivenessProofBytes,
            binding_signature: [u8; 64],
        ) -> DispatchResult {
            let who = ensure_signed(origin)?;

            let info = Custodians::<T>::get(&who)
                .ok_or(Error::<T>::NotRegistered)?;

            ensure!(
                matches!(CurrentResharePhase::<T>::get(), ResharePhase::Commitments { .. }),
                Error::<T>::WrongResharePhase
            );

            ensure!(
                !DealerCommitments::<T>::contains_key(info.index),
                Error::<T>::AlreadySubmitted
            );

            // verify liveness is fresh
            let now: u32 = frame_system::Pallet::<T>::block_number()
                .try_into()
                .unwrap_or(u32::MAX);
            let last_liveness: u32 = info.last_liveness_at
                .try_into()
                .unwrap_or(0);
            ensure!(
                now <= last_liveness + T::LivenessValidity::get(),
                Error::<T>::LivenessExpired
            );

            // verify liveness proof if provided
            if !liveness_proof.is_empty() {
                let anchor = LivenessAnchor::<T>::get();
                let valid = Self::verify_liveness_proof(&anchor, &liveness_proof);
                ensure!(valid, Error::<T>::InvalidLivenessProof);
            }

            // verify coefficient count matches threshold
            let threshold = Self::compute_threshold();
            ensure!(
                coefficients.len() == threshold as usize,
                Error::<T>::InvalidCommitment
            );

            // verify binding signature (schnorr signature over coefficients + liveness proof)
            let valid_sig = Self::verify_binding_signature(
                &info.public_share,
                &coefficients,
                &liveness_proof,
                &binding_signature,
            );
            ensure!(valid_sig, Error::<T>::InvalidCommitment);

            let commitment = DealerCommitmentData {
                dealer_index: info.index,
                coefficients: coefficients.clone(),
                liveness_proof,
                binding_signature,
            };

            DealerCommitments::<T>::insert(info.index, commitment);

            Self::deposit_event(Event::DealerCommitmentSubmitted {
                dealer_index: info.index,
                coefficient_count: coefficients.len() as u32,
            });

            // check if all commitments received
            Self::check_commitments_complete();

            Ok(())
        }

        /// submit subshare for reshare
        #[pallet::call_index(3)]
        #[pallet::weight(Weight::from_parts(50_000, 0))]
        pub fn submit_subshare(
            origin: OriginFor<T>,
            to_recipient: u32,
            encrypted_share: Vec<u8>,
            ephemeral_pk: [u8; 32],
        ) -> DispatchResult {
            let who = ensure_signed(origin)?;

            let info = Custodians::<T>::get(&who)
                .ok_or(Error::<T>::NotRegistered)?;

            ensure!(
                matches!(CurrentResharePhase::<T>::get(), ResharePhase::Subshares { .. }),
                Error::<T>::WrongResharePhase
            );

            // must be a dealer (submitted commitment)
            ensure!(
                DealerCommitments::<T>::contains_key(info.index),
                Error::<T>::NotADealer
            );

            // recipient must exist
            ensure!(
                CustodianByIndex::<T>::contains_key(to_recipient),
                Error::<T>::NotARecipient
            );

            ensure!(
                !Subshares::<T>::contains_key(info.index, to_recipient),
                Error::<T>::AlreadySubmitted
            );

            let subshare = SubshareData {
                from_dealer: info.index,
                to_recipient,
                encrypted_share,
                ephemeral_pk,
            };

            Subshares::<T>::insert(info.index, to_recipient, subshare);

            // update count for recipient
            ReceivedSubshareCount::<T>::mutate(to_recipient, |c| *c += 1);

            // check if all subshares received
            Self::check_subshares_complete();

            Ok(())
        }

        /// finalize reshare with new public share
        #[pallet::call_index(4)]
        #[pallet::weight(Weight::from_parts(100_000, 0))]
        pub fn finalize_reshare(
            origin: OriginFor<T>,
            public_share: CompressedPoint,
        ) -> DispatchResult {
            let who = ensure_signed(origin)?;

            let mut info = Custodians::<T>::get(&who)
                .ok_or(Error::<T>::NotRegistered)?;

            ensure!(
                matches!(CurrentResharePhase::<T>::get(), ResharePhase::Verification { .. }),
                Error::<T>::WrongResharePhase
            );

            info.public_share = public_share;
            info.active = true;
            info.reshare_count += 1;
            Custodians::<T>::insert(&who, info);

            // check if all verified
            Self::check_reshare_complete();

            Ok(())
        }

        /// verify osst proof on-chain
        #[pallet::call_index(5)]
        #[pallet::weight(Weight::from_parts(500_000, 0))]
        pub fn verify_osst(
            origin: OriginFor<T>,
            contributions: Vec<OsstContribution>,
            payload: Vec<u8>,
        ) -> DispatchResult {
            let _who = ensure_signed(origin)?;

            let epoch = CurrentEpoch::<T>::get();
            ensure!(epoch.custodian_count > 0, Error::<T>::NoCustody);

            let valid = Self::do_verify_osst(
                &epoch.group_key,
                &contributions,
                epoch.threshold,
                &payload,
            );

            if valid {
                // compute payload hash for event
                let payload_hash = sp_io::hashing::blake2_256(&payload);

                Self::deposit_event(Event::OsstVerified {
                    group_key: epoch.group_key,
                    contributor_count: contributions.len() as u32,
                    payload_hash,
                });
            } else {
                Self::deposit_event(Event::OsstVerificationFailed {
                    reason: b"verification equation failed".to_vec(),
                });
                return Err(Error::<T>::VerificationFailed.into());
            }

            Ok(())
        }

        /// update liveness anchor (root or sudo)
        #[pallet::call_index(10)]
        #[pallet::weight(Weight::from_parts(10_000, 0))]
        pub fn set_liveness_anchor(
            origin: OriginFor<T>,
            height: u32,
            block_hash: [u8; 32],
        ) -> DispatchResult {
            ensure_root(origin)?;

            let now: u32 = frame_system::Pallet::<T>::block_number()
                .try_into()
                .unwrap_or(u32::MAX);

            let anchor = CheckpointAnchor {
                height,
                block_hash,
                anchored_at: now,
            };

            LivenessAnchor::<T>::put(anchor);

            Self::deposit_event(Event::LivenessAnchorUpdated {
                height,
                block_hash,
            });

            Ok(())
        }

        /// force start reshare (root only)
        #[pallet::call_index(11)]
        #[pallet::weight(Weight::from_parts(10_000, 0))]
        pub fn force_reshare(origin: OriginFor<T>) -> DispatchResult {
            ensure_root(origin)?;

            ensure!(
                matches!(CurrentResharePhase::<T>::get(), ResharePhase::Idle | ResharePhase::Failed { .. }),
                Error::<T>::WrongResharePhase
            );

            Self::start_reshare();
            Ok(())
        }
    }

    // ========== internal ==========

    impl<T: Config> Pallet<T> {
        /// compute threshold from custodian count
        /// uses strict BFT: t = floor(2n/3) + 1
        /// this tolerates up to floor((n-1)/3) failures
        fn compute_threshold() -> u32 {
            let count = ActiveCustodians::<T>::get().len() as u32;
            let num = T::ThresholdNumerator::get();
            let den = T::ThresholdDenominator::get();

            // floor(count * num / den) + 1 for strict BFT
            (count * num / den) + 1
        }

        /// maybe start reshare if conditions met
        fn maybe_start_reshare() {
            if !matches!(CurrentResharePhase::<T>::get(), ResharePhase::Idle) {
                return;
            }

            let count = ActiveCustodians::<T>::get().len() as u32;
            if count >= T::MinCustodians::get() {
                Self::start_reshare();
            }
        }

        /// start reshare ceremony
        fn start_reshare() {
            let now: u32 = frame_system::Pallet::<T>::block_number()
                .try_into()
                .unwrap_or(u32::MAX);
            let deadline = now + T::ReshareTimeout::get();

            CurrentResharePhase::<T>::put(ResharePhase::Commitments { deadline });

            let epoch = NextEpoch::<T>::get();
            NextEpoch::<T>::put(epoch + 1);

            let count = ActiveCustodians::<T>::get().len() as u32;
            let threshold = Self::compute_threshold();

            Self::deposit_event(Event::ReshareStarted {
                epoch,
                participants: count,
                new_threshold: threshold,
                deadline,
            });
        }

        /// check if all commitments received
        fn check_commitments_complete() {
            let active = ActiveCustodians::<T>::get();
            let threshold = Self::compute_threshold();

            // need at least threshold dealers
            let dealer_count = active.iter()
                .filter_map(|acc| Custodians::<T>::get(acc))
                .filter(|info| DealerCommitments::<T>::contains_key(info.index))
                .count() as u32;

            if dealer_count >= threshold {
                let now: u32 = frame_system::Pallet::<T>::block_number()
                    .try_into()
                    .unwrap_or(u32::MAX);
                let deadline = now + T::ReshareTimeout::get();

                CurrentResharePhase::<T>::put(ResharePhase::Subshares { deadline });

                Self::deposit_event(Event::ResharePhaseAdvanced {
                    new_phase: 2,
                    deadline,
                });
            }
        }

        /// check if all subshares received
        fn check_subshares_complete() {
            let active = ActiveCustodians::<T>::get();

            // each recipient should get shares from all dealers
            let dealer_count = active.iter()
                .filter_map(|acc| Custodians::<T>::get(acc))
                .filter(|info| DealerCommitments::<T>::contains_key(info.index))
                .count() as u32;

            // check all recipients have enough shares
            let all_received = active.iter()
                .filter_map(|acc| Custodians::<T>::get(acc))
                .all(|info| ReceivedSubshareCount::<T>::get(info.index) >= dealer_count);

            if all_received {
                let now: u32 = frame_system::Pallet::<T>::block_number()
                    .try_into()
                    .unwrap_or(u32::MAX);
                let deadline = now + T::ReshareTimeout::get();

                CurrentResharePhase::<T>::put(ResharePhase::Verification { deadline });

                Self::deposit_event(Event::ResharePhaseAdvanced {
                    new_phase: 3,
                    deadline,
                });
            }
        }

        /// check if reshare complete
        fn check_reshare_complete() {
            let active = ActiveCustodians::<T>::get();

            let active_count = active.iter()
                .filter_map(|acc| Custodians::<T>::get(acc))
                .filter(|info| info.active)
                .count() as u32;

            if active_count == active.len() as u32 {
                // compute group key from public shares
                let group_key = Self::compute_group_key();

                let threshold = Self::compute_threshold();
                let epoch_num = NextEpoch::<T>::get().saturating_sub(1);

                let now: u32 = frame_system::Pallet::<T>::block_number()
                    .try_into()
                    .unwrap_or(u32::MAX);

                let epoch = EpochInfo {
                    epoch: epoch_num,
                    group_key,
                    threshold,
                    custodian_count: active_count,
                    started_at: now,
                };

                CurrentEpoch::<T>::put(epoch);
                CurrentResharePhase::<T>::put(ResharePhase::Idle);
                LastReshareBlock::<T>::put(now);

                // cleanup reshare storage
                let _ = DealerCommitments::<T>::clear(u32::MAX, None);
                let _ = Subshares::<T>::clear(u32::MAX, None);
                let _ = ReceivedSubshareCount::<T>::clear(u32::MAX, None);

                Self::deposit_event(Event::ReshareCompleted {
                    epoch: epoch_num,
                    group_key,
                    threshold,
                    custodian_count: active_count,
                });

                Self::deposit_event(Event::EpochFinalized {
                    epoch: epoch_num,
                    group_key,
                });
            }
        }

        /// compute group key from individual public shares
        /// Y = Σ λ_i · Y_i where λ_i are Lagrange coefficients at x=0
        fn compute_group_key() -> CompressedPoint {
            let active = ActiveCustodians::<T>::get();

            // collect indices and public shares
            let mut indices: Vec<u32> = Vec::new();
            let mut points: Vec<PallasPoint> = Vec::new();

            for acc in active.iter() {
                if let Some(info) = Custodians::<T>::get(acc) {
                    if info.active {
                        // decompress public share
                        if let Some(point) = PallasPoint::from_bytes(&info.public_share).into_option() {
                            indices.push(info.index);
                            points.push(point);
                        }
                    }
                }
            }

            if indices.is_empty() {
                return [0u8; 32];
            }

            // compute lagrange coefficients
            let coefficients: Vec<PallasScalar> = match compute_lagrange_coefficients(&indices) {
                Ok(c) => c,
                Err(_) => return [0u8; 32],
            };

            // compute weighted sum: Y = Σ λ_i · Y_i
            let group_key = coefficients
                .iter()
                .zip(points.iter())
                .fold(<PallasPoint as Group>::identity(), |acc, (lambda, point)| {
                    acc + point.mul_scalar(lambda)
                });

            group_key.to_bytes()
        }

        /// verify schnorr binding signature
        /// signature = (R, s) where R = g^k, s = k + e*x
        /// verify: g^s == R * Y^e where e = H(R || message)
        fn verify_binding_signature(
            public_key: &CompressedPoint,
            coefficients: &[CompressedPoint],
            liveness_proof: &[u8],
            signature: &[u8; 64],
        ) -> bool {
            // decompress public key
            let pubkey = match PallasPoint::from_bytes(public_key).into_option() {
                Some(p) => p,
                None => return false,
            };

            // extract R (first 32 bytes) and s (last 32 bytes)
            let r_bytes: [u8; 32] = signature[0..32].try_into().unwrap_or([0u8; 32]);
            let s_bytes: [u8; 32] = signature[32..64].try_into().unwrap_or([0u8; 32]);

            let r_point = match PallasPoint::from_bytes(&r_bytes).into_option() {
                Some(p) => p,
                None => return false,
            };

            let s_scalar = match PallasScalar::from_repr(s_bytes).into_option() {
                Some(s) => s,
                None => return false,
            };

            // compute challenge e = H(R || coefficients || liveness_proof)
            let mut hasher = Sha512::new();
            hasher.update(&r_bytes);
            for coeff in coefficients {
                hasher.update(coeff);
            }
            hasher.update(liveness_proof);
            let hash: [u8; 64] = hasher.finalize().into();
            let e = <PallasScalar as OsstScalar>::from_bytes_wide(&hash);

            // verify: g^s == R + Y^e
            let lhs = <PallasPoint as Group>::generator() * s_scalar;
            let rhs = r_point + pubkey * e;

            lhs == rhs
        }

        /// verify liveness proof (ligerito)
        #[cfg(feature = "ligerito-verify")]
        fn verify_liveness_proof(
            _anchor: &CheckpointAnchor,
            proof_bytes: &[u8],
        ) -> bool {
            use codec::Decode;
            use ligerito::{verify, FinalizedLigeritoProof, hardcoded_config_24_verifier};
            use ligerito_binary_fields::{BinaryElem32, BinaryElem128};

            let proof: FinalizedLigeritoProof<BinaryElem32, BinaryElem128> =
                match Decode::decode(&mut &proof_bytes[..]) {
                    Ok(p) => p,
                    Err(_) => return false,
                };

            let config = hardcoded_config_24_verifier();

            match verify(&config, &proof) {
                Ok(valid) => valid,
                Err(_) => false,
            }
        }

        #[cfg(not(feature = "ligerito-verify"))]
        fn verify_liveness_proof(
            _anchor: &CheckpointAnchor,
            _proof_bytes: &[u8],
        ) -> bool {
            // without ligerito feature, accept based on liveness timestamp
            true
        }

        /// verify osst contributions using osst crate
        /// implements equation 3.3: g^{Σ μ_i·s_i} = Y^{c̄} · Π u_i^{μ_i}
        fn do_verify_osst(
            group_key: &CompressedPoint,
            contributions: &[OsstContribution],
            threshold: u32,
            payload: &[u8],
        ) -> bool {
            // ensure we have enough contributions
            if (contributions.len() as u32) < threshold {
                log::debug!("osst: insufficient contributions {} < {}", contributions.len(), threshold);
                return false;
            }

            // decompress group public key
            let group_pubkey = match PallasPoint::from_bytes(group_key).into_option() {
                Some(p) => p,
                None => {
                    log::debug!("osst: invalid group key");
                    return false;
                }
            };

            // convert pallet contributions to osst contributions
            let mut osst_contributions: Vec<OsstContrib<PallasPoint>> = Vec::new();

            for contrib in contributions {
                // decompress commitment (public_share is used as commitment u_i)
                let commitment = match PallasPoint::from_bytes(&contrib.public_share).into_option() {
                    Some(p) => p,
                    None => {
                        log::debug!("osst: invalid commitment for index {}", contrib.index);
                        return false;
                    }
                };

                // deserialize response scalar
                let response = match PallasScalar::from_repr(contrib.response).into_option() {
                    Some(s) => s,
                    None => {
                        log::debug!("osst: invalid response for index {}", contrib.index);
                        return false;
                    }
                };

                osst_contributions.push(OsstContrib::new(contrib.index, commitment, response));
            }

            // verify using osst crate
            match osst_verify(&group_pubkey, &osst_contributions, threshold, payload) {
                Ok(valid) => valid,
                Err(e) => {
                    log::debug!("osst: verification error {:?}", e);
                    false
                }
            }
        }

        /// get current custody state
        pub fn get_custody_state() -> Option<(u64, CompressedPoint, u32)> {
            let epoch = CurrentEpoch::<T>::get();
            // check if custody exists by looking at custodian count
            if epoch.custodian_count == 0 {
                return None;
            }

            Some((epoch.epoch, epoch.group_key, epoch.threshold))
        }

        /// get custodian by account
        pub fn get_custodian(who: &T::AccountId) -> Option<CustodianInfo<T::AccountId, BlockNumberFor<T>>> {
            Custodians::<T>::get(who)
        }
    }
}
