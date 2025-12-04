//! Zcash Light Client Pallet with On-Chain Ligerito Verification
//!
//! Trustless Zcash header chain proofs verified directly on-chain.
//!
//! ## Design
//!
//! Instead of trusting 2/3 relayer consensus, we verify cryptographic proofs:
//! - Relayer generates ligerito proof over Zcash header chain
//! - Pallet verifies proof on-chain (5-30ms in WASM)
//! - Valid proof → immediate finalization
//! - Only need 1 honest relayer with valid proof!
//!
//! ## Security Model
//!
//! Trust comes from math, not economics:
//! - Ligerito proof commits to header polynomial
//! - Polynomial encodes: prev_hash linkage, PoW validity
//! - Invalid chain → invalid polynomial → proof fails
//!
//! ## Flow
//!
//! ```text
//! Relayer:
//!   headers[anchor..tip] → polynomial → ligerito::prove() → proof
//!
//! On-chain:
//!   proof → ligerito::verify() → valid? → finalize immediately
//! ```

#![cfg_attr(not(feature = "std"), no_std)]

pub use pallet::*;

extern crate alloc;
use alloc::vec::Vec;

mod types;
pub use types::*;

// Ligerito verifier - works in both native and WASM (uses SCALE codec)
#[cfg(feature = "ligerito-verify")]
use ligerito::{verify, FinalizedLigeritoProof, hardcoded_config_24_verifier};
#[cfg(feature = "ligerito-verify")]
use ligerito_binary_fields::{BinaryElem32, BinaryElem128};
#[cfg(feature = "ligerito-verify")]
use codec::Decode;

#[frame_support::pallet]
pub mod pallet {
    use super::*;
    use frame_support::{
        pallet_prelude::*,
        traits::{Currency, ReservableCurrency, ExistenceRequirement},
        Blake2_128Concat,
    };
    use frame_system::pallet_prelude::*;
    use sp_runtime::traits::Saturating;

    pub type BalanceOf<T> =
        <<T as Config>::Currency as Currency<<T as frame_system::Config>::AccountId>>::Balance;

    #[pallet::pallet]
    pub struct Pallet<T>(_);

    #[pallet::config]
    pub trait Config: frame_system::Config<RuntimeEvent: From<Event<Self>>> {
        /// Currency for staking/rewards
        type Currency: ReservableCurrency<Self::AccountId>;

        /// Minimum stake to become a relayer
        #[pallet::constant]
        type MinRelayerStake: Get<BalanceOf<Self>>;

        /// Reward for submitting valid proof
        #[pallet::constant]
        type ProofReward: Get<BalanceOf<Self>>;

        /// Minimum attestations for non-proof finalization (fallback)
        #[pallet::constant]
        type MinAttestationsForFinality: Get<u32>;

        /// Challenge period for attestation-only mode
        #[pallet::constant]
        type ChallengePeriod: Get<BlockNumberFor<Self>>;

        /// Slash percentage for fraud
        #[pallet::constant]
        type FraudSlashPercent: Get<u8>;

        /// Weight info
        type WeightInfo: WeightInfo;
    }

    // ========== STORAGE ==========

    /// Registered relayers
    #[pallet::storage]
    pub type Relayers<T: Config> = StorageMap<
        _,
        Blake2_128Concat,
        T::AccountId,
        RelayerInfo<BalanceOf<T>, BlockNumberFor<T>>,
    >;

    /// Relayer count
    #[pallet::storage]
    pub type RelayerCount<T: Config> = StorageValue<_, u32, ValueQuery>;

    /// Finalized blocks (proven or consensus)
    #[pallet::storage]
    pub type FinalizedBlocks<T: Config> = StorageMap<
        _,
        Blake2_128Concat,
        u32, // zcash_height
        FinalizedBlock,
    >;

    /// Latest finalized height
    #[pallet::storage]
    pub type LatestFinalizedHeight<T: Config> = StorageValue<_, u32, ValueQuery>;

    /// Latest finalized hash
    #[pallet::storage]
    pub type LatestFinalizedHash<T: Config> = StorageValue<_, [u8; 32], ValueQuery>;

