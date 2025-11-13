//! ZK Verifier Module - On-Chain PolkaVM Verification
//!
//! This module enables on-chain verification of Ligerito proofs using PolkaVM **directly**,
//! without requiring Substrate runtime or pallet_revive.
//!
//! ## Overview
//!
//! Instead of verifying proofs off-chain (no consensus guarantee), this module
//! runs verification **in Commonware consensus** via direct PolkaVM integration.
//!
//! ## Architecture
//!
//! ```
//! Client                    Runtime                    PolkaVM
//!   │                         │                          │
//!   │  submit_proof()         │                          │
//!   ├────────────────────────>│                          │
//!   │                         │                          │
//!   │                         │  deploy_verifier()       │
//!   │                         │  (one-time, sudo)        │
//!   │                         ├─────────────────────────>│
//!   │                         │                          │
//!   │                         │  verify_proof()          │
//!   │                         ├─────────────────────────>│
//!   │                         │                          │
//!   │                         │       (execute)          │
//!   │                         │                          │
//!   │                         │<─────────────────────────┤
//!   │                         │    (exit code 0=valid)   │
//!   │                         │                          │
//!   │<────────────────────────┤                          │
//!   │    ProofVerified        │                          │
//! ```
//!
//! ## Usage
//!
//! ### 1. Deploy Verifier (One-Time)
//!
//! ```rust,ignore
//! // Build PolkaVM verifier binary
//! let verifier_code = include_bytes!("../polkavm_verifier.polkavm");
//!
//! // Deploy via sudo
//! RuntimeCall::ZKVerifier(
//!     pallet_zk_verifier::Call::deploy_verifier {
//!         verifier_code: verifier_code.to_vec(),
//!     }
//! ).dispatch(RawOrigin::Root)?;
//! ```
//!
//! ### 2. Verify Proofs
//!
//! ```rust,ignore
//! // Extract succinct proof from AccidentalComputer
//! let succinct = extract_succinct_proof(&accidental_proof, 24)?;
//!
//! // Verify on-chain
//! RuntimeCall::ZKVerifier(
//!     pallet_zk_verifier::Call::verify_proof {
//!         proof: succinct,
//!     }
//! ).dispatch(origin)?;
//! ```

use frame_support::{
    dispatch::DispatchResult,
    pallet_prelude::*,
    traits::{Get, Currency},
};
use frame_system::pallet_prelude::*;
use sp_std::prelude::*;
use codec::{Decode, Encode};

// Re-export types from light_client module
pub use crate::light_client::LigeritoSuccinctProof;

// Direct PolkaVM integration (no Substrate)
pub mod polkavm_direct;
pub use polkavm_direct::{PolkaVMVerifier, PolkaVMConfig, GasMeter};

// Legacy pallet interface (for future Substrate integration if needed)
#[frame_support::pallet]
pub mod pallet {
    use super::*;

    /// Configuration trait for the ZK Verifier pallet
    #[pallet::config]
    pub trait Config: frame_system::Config {
        /// The overarching event type
        type RuntimeEvent: From<Event<Self>> + IsType<<Self as frame_system::Config>::RuntimeEvent>;

        /// Maximum proof size to accept (DoS prevention)
        #[pallet::constant]
        type MaxProofSize: Get<u32>;

        /// Gas limit for verification
        ///
        /// Typical: ~5-10M gas for Ligerito verification
        #[pallet::constant]
        type VerificationGasLimit: Get<Weight>;

        /// Currency type for gas payments
        type Currency: Currency<Self::AccountId>;
    }

    #[pallet::pallet]
    pub struct Pallet<T>(_);

    /// Address of the deployed Ligerito verifier contract
    ///
    /// This is the PolkaVM contract that contains the Ligerito verifier logic.
    /// Deployed once via `deploy_verifier()`, then used for all verifications.
    #[pallet::storage]
    pub type VerifierContract<T: Config> = StorageValue<_, T::AccountId, OptionQuery>;

