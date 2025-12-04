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

#[cfg(feature = "std")]
use sp_core::offchain::StorageKind;

use frame_support::traits::Get;
use sp_std::prelude::*;

/// FROST signature on Pallas curve (Zcash Orchard)
#[derive(Clone, Encode, Decode, TypeInfo, MaxEncodedLen, Debug, PartialEq)]
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
#[derive(Clone, Encode, Decode, TypeInfo, MaxEncodedLen, Debug, PartialEq)]
pub enum DkgFailureReason {
    Timeout,
    InvalidCommitment,
    InvalidShare,
    InsufficientParticipation,
}

/// Signing request queued for processing
#[derive(Clone, Encode, Decode, TypeInfo, Debug, PartialEq)]
pub struct SigningRequest<AccountId> {
    /// Unique request ID
    pub id: u64,
    /// Who requested (for fee payment)
    pub requester: AccountId,
    /// Zcash transaction to sign (serialized)
    pub tx_data: Vec<u8>,
    /// Block when request was created
    pub created_at: u32,
    /// Deadline block for signing
    pub deadline: u32,
    /// Collected partial signatures
    pub partial_sigs: Vec<(u16, FrostSignature)>,
    /// Final aggregated signature (when complete)
    pub final_sig: Option<FrostSignature>,
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
    /// Whether currently active
    pub active: bool,
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
    pub type ActiveSignerList<T: Config> = StorageValue<_, Vec<T::AccountId>, ValueQuery>;

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
        Vec<u8>, // encrypted share
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
        SigningRequest<T::AccountId>,
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
        },

        /// Signing failed (timeout or invalid sigs)
        SigningFailed {
            request_id: u64,
        },

        /// Key rotation initiated
        RotationStarted {
            old_signers: u16,
            new_signers: u16,
        },

        /// Signer slashed for misbehavior
        SignerSlashed {
            who: T::AccountId,
            amount: u128,
            reason: Vec<u8>,
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
            signers.push(who.clone());
            ActiveSignerList::<T>::put(signers);

            let info = SignerInfo {
                account: who.clone(),
                index,
                public_share: [0u8; 32], // Set after DKG
                encryption_key,
                joined_at: frame_system::Pallet::<T>::block_number().saturated_into(),
                active: false, // Active after DKG
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
            encrypted_share: Vec<u8>,
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
            signer.active = true;
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
            tx_data: Vec<u8>,
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
                partial_sigs: Vec::new(),
                final_sig: None,
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
            ensure!(signer.active, Error::<T>::NotRegistered);

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

            request.partial_sigs.push((signer_index, partial_sig));

            Self::deposit_event(Event::PartialSignatureSubmitted {
                request_id,
                signer_index,
            });

            // Check if we have threshold
            if request.partial_sigs.len() >= T::Threshold::get() as usize {
                // Aggregate signatures
                if let Some(final_sig) = Self::aggregate_signatures(&request.partial_sigs) {
                    request.final_sig = Some(final_sig.clone());

                    Self::deposit_event(Event::SigningCompleted {
                        request_id,
                        signature: final_sig,
                    });
                }
            }

            SigningQueue::<T>::insert(request_id, request);

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

            // Check signing timeouts
            // (In production, iterate and clean up expired requests)
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
                .filter(|info| info.active)
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