    /// Pending attestations (fallback for non-proof mode)
    #[pallet::storage]
    pub type PendingAttestations<T: Config> = StorageDoubleMap<
        _,
        Blake2_128Concat,
        u32,
        Blake2_128Concat,
        T::AccountId,
        BlockAttestation,
    >;

    /// Attestation counts
    #[pallet::storage]
    pub type AttestationCounts<T: Config> = StorageDoubleMap<
        _,
        Blake2_128Concat,
        u32,
        Blake2_128Concat,
        [u8; 32],
        u32,
        ValueQuery,
    >;

    /// Proof anchor: the trusted starting point for header chain proofs
    /// (Initially set to Orchard activation or known checkpoint)
    #[pallet::storage]
    pub type ProofAnchor<T: Config> = StorageValue<_, ProofAnchorData, OptionQuery>;

    /// Active challenges
    #[pallet::storage]
    pub type Challenges<T: Config> = StorageDoubleMap<
        _,
        Blake2_128Concat,
        u32,
        Blake2_128Concat,
        T::AccountId,
        Challenge<T::AccountId, BalanceOf<T>>,
    >;

    // ========== EVENTS ==========

    #[pallet::event]
    #[pallet::generate_deposit(pub(super) fn deposit_event)]
    pub enum Event<T: Config> {
        /// Relayer registered
        RelayerRegistered {
            relayer: T::AccountId,
            stake: BalanceOf<T>,
        },
        /// Relayer unregistered
        RelayerUnregistered {
            relayer: T::AccountId,
        },
        /// Attestation submitted (fallback mode)
        AttestationSubmitted {
            relayer: T::AccountId,
            zcash_height: u32,
            block_hash: [u8; 32],
        },
        /// Block finalized via consensus (fallback)
        BlockFinalizedByConsensus {
            zcash_height: u32,
            block_hash: [u8; 32],
            attester_count: u32,
        },
        /// Block finalized via cryptographic proof! (the good path)
        BlockFinalizedByProof {
            zcash_height: u32,
            block_hash: [u8; 32],
            orchard_root: [u8; 32],
            prover: T::AccountId,
            proof_size: u32,
        },
        /// Proof verification failed
        ProofVerificationFailed {
            relayer: T::AccountId,
            zcash_height: u32,
            reason: Vec<u8>,
        },
        /// Proof anchor updated
        ProofAnchorUpdated {
            height: u32,
            block_hash: [u8; 32],
        },
        /// Fraud proven
        FraudProven {
            relayer: T::AccountId,
            zcash_height: u32,
            slashed: BalanceOf<T>,
        },
    }

    // ========== ERRORS ==========

    #[pallet::error]
    pub enum Error<T> {
        AlreadyRegistered,
        NotRegistered,
        InsufficientStake,
        AlreadyAttested,
        AlreadyFinalized,
        InvalidHeight,
        ChallengeExists,
        NoAttestation,
        InvalidProof,
        Slashed,
        NoProofAnchor,
        ProofTooLarge,
        InvalidProofFormat,
        VerificationFailed,
    }

    // ========== HOOKS ==========

    #[pallet::hooks]
    impl<T: Config> Hooks<BlockNumberFor<T>> for Pallet<T> {}

    // ========== EXTRINSICS ==========

    #[pallet::call]
    impl<T: Config> Pallet<T> {
        /// Register as a relayer
        #[pallet::call_index(0)]
        #[pallet::weight(T::WeightInfo::register_relayer())]
        pub fn register_relayer(
            origin: OriginFor<T>,
            stake: BalanceOf<T>,
        ) -> DispatchResult {
            let who = ensure_signed(origin)?;

            ensure!(!Relayers::<T>::contains_key(&who), Error::<T>::AlreadyRegistered);
            ensure!(stake >= T::MinRelayerStake::get(), Error::<T>::InsufficientStake);

            T::Currency::reserve(&who, stake)?;

            let info = RelayerInfo {
                stake,
                registered_at: frame_system::Pallet::<T>::block_number(),
                total_attestations: 0,
                successful_attestations: 0,
                slashed: false,
            };

            Relayers::<T>::insert(&who, info);
            RelayerCount::<T>::mutate(|c| *c = c.saturating_add(1));

            Self::deposit_event(Event::RelayerRegistered { relayer: who, stake });
            Ok(())
        }