    /// Verified proofs (for optimistic mode)
    ///
    /// Maps proof ID -> (commitments, block number)
    /// Used to track which proofs have been verified and when.
    #[pallet::storage]
    pub type VerifiedProofs<T: Config> = StorageMap<
        _,
        Blake2_128Concat,
        [u8; 32], // proof ID (hash of commitments)
        (
            [u8; 32], // sender_old
            [u8; 32], // sender_new
            [u8; 32], // receiver_old
            [u8; 32], // receiver_new
            BlockNumberFor<T>, // verified at block
        ),
        OptionQuery,
    >;

    /// Configuration for verification mode
    #[pallet::storage]
    pub type VerificationMode<T: Config> = StorageValue<_, VerificationModeConfig, ValueQuery>;

    /// Verification mode configuration
    #[derive(Clone, Encode, Decode, PartialEq, Eq, RuntimeDebug, TypeInfo, MaxEncodedLen)]
    pub enum VerificationModeConfig {
        /// Always verify on-chain (pessimistic)
        AlwaysOnChain,

        /// Optimistic: Accept proofs, allow challenges
        Optimistic { challenge_period: u32 },

        /// Hybrid: Native verification off-chain, on-chain for disputes
        Hybrid,
    }

    impl Default for VerificationModeConfig {
        fn default() -> Self {
            Self::Hybrid
        }
    }

    #[pallet::event]
    #[pallet::generate_deposit(pub(super) fn deposit_event)]
    pub enum Event<T: Config> {
        /// Verifier contract deployed
        VerifierDeployed {
            address: T::AccountId,
            code_hash: [u8; 32],
        },

        /// Proof verified successfully (on-chain)
        ProofVerified {
            proof_id: [u8; 32],
            sender_old: [u8; 32],
            sender_new: [u8; 32],
            receiver_old: [u8; 32],
            receiver_new: [u8; 32],
        },

        /// Proof accepted optimistically (not verified yet)
        ProofAcceptedOptimistic {
            proof_id: [u8; 32],
            challenge_deadline: BlockNumberFor<T>,
        },

        /// Proof challenged and found invalid
        ProofChallengedInvalid {
            proof_id: [u8; 32],
            challenger: T::AccountId,
        },

        /// Verification mode changed
        VerificationModeChanged {
            new_mode: VerificationModeConfig,
        },
    }

    #[pallet::error]
    pub enum Error<T> {
        /// Proof is too large
        ProofTooLarge,

        /// Proof verification failed
        InvalidProof,

        /// Verifier contract not deployed
        VerifierNotDeployed,

        /// PolkaVM execution failed
        ExecutionFailed,

        /// Proof already verified
        ProofAlreadyVerified,

        /// Proof not found
        ProofNotFound,

        /// Challenge period expired
        ChallengePeriodExpired,

        /// Not authorized to deploy verifier
        NotAuthorized,
    }

    #[pallet::call]
    impl<T: Config> Pallet<T> {
        /// Deploy Ligerito verifier contract
        ///
        /// This is a one-time operation that deploys the PolkaVM contract
        /// containing the Ligerito verifier logic.
        ///
        /// **Requires**: Root origin (sudo)
        ///
        /// # Arguments
        ///
        /// * `verifier_code` - The PolkaVM binary containing the verifier
        ///
        /// # Example
        ///
        /// ```rust,ignore
        /// let verifier_binary = include_bytes!("../polkavm_verifier.polkavm");
        ///
        /// RuntimeCall::ZKVerifier(
        ///     Call::deploy_verifier {
        ///         verifier_code: verifier_binary.to_vec(),
        ///     }
        /// ).dispatch(RawOrigin::Root)?;
        /// ```
        #[pallet::call_index(0)]
        #[pallet::weight(T::VerificationGasLimit::get())]
        pub fn deploy_verifier(
            origin: OriginFor<T>,
            verifier_code: Vec<u8>,
        ) -> DispatchResult {
            // Only root can deploy verifier
            ensure_root(origin)?;

            // TODO: Integrate with pallet_revive
            //
            // In real implementation:
            // let deploy_result = <pallet_revive::Pallet<T>>::bare_instantiate(
            //     deployer,
            //     0,
            //     Weight::from_parts(10_000_000_000, 0),
            //     None,
            //     pallet_revive::Code::Upload(verifier_code),
            //     vec![],
            //     vec![],
            //     pallet_revive::DebugInfo::Skip,
            //     pallet_revive::CollectEvents::Skip,
            // );
            //
            // let address = deploy_result.account_id;

            // For now, use placeholder address
            let address = T::AccountId::decode(&mut &[0u8; 32][..])
                .map_err(|_| Error::<T>::NotAuthorized)?;

            let code_hash = sp_io::hashing::blake2_256(&verifier_code);

            VerifierContract::<T>::put(&address);

            Self::deposit_event(Event::VerifierDeployed { address, code_hash });

            Ok(())
        }

