//! Types for Zcash light client pallet

use codec::{Decode, Encode, MaxEncodedLen};
use frame_support::pallet_prelude::*;
use scale_info::TypeInfo;

/// Information about a registered relayer
#[derive(Clone, Encode, Decode, TypeInfo, MaxEncodedLen, Debug, PartialEq)]
pub struct RelayerInfo<Balance, BlockNumber> {
    /// Staked amount
    pub stake: Balance,
    /// Block when registered
    pub registered_at: BlockNumber,
    /// Total attestations submitted
    pub total_attestations: u64,
    /// Attestations that were finalized successfully
    pub successful_attestations: u64,
    /// Whether relayer has been slashed
    pub slashed: bool,
}

/// Per-block attestation (simplified from epoch)
#[derive(Clone, Encode, Decode, TypeInfo, MaxEncodedLen, Debug, PartialEq)]
pub struct BlockAttestation {
    /// Zcash block height
    pub height: u32,
    /// Block hash
    pub block_hash: [u8; 32],
    /// Previous block hash (for chain verification)
    pub prev_hash: [u8; 32],
    /// Orchard note commitment tree root
    pub orchard_root: [u8; 32],
    /// Sapling note commitment tree root
    pub sapling_root: [u8; 32],
}

/// Finalized block (after threshold attestations)
#[derive(Clone, Encode, Decode, TypeInfo, MaxEncodedLen, Debug, PartialEq)]
pub struct FinalizedBlock {
    /// Block hash
    pub block_hash: [u8; 32],
    /// Previous block hash
    pub prev_hash: [u8; 32],
    /// Orchard tree root
    pub orchard_root: [u8; 32],
    /// Sapling tree root
    pub sapling_root: [u8; 32],
    /// Number of relayers who attested
    pub attester_count: u32,
    /// Parachain block when finalized
    pub finalized_at: u32,
}

/// Active challenge against an attestation
#[derive(Clone, Encode, Decode, TypeInfo, MaxEncodedLen, Debug, PartialEq)]
pub struct Challenge<AccountId, Balance> {
    /// Who submitted the challenge
    pub challenger: AccountId,
    /// Challenged relayer
    pub relayer: AccountId,
    /// Zcash height being challenged
    pub zcash_height: u32,
    /// Claimed correct block hash
    pub correct_block_hash: [u8; 32],
    /// Challenge bond (slashed if frivolous)
    pub bond: Balance,
    /// Status
    pub resolved: bool,
}

/// Bridge deposit from Zcash
#[derive(Clone, Encode, Decode, TypeInfo, MaxEncodedLen, Debug, PartialEq)]
pub struct BridgeDeposit<AccountId> {
    /// Zcash transaction ID
    pub zcash_txid: [u8; 32],
    /// Block height of deposit tx
    pub zcash_height: u32,
    /// Amount in zatoshi (1 ZEC = 10^8 zatoshi)
    pub amount_zatoshi: u64,
    /// Polkadot recipient
    pub recipient: AccountId,
    /// Number of relayer attestations
    pub attestations: u32,
    /// Whether threshold reached and minted
    pub finalized: bool,
}

/// Bridge withdrawal to Zcash
#[derive(Clone, Encode, Decode, TypeInfo, MaxEncodedLen, Debug, PartialEq)]
pub struct BridgeWithdrawal<AccountId> {
    /// Who is withdrawing
    pub who: AccountId,
    /// Amount in zatoshi
    pub amount_zatoshi: u64,
    /// Zcash destination address (t-addr or z-addr)
    pub zcash_address: BoundedVec<u8, ConstU32<512>>,
    /// Zcash tx ID once processed
    pub zcash_txid: Option<[u8; 32]>,
    /// Status
    pub processed: bool,
}

/// Proof anchor: trusted starting point for header chain proofs
/// Set to a known finalized checkpoint (e.g., Orchard activation)
#[derive(Clone, Encode, Decode, TypeInfo, MaxEncodedLen, Debug, PartialEq)]
pub struct ProofAnchorData {
    /// Zcash block height of anchor
    pub height: u32,
    /// Block hash at anchor
    pub block_hash: [u8; 32],
    /// Header commitment at anchor (for ligerito proof verification)
    pub header_commitment: [u8; 32],
}

// Keep old types for backward compat during migration
pub type EpochAttestation = BlockAttestation;
pub type FinalizedEpoch = FinalizedBlock;

/// Zcash network constants
pub mod constants {
    /// Orchard activation height (mainnet)
    pub const ORCHARD_ACTIVATION_HEIGHT: u32 = 1_687_104;

    /// Zcash block time in seconds
    pub const ZCASH_BLOCK_TIME_SECS: u32 = 75;

    /// Mainnet genesis block hash
    pub const GENESIS_HASH: [u8; 32] = [
        0x00, 0x04, 0x0f, 0xe8, 0xec, 0x84, 0x71, 0x91,
        0x1b, 0xaa, 0x1d, 0xb1, 0x26, 0x6e, 0xa1, 0x5d,
        0xd0, 0x6b, 0x4a, 0x8a, 0x5c, 0x45, 0x38, 0x83,
        0xc0, 0x00, 0xb0, 0x31, 0x97, 0x3d, 0xce, 0x08,
    ];

    /// Zatoshi per ZEC
    pub const ZATOSHI_PER_ZEC: u64 = 100_000_000;
}