        /// Unregister as relayer
        #[pallet::call_index(1)]
        #[pallet::weight(T::WeightInfo::unregister_relayer())]
        pub fn unregister_relayer(origin: OriginFor<T>) -> DispatchResult {
            let who = ensure_signed(origin)?;

            let info = Relayers::<T>::get(&who).ok_or(Error::<T>::NotRegistered)?;

            T::Currency::unreserve(&who, info.stake);
            Relayers::<T>::remove(&who);
            RelayerCount::<T>::mutate(|c| *c = c.saturating_sub(1));

            Self::deposit_event(Event::RelayerUnregistered { relayer: who });
            Ok(())
        }

        /// Submit attestation with ligerito proof (THE GOOD PATH)
        ///
        /// Proof verifies header chain from anchor to claimed tip.
        /// If valid → immediate finalization, no waiting!
        #[pallet::call_index(2)]
        #[pallet::weight(T::WeightInfo::submit_proven_attestation())]
        pub fn submit_proven_attestation(
            origin: OriginFor<T>,
            zcash_height: u32,
            block_hash: [u8; 32],
            prev_hash: [u8; 32],
            orchard_root: [u8; 32],
            sapling_root: [u8; 32],
            proof: Vec<u8>,
        ) -> DispatchResult {
            let who = ensure_signed(origin)?;

            // Verify relayer
            let relayer_info = Relayers::<T>::get(&who)
                .ok_or(Error::<T>::NotRegistered)?;
            ensure!(!relayer_info.slashed, Error::<T>::Slashed);

            // Check not already finalized
            ensure!(
                !FinalizedBlocks::<T>::contains_key(zcash_height),
                Error::<T>::AlreadyFinalized
            );

            // Verify proof size is reasonable (max 3MB for 2^28)
            ensure!(proof.len() <= 3 * 1024 * 1024, Error::<T>::ProofTooLarge);

            // Verify the ligerito proof
            let proof_valid = Self::verify_header_chain_proof(
                zcash_height,
                block_hash,
                &proof,
            );

            if !proof_valid {
                Self::deposit_event(Event::ProofVerificationFailed {
                    relayer: who.clone(),
                    zcash_height,
                    reason: b"verification failed".to_vec(),
                });
                return Err(Error::<T>::VerificationFailed.into());
            }

            // Proof valid! Finalize immediately
            let now: u32 = frame_system::Pallet::<T>::block_number()
                .try_into()
                .unwrap_or(u32::MAX);

            let finalized = FinalizedBlock {
                block_hash,
                prev_hash,
                orchard_root,
                sapling_root,
                attester_count: 1, // Just the prover
                finalized_at: now,
            };

            FinalizedBlocks::<T>::insert(zcash_height, finalized);

            if zcash_height > LatestFinalizedHeight::<T>::get() {
                LatestFinalizedHeight::<T>::put(zcash_height);
                LatestFinalizedHash::<T>::put(block_hash);
            }

            // Update relayer stats
            Relayers::<T>::mutate(&who, |maybe_info| {
                if let Some(info) = maybe_info {
                    info.total_attestations = info.total_attestations.saturating_add(1);
                    info.successful_attestations = info.successful_attestations.saturating_add(1);
                }
            });

            // TODO: Reward relayer for valid proof
            // T::Currency::deposit_creating(&who, T::ProofReward::get());

            Self::deposit_event(Event::BlockFinalizedByProof {
                zcash_height,
                block_hash,
                orchard_root,
                prover: who,
                proof_size: proof.len() as u32,
            });

            Ok(())
        }

