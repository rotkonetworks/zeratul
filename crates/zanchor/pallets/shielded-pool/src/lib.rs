//! shielded pool pallet
//!
//! penumbra-style privacy for zbtc/zzec. users are their own executors.
//!
//! ## architecture
//!
//! on-chain (minimal):
//! - commitment tree (append-only merkle tree of note commitments)
//! - spent nullifiers (prevents double-spend)
//! - current merkle root
//!
//! off-chain (signer gossip via litep2p):
//! - encrypted notes (user encrypts to self, gossips to signers)
//! - signers store notes indexed by commitment
//! - user fetches note from any signer to spend
//!
//! ## extrinsics
//!
//! - `shield`: burn transparent zbtc, emit commitment (note gossiped separately)
//! - `spend`: verify proof, mark nullifier spent, emit new commitment
//! - `withdraw`: verify proof, mark nullifier spent, request FROST signature
//!
//! ## privacy model
//!
//! - shield: public (links transparent burn to commitment)
//! - spend: private (only nullifier + new commitment visible)
//! - withdraw: user sends withdrawal request directly to signers (off-chain)
//!             signers verify proof, FROST sign btc tx, user broadcasts
//!             signers post nullifier to chain (just the nullifier, nothing else)

#![cfg_attr(not(feature = "std"), no_std)]

pub use pallet::*;

#[cfg(test)]
mod tests;

use codec::{Decode, DecodeWithMemTracking, Encode, MaxEncodedLen};
use frame_support::pallet_prelude::BoundedVec;
use frame_support::traits::fungibles::{Inspect, Mutate};
use frame_support::traits::Get;
use scale_info::TypeInfo;
use sp_core::H256;
use sp_std::prelude::*;

/// max tree depth (2^32 notes)
pub const TREE_DEPTH: u32 = 32;

/// max proof size
pub type MaxProofSize = frame_support::traits::ConstU32<1024>;

// ============ core types ============

/// asset type in shielded pool
#[derive(Clone, Copy, Encode, Decode, DecodeWithMemTracking, TypeInfo, MaxEncodedLen, Debug, PartialEq, Eq, Default)]
pub enum AssetType {
    #[default]
    Btc = 0,
    Zec = 1,
}

/// note commitment (hash of note contents)
/// commitment = poseidon(value, asset_type, owner_pubkey, blinding)
#[derive(Clone, Copy, Encode, Decode, DecodeWithMemTracking, TypeInfo, MaxEncodedLen, Debug, PartialEq, Eq, Default)]
pub struct NoteCommitment(pub H256);

impl NoteCommitment {
    pub fn from_bytes(bytes: [u8; 32]) -> Self {
        Self(H256::from(bytes))
    }
}

/// nullifier prevents double-spending
/// nullifier = poseidon(commitment, nullifier_key, position)
#[derive(Clone, Copy, Encode, Decode, DecodeWithMemTracking, TypeInfo, MaxEncodedLen, Debug, PartialEq, Eq, Default, Hash)]
pub struct Nullifier(pub H256);

impl Nullifier {
    pub fn from_bytes(bytes: [u8; 32]) -> Self {
        Self(H256::from(bytes))
    }
}

/// merkle tree root
#[derive(Clone, Copy, Encode, Decode, DecodeWithMemTracking, TypeInfo, MaxEncodedLen, Debug, PartialEq, Eq, Default)]
pub struct MerkleRoot(pub H256);

/// position in the commitment tree (leaf index)
#[derive(Clone, Copy, Encode, Decode, DecodeWithMemTracking, TypeInfo, MaxEncodedLen, Debug, PartialEq, Eq, Default)]
pub struct TreePosition(pub u64);

/// spend proof (zkproof of note ownership + nullifier derivation)
#[derive(Clone, Encode, Decode, DecodeWithMemTracking, TypeInfo, MaxEncodedLen, Debug, PartialEq)]
pub struct SpendProof {
    /// the proof bytes (groth16 or plonk)
    pub proof: BoundedVec<u8, MaxProofSize>,
}