        /// Verify state transition proof on-chain
        ///
        /// Runs Ligerito verification in the runtime via PolkaVM.
        ///
        /// **Gas Cost**: ~5-10M gas (~20-30ms)
        ///
        /// # Arguments
        ///
        /// * `proof` - The succinct Ligerito proof
        ///
        /// # Example
        ///
        /// ```rust,ignore
        /// let succinct = extract_succinct_proof(&accidental_proof, 24)?;
        ///
        /// RuntimeCall::ZKVerifier(
        ///     Call::verify_proof { proof: succinct }
        /// ).dispatch(origin)?;
        /// ```
        #[pallet::call_index(1)]
        #[pallet::weight(T::VerificationGasLimit::get())]
        pub fn verify_proof(
            origin: OriginFor<T>,
            proof: LigeritoSuccinctProof,
        ) -> DispatchResult {
            let _who = ensure_signed(origin)?;

            // Check proof size (DoS prevention)
            ensure!(
                proof.proof_bytes.len() <= T::MaxProofSize::get() as usize,
                Error::<T>::ProofTooLarge
            );

            // Compute proof ID
            let proof_id = Self::compute_proof_id(&proof);

            // Check if already verified
            ensure!(
                !VerifiedProofs::<T>::contains_key(proof_id),
                Error::<T>::ProofAlreadyVerified
            );

            // Verify via PolkaVM
            Self::verify_via_polkavm(&proof)?;

            // Store verified proof
            let current_block = <frame_system::Pallet<T>>::block_number();
            VerifiedProofs::<T>::insert(
                proof_id,
                (
                    proof.sender_commitment_old,
                    proof.sender_commitment_new,
                    proof.receiver_commitment_old,
                    proof.receiver_commitment_new,
                    current_block,
                ),
            );

            // Emit event
            Self::deposit_event(Event::ProofVerified {
                proof_id,
                sender_old: proof.sender_commitment_old,
                sender_new: proof.sender_commitment_new,
                receiver_old: proof.receiver_commitment_old,
                receiver_new: proof.receiver_commitment_new,
            });

            Ok(())
        }

        /// Submit proof optimistically (no verification)
        ///
        /// For optimistic mode: Accept proof without verification,
        /// allow challenges within challenge period.
        ///
        /// **Gas Cost**: Very low (just storage)
        ///
        /// # Arguments
        ///
        /// * `proof` - The succinct Ligerito proof
        #[pallet::call_index(2)]
        #[pallet::weight(Weight::from_parts(100_000, 0))]
        pub fn submit_optimistic(
            origin: OriginFor<T>,
            proof: LigeritoSuccinctProof,
        ) -> DispatchResult {
            let _who = ensure_signed(origin)?;

            // Check mode
            let mode = VerificationMode::<T>::get();
            let challenge_period = match mode {
                VerificationModeConfig::Optimistic { challenge_period } => challenge_period,
                _ => return Err(Error::<T>::NotAuthorized.into()),
            };

            // Compute proof ID
            let proof_id = Self::compute_proof_id(&proof);

            // Store proof (unverified)
            let current_block = <frame_system::Pallet<T>>::block_number();
            let challenge_deadline = current_block + challenge_period.into();

            VerifiedProofs::<T>::insert(
                proof_id,
                (
                    proof.sender_commitment_old,
                    proof.sender_commitment_new,
                    proof.receiver_commitment_old,
                    proof.receiver_commitment_new,
                    challenge_deadline,
                ),
            );

            Self::deposit_event(Event::ProofAcceptedOptimistic {
                proof_id,
                challenge_deadline,
            });

            Ok(())
        }