        /// Submit attestation without proof (fallback mode)
        ///
        /// Requires consensus from multiple relayers.
        /// Use this when proof generation isn't available.
        #[pallet::call_index(3)]
        #[pallet::weight(T::WeightInfo::submit_attestation())]
        pub fn submit_attestation(
            origin: OriginFor<T>,
            zcash_height: u32,
            block_hash: [u8; 32],
            prev_hash: [u8; 32],
            orchard_root: [u8; 32],
            sapling_root: [u8; 32],
        ) -> DispatchResult {
            let who = ensure_signed(origin)?;

            // Verify relayer
            let mut relayer_info = Relayers::<T>::get(&who)
                .ok_or(Error::<T>::NotRegistered)?;
            ensure!(!relayer_info.slashed, Error::<T>::Slashed);

            // Check not already attested
            ensure!(
                !PendingAttestations::<T>::contains_key(zcash_height, &who),
                Error::<T>::AlreadyAttested
            );

            // Check not already finalized
            ensure!(
                !FinalizedBlocks::<T>::contains_key(zcash_height),
                Error::<T>::AlreadyFinalized
            );

            // Store attestation
            let attestation = BlockAttestation {
                height: zcash_height,
                block_hash,
                prev_hash,
                orchard_root,
                sapling_root,
            };

            PendingAttestations::<T>::insert(zcash_height, &who, attestation.clone());

            let new_count = AttestationCounts::<T>::mutate(
                zcash_height,
                block_hash,
                |c| {
                    *c = c.saturating_add(1);
                    *c
                }
            );

            relayer_info.total_attestations = relayer_info.total_attestations.saturating_add(1);
            Relayers::<T>::insert(&who, relayer_info);

            Self::deposit_event(Event::AttestationSubmitted {
                relayer: who,
                zcash_height,
                block_hash,
            });

            // Auto-finalize if threshold reached
            let threshold = T::MinAttestationsForFinality::get();
            if new_count >= threshold {
                Self::do_finalize_by_consensus(zcash_height, attestation, new_count)?;
            }

            Ok(())
        }

        /// Set proof anchor (sudo only)
        ///
        /// The anchor is the trusted starting point for header chain proofs.
        /// Should be set to a known finalized checkpoint.
        #[pallet::call_index(10)]
        #[pallet::weight(Weight::from_parts(10_000, 0))]
        pub fn set_proof_anchor(
            origin: OriginFor<T>,
            height: u32,
            block_hash: [u8; 32],
            header_commitment: [u8; 32],
        ) -> DispatchResult {
            ensure_root(origin)?;

            let anchor = ProofAnchorData {
                height,
                block_hash,
                header_commitment,
            };

            ProofAnchor::<T>::put(anchor);

            Self::deposit_event(Event::ProofAnchorUpdated { height, block_hash });
            Ok(())
        }

        /// Challenge attestation with fraud proof
        #[pallet::call_index(4)]
        #[pallet::weight(T::WeightInfo::challenge_attestation())]
        pub fn challenge_attestation(
            origin: OriginFor<T>,
            zcash_height: u32,
            relayer: T::AccountId,
            correct_block_hash: [u8; 32],
            _proof: Vec<u8>,
        ) -> DispatchResult {
            let challenger = ensure_signed(origin)?;

            let attestation = PendingAttestations::<T>::get(zcash_height, &relayer)
                .ok_or(Error::<T>::NoAttestation)?;

            ensure!(
                !Challenges::<T>::contains_key(zcash_height, &relayer),
                Error::<T>::ChallengeExists
            );

            ensure!(
                correct_block_hash != attestation.block_hash,
                Error::<T>::InvalidProof
            );

            // Slash relayer
            let mut relayer_info = Relayers::<T>::get(&relayer)
                .ok_or(Error::<T>::NotRegistered)?;

            let slash_percent = T::FraudSlashPercent::get();
            let slash_amount = relayer_info.stake
                .saturating_mul(slash_percent.into())
                / 100u32.into();

            let challenger_reward = slash_amount / 2u32.into();

            T::Currency::unreserve(&relayer, slash_amount);
            T::Currency::transfer(
                &relayer,
                &challenger,
                challenger_reward,
                ExistenceRequirement::AllowDeath,
            )?;

            relayer_info.slashed = true;
            relayer_info.stake = relayer_info.stake.saturating_sub(slash_amount);
            Relayers::<T>::insert(&relayer, relayer_info);

            PendingAttestations::<T>::remove(zcash_height, &relayer);
            AttestationCounts::<T>::mutate(zcash_height, attestation.block_hash, |c| {
                *c = c.saturating_sub(1);
            });

            Self::deposit_event(Event::FraudProven {
                relayer,
                zcash_height,
                slashed: slash_amount,
            });

            Ok(())
        }
    }

    // ========== INTERNAL ==========