/// output proof (zkproof that new note is valid)
#[derive(Clone, Encode, Decode, DecodeWithMemTracking, TypeInfo, MaxEncodedLen, Debug, PartialEq)]
pub struct OutputProof {
    /// the proof bytes
    pub proof: BoundedVec<u8, MaxProofSize>,
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
        /// runtime event type
        type RuntimeEvent: From<Event<Self>> + IsType<<Self as frame_system::Config>::RuntimeEvent>;

        /// asset id type
        type AssetId: Member + Parameter + MaxEncodedLen + Copy + Default + Ord;

        /// balance type
        type Balance: Member
            + Parameter
            + MaxEncodedLen
            + Copy
            + Default
            + From<u64>
            + Into<u128>
            + sp_runtime::traits::AtLeast32BitUnsigned;

        /// fungible assets for burning transparent tokens
        type Assets: Inspect<Self::AccountId, AssetId = Self::AssetId, Balance = Self::Balance>
            + Mutate<Self::AccountId>;

        /// zbtc asset id
        #[pallet::constant]
        type ZbtcAssetId: Get<Self::AssetId>;

        /// zzec asset id
        #[pallet::constant]
        type ZzecAssetId: Get<Self::AssetId>;

        /// minimum shielding amount (dust limit)
        #[pallet::constant]
        type MinShieldAmount: Get<u64>;