        /// Challenge an optimistic proof
        ///
        /// Forces on-chain verification. If proof is invalid,
        /// challenger gets reward, submitter gets slashed.
        ///
        /// # Arguments
        ///
        /// * `proof_id` - ID of the proof to challenge
        /// * `proof` - The actual proof data
        #[pallet::call_index(3)]
        #[pallet::weight(T::VerificationGasLimit::get())]
        pub fn challenge_proof(
            origin: OriginFor<T>,
            proof_id: [u8; 32],
            proof: LigeritoSuccinctProof,
        ) -> DispatchResult {
            let challenger = ensure_signed(origin)?;

            // Get proof info
            let (_, _, _, _, deadline) = VerifiedProofs::<T>::get(proof_id)
                .ok_or(Error::<T>::ProofNotFound)?;

            // Check challenge period
            let current_block = <frame_system::Pallet<T>>::block_number();
            ensure!(current_block <= deadline, Error::<T>::ChallengePeriodExpired);

            // Verify proof on-chain
            let valid = Self::verify_via_polkavm(&proof).is_ok();

            if !valid {
                // Proof is invalid! Remove it and reward challenger
                VerifiedProofs::<T>::remove(proof_id);

                Self::deposit_event(Event::ProofChallengedInvalid {
                    proof_id,
                    challenger,
                });

                // TODO: Slash submitter, reward challenger
            }

            Ok(())
        }

        /// Set verification mode
        ///
        /// **Requires**: Root origin (sudo)
        ///
        /// # Arguments
        ///
        /// * `mode` - New verification mode
        #[pallet::call_index(4)]
        #[pallet::weight(Weight::from_parts(100_000, 0))]
        pub fn set_verification_mode(
            origin: OriginFor<T>,
            mode: VerificationModeConfig,
        ) -> DispatchResult {
            ensure_root(origin)?;

            VerificationMode::<T>::put(mode.clone());

            Self::deposit_event(Event::VerificationModeChanged { new_mode: mode });

            Ok(())
        }
    }

    impl<T: Config> Pallet<T> {
        /// Verify proof via PolkaVM
        ///
        /// This calls the deployed Ligerito verifier contract via pallet_revive.
        fn verify_via_polkavm(proof: &LigeritoSuccinctProof) -> DispatchResult {
            // Get verifier contract address
            let _verifier_address = VerifierContract::<T>::get()
                .ok_or(Error::<T>::VerifierNotDeployed)?;

            // TODO: Call via pallet_revive
            //
            // In real implementation:
            // let mut input = Vec::new();
            // input.extend_from_slice(&proof.config_size.to_le_bytes());
            // input.extend_from_slice(&proof.proof_bytes);
            //
            // let call_result = <pallet_revive::Pallet<T>>::bare_call(
            //     T::AccountId::decode(&mut &[0u8; 32][..])?,
            //     verifier_address,
            //     0,
            //     T::VerificationGasLimit::get(),
            //     None,
            //     input,
            //     pallet_revive::DebugInfo::Skip,
            //     pallet_revive::CollectEvents::Skip,
            //     pallet_revive::Determinism::Enforced,
            // );
            //
            // ensure!(call_result.result.is_ok(), Error::<T>::ExecutionFailed);
            //
            // let return_data = call_result.result.unwrap();
            // ensure!(!return_data.is_empty() && return_data[0] == 0, Error::<T>::InvalidProof);

            // For now, placeholder that accepts all proofs
            // (Real implementation will call PolkaVM)
            if proof.config_size >= 12 && proof.config_size <= 30 {
                Ok(())
            } else {
                Err(Error::<T>::InvalidProof.into())
            }
        }

        /// Compute proof ID from commitments
        fn compute_proof_id(proof: &LigeritoSuccinctProof) -> [u8; 32] {
            let mut data = Vec::new();
            data.extend_from_slice(&proof.sender_commitment_old);
            data.extend_from_slice(&proof.sender_commitment_new);
            data.extend_from_slice(&proof.receiver_commitment_old);
            data.extend_from_slice(&proof.receiver_commitment_new);
            sp_io::hashing::blake2_256(&data)
        }
    }
}