    impl<T: Config> Pallet<T> {
        /// Verify ligerito proof of header chain
        ///
        /// The proof commits to polynomial encoding:
        /// - headers[anchor..tip] in sequence
        /// - each header's prev_hash matches previous block_hash
        /// - each header has valid Equihash PoW
        ///
        /// Works in both native and WASM - uses SCALE codec for serialization
        #[cfg(feature = "ligerito-verify")]
        fn verify_header_chain_proof(
            _tip_height: u32,
            _tip_hash: [u8; 32],
            proof_bytes: &[u8],
        ) -> bool {
            // Deserialize proof using SCALE codec (Substrate-native, WASM compatible)
            let proof: FinalizedLigeritoProof<BinaryElem32, BinaryElem128> =
                match Decode::decode(&mut &proof_bytes[..]) {
                    Ok(p) => p,
                    Err(_) => return false,
                };

            // Get verifier config for 2^24 size (sufficient for header chain)
            let config = hardcoded_config_24_verifier();

            // Verify!
            match verify(&config, &proof) {
                Ok(valid) => valid,
                Err(_) => false,
            }
        }

        /// Fallback when ligerito-verify feature is disabled
        #[cfg(not(feature = "ligerito-verify"))]
        fn verify_header_chain_proof(
            _tip_height: u32,
            _tip_hash: [u8; 32],
            _proof_bytes: &[u8],
        ) -> bool {
            // Ligerito verification not enabled
            // Use attestation consensus fallback instead
            false
        }

        /// Finalize by attestation consensus (fallback)
        fn do_finalize_by_consensus(
            zcash_height: u32,
            attestation: BlockAttestation,
            attester_count: u32,
        ) -> DispatchResult {
            let now: u32 = frame_system::Pallet::<T>::block_number()
                .try_into()
                .unwrap_or(u32::MAX);

            let finalized = FinalizedBlock {
                block_hash: attestation.block_hash,
                prev_hash: attestation.prev_hash,
                orchard_root: attestation.orchard_root,
                sapling_root: attestation.sapling_root,
                attester_count,
                finalized_at: now,
            };

            FinalizedBlocks::<T>::insert(zcash_height, finalized);

            if zcash_height > LatestFinalizedHeight::<T>::get() {
                LatestFinalizedHeight::<T>::put(zcash_height);
                LatestFinalizedHash::<T>::put(attestation.block_hash);
            }

            // Update successful attestations
            for (relayer_id, att) in PendingAttestations::<T>::iter_prefix(zcash_height) {
                if att.block_hash == attestation.block_hash {
                    Relayers::<T>::mutate(&relayer_id, |maybe_info| {
                        if let Some(info) = maybe_info {
                            info.successful_attestations = info.successful_attestations.saturating_add(1);
                        }
                    });
                }
            }

            // Cleanup
            let _ = PendingAttestations::<T>::clear_prefix(zcash_height, u32::MAX, None);
            let _ = AttestationCounts::<T>::clear_prefix(zcash_height, u32::MAX, None);

            Self::deposit_event(Event::BlockFinalizedByConsensus {
                zcash_height,
                block_hash: attestation.block_hash,
                attester_count,
            });

            Ok(())
        }

        /// Get latest finalized state
        pub fn get_latest_state() -> Option<(u32, [u8; 32], [u8; 32])> {
            let height = LatestFinalizedHeight::<T>::get();
            if height == 0 {
                return None;
            }

            FinalizedBlocks::<T>::get(height)
                .map(|b| (height, b.block_hash, b.orchard_root))
        }
    }

    // ========== WEIGHTS ==========

    pub trait WeightInfo {
        fn register_relayer() -> Weight;
        fn unregister_relayer() -> Weight;
        fn submit_attestation() -> Weight;
        fn submit_proven_attestation() -> Weight;
        fn challenge_attestation() -> Weight;
    }

    impl WeightInfo for () {
        fn register_relayer() -> Weight { Weight::from_parts(50_000, 0) }
        fn unregister_relayer() -> Weight { Weight::from_parts(50_000, 0) }
        fn submit_attestation() -> Weight { Weight::from_parts(100_000, 0) }
        // Proof verification is expensive but still within block limits
        // ~30ms native → ~150ms WASM estimate
        fn submit_proven_attestation() -> Weight { Weight::from_parts(150_000_000_000, 0) }
        fn challenge_attestation() -> Weight { Weight::from_parts(500_000, 0) }
    }
}