        /// how many historical roots to keep valid
        #[pallet::constant]
        type RootHistorySize: Get<u32>;
    }

    // ============ storage ============

    /// commitment tree - append only merkle tree of note commitments
    /// stored as: position -> commitment
    #[pallet::storage]
    pub type CommitmentTree<T: Config> = StorageMap<
        _,
        Blake2_128Concat,
        TreePosition,
        NoteCommitment,
    >;

    /// next position in commitment tree
    #[pallet::storage]
    pub type NextTreePosition<T: Config> = StorageValue<_, TreePosition, ValueQuery>;

    /// current merkle root
    #[pallet::storage]
    pub type CurrentRoot<T: Config> = StorageValue<_, MerkleRoot, ValueQuery>;

    /// historical roots (ring buffer for proof verification)
    /// maps root -> block number when it was current
    #[pallet::storage]
    pub type HistoricalRoots<T: Config> = StorageMap<
        _,
        Blake2_128Concat,
        MerkleRoot,
        BlockNumberFor<T>,
    >;

    /// spent nullifiers (just the nullifier, nothing else)
    #[pallet::storage]
    pub type SpentNullifiers<T: Config> = StorageMap<
        _,
        Blake2_128Concat,
        Nullifier,
        BlockNumberFor<T>, // block when spent
    >;

    /// total shielded value per asset type
    #[pallet::storage]
    pub type TotalShielded<T: Config> = StorageMap<
        _,
        Blake2_128Concat,
        AssetType,
        u128,
        ValueQuery,
    >;

    // ============ events ============

    #[pallet::event]
    #[pallet::generate_deposit(pub(super) fn deposit_event)]
    pub enum Event<T: Config> {
        /// note commitment added to tree
        /// user watches for this, then gossips encrypted note to signers
        NoteCommitted {
            commitment: NoteCommitment,
            position: TreePosition,
            asset_type: AssetType,
        },

        /// nullifier spent (no other info - preserves privacy)
        NullifierSpent {
            nullifier: Nullifier,
        },

        /// merkle root updated
        RootUpdated {
            root: MerkleRoot,
            tree_size: u64,
        },
    }

    // ============ errors ============

    #[pallet::error]
    pub enum Error<T> {
        /// nullifier already spent
        NullifierAlreadySpent,
        /// invalid spend proof
        InvalidSpendProof,
        /// invalid output proof
        InvalidOutputProof,
        /// anchor (merkle root) not found or too old
        InvalidAnchor,
        /// amount below minimum
        AmountBelowMinimum,
        /// arithmetic overflow
        Overflow,
        /// value not conserved (inputs != outputs)
        ValueNotConserved,
    }

    // ============ calls ============

    #[pallet::call]
    impl<T: Config> Pallet<T> {
        /// shield transparent value into the pool
        ///
        /// burns transparent zbtc/zzec, adds commitment to tree.
        /// user must gossip encrypted note to signers separately.
        #[pallet::call_index(0)]
        #[pallet::weight(50_000)]
        pub fn shield(
            origin: OriginFor<T>,
            asset_type: AssetType,
            amount: u64,
            commitment: NoteCommitment,
        ) -> DispatchResult {
            let who = ensure_signed(origin)?;

            ensure!(
                amount >= T::MinShieldAmount::get(),
                Error::<T>::AmountBelowMinimum
            );

            // get asset id based on type
            let asset_id = match asset_type {
                AssetType::Btc => T::ZbtcAssetId::get(),
                AssetType::Zec => T::ZzecAssetId::get(),
            };

            let balance: T::Balance = amount.into();

            // burn transparent tokens from caller
            T::Assets::burn_from(
                asset_id,
                &who,
                balance,
                frame_support::traits::tokens::Preservation::Expendable,
                frame_support::traits::tokens::Precision::Exact,
                frame_support::traits::tokens::Fortitude::Polite,
            )?;

            // add commitment to tree
            let position = Self::append_commitment(commitment)?;

            // update total shielded
            TotalShielded::<T>::mutate(asset_type, |total| {
                *total = total.saturating_add(amount as u128);
            });

            Self::deposit_event(Event::NoteCommitted {
                commitment,
                position,
                asset_type,
            });

            Ok(())
        }

        /// private spend - consume old note, create new note
        ///
        /// verifies:
        /// 1. nullifier derived correctly from spent note
        /// 2. spent note exists in commitment tree (anchor proof)
        /// 3. new commitment is valid
        /// 4. value is conserved (can be checked in proof or as public input)
        #[pallet::call_index(1)]
        #[pallet::weight(100_000)]
        pub fn spend(
            origin: OriginFor<T>,
            // spend side
            anchor: MerkleRoot,
            nullifier: Nullifier,
            _spend_proof: SpendProof,
            // output side
            new_commitment: NoteCommitment,
            _output_proof: OutputProof,
        ) -> DispatchResult {
            // anyone can submit (no signer required - proof is self-authenticating)
            let _who = ensure_signed(origin)?;

            // check nullifier not already spent
            ensure!(
                !SpentNullifiers::<T>::contains_key(nullifier),
                Error::<T>::NullifierAlreadySpent
            );

            // verify anchor is valid (current or recent root)
            ensure!(
                Self::is_valid_anchor(&anchor),
                Error::<T>::InvalidAnchor
            );

            // todo: verify spend proof
            // groth16::verify(spend_proof, public_inputs)?

            // todo: verify output proof
            // groth16::verify(output_proof, public_inputs)?

            // mark nullifier as spent
            let now = frame_system::Pallet::<T>::block_number();
            SpentNullifiers::<T>::insert(nullifier, now);

            Self::deposit_event(Event::NullifierSpent { nullifier });

            // add new commitment to tree
            let position = Self::append_commitment(new_commitment)?;

            Self::deposit_event(Event::NoteCommitted {
                commitment: new_commitment,
                position,
                asset_type: AssetType::Btc, // todo: get from proof
            });

            Ok(())
        }

        /// mark nullifier as spent (called by signers after FROST signing withdrawal)
        ///
        /// withdrawal flow:
        /// 1. user sends withdrawal request to signers (off-chain, encrypted)
        /// 2. signers verify spend proof against on-chain anchor
        /// 3. signers FROST sign btc transaction
        /// 4. user broadcasts btc tx
        /// 5. signers call this to mark nullifier spent
        ///
        /// note: this is intentionally minimal - just marks nullifier spent.
        /// no withdrawal details on-chain = no privacy leak.
        #[pallet::call_index(2)]
        #[pallet::weight(30_000)]
        pub fn mark_nullifier_spent(
            origin: OriginFor<T>,
            nullifier: Nullifier,
            asset_type: AssetType,
            amount: u64,
        ) -> DispatchResult {
            // todo: require signer origin (threshold signature or privileged)
            let _who = ensure_signed(origin)?;

            // check nullifier not already spent
            ensure!(
                !SpentNullifiers::<T>::contains_key(nullifier),
                Error::<T>::NullifierAlreadySpent
            );

            // mark nullifier as spent
            let now = frame_system::Pallet::<T>::block_number();
            SpentNullifiers::<T>::insert(nullifier, now);

            // reduce total shielded (value left the pool)
            TotalShielded::<T>::mutate(asset_type, |total| {
                *total = total.saturating_sub(amount as u128);
            });

            Self::deposit_event(Event::NullifierSpent { nullifier });

            Ok(())
        }
    }

    // ============ internal functions ============

    impl<T: Config> Pallet<T> {
        /// append commitment to merkle tree
        fn append_commitment(commitment: NoteCommitment) -> Result<TreePosition, DispatchError> {
            let position = NextTreePosition::<T>::mutate(|pos| {
                let current = *pos;
                pos.0 = pos.0.saturating_add(1);
                current
            });

            CommitmentTree::<T>::insert(position, commitment);

            // update merkle root
            let old_root = CurrentRoot::<T>::get();
            let new_root = Self::compute_new_root(position, commitment);

            // store old root in history
            let now = frame_system::Pallet::<T>::block_number();
            HistoricalRoots::<T>::insert(old_root, now);

            CurrentRoot::<T>::put(new_root);

            Self::deposit_event(Event::RootUpdated {
                root: new_root,
                tree_size: position.0 + 1,
            });

            Ok(position)
        }

        /// compute new merkle root after appending
        fn compute_new_root(position: TreePosition, commitment: NoteCommitment) -> MerkleRoot {
            // todo: implement proper incremental merkle tree (poseidon hash)
            // for now, just hash position + commitment
            let mut data = position.0.to_le_bytes().to_vec();
            data.extend_from_slice(commitment.0.as_bytes());
            MerkleRoot(H256::from(sp_io::hashing::blake2_256(&data)))
        }

        /// check if anchor is valid (current or recent root)
        fn is_valid_anchor(anchor: &MerkleRoot) -> bool {
            // current root is always valid
            if CurrentRoot::<T>::get() == *anchor {
                return true;
            }

            // check historical roots
            HistoricalRoots::<T>::contains_key(anchor)
        }
    }
}

// ============ off-chain types (for signer gossip) ============

/// encrypted note (gossiped to signers, not stored on-chain)
/// user encrypts to their own pubkey, signers just store it
#[derive(Clone, Encode, Decode, TypeInfo, Debug, PartialEq)]
pub struct EncryptedNote {
    /// commitment this note corresponds to (for indexing)
    pub commitment: NoteCommitment,
    /// ephemeral public key for ECDH
    pub ephemeral_pk: [u8; 32],
    /// encrypted note contents (value, asset_type, blinding, etc)
    pub ciphertext: Vec<u8>,
}

/// withdrawal request (sent to signers off-chain)
#[derive(Clone, Encode, Decode, TypeInfo, Debug, PartialEq)]
pub struct WithdrawalRequest {
    /// nullifier being spent
    pub nullifier: Nullifier,
    /// anchor (merkle root) for the proof
    pub anchor: MerkleRoot,
    /// spend proof
    pub spend_proof: Vec<u8>,
    /// btc destination address
    pub btc_dest: Vec<u8>,
    /// amount in satoshis
    pub amount: u64,
    /// asset type
    pub asset_type: AssetType,
}
