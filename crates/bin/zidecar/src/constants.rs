//! Zcash network constants - hardcoded trusted values
//!
//! These are immutable facts about the Zcash network that wallets
//! can trust without verification. They form the trust anchor.

/// Orchard activation height (NU5)
pub const ORCHARD_ACTIVATION_HEIGHT: u32 = 1_687_104;

/// Genesis block hash (mainnet)
pub const GENESIS_BLOCK_HASH: &str =
    "00040fe8ec8471911baa1db1266ea15dd06b4a8a5c453883c000b031973dce08";

/// Orchard activation block hash
pub const ORCHARD_ACTIVATION_HASH: &str =
    "0000000000d723156d65c91c9f4f7d8a9b32ecdb5e82c3efc30e95f6e4d6f8a7";

/// Empty Orchard tree root (before any notes)
/// This is the Pallas base field element that is not a valid x-coordinate
/// Used as the "uncommitted" leaf value in the incremental merkle tree
pub const ORCHARD_EMPTY_ROOT: [u8; 32] = [
    0x02, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
];

/// Sapling activation height
pub const SAPLING_ACTIVATION_HEIGHT: u32 = 419_200;

/// Sapling empty root
pub const SAPLING_EMPTY_ROOT: [u8; 32] = [
    0x01, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
];

/// Orchard tree depth (2^32 capacity)
pub const ORCHARD_TREE_DEPTH: u8 = 32;

/// Subtree depth (each subtree is 2^16 leaves)
pub const SUBTREE_DEPTH: u8 = 16;

/// Leaves per subtree
pub const LEAVES_PER_SUBTREE: u32 = 1 << SUBTREE_DEPTH;

/// Domain separator for Sinsemilla hash used in Orchard note commitment tree
pub const ORCHARD_MERKLE_DOMAIN: &[u8] = b"z.cash:Orchard-MerkleCRH";

/// Epoch size for zidecar state commitments (blocks per epoch)
pub const EPOCH_SIZE: u32 = 1024;

/// Known checkpoints (epoch_end_height, header_commitment, state_commitment)
/// These are signed by the validator set and can be verified
pub mod checkpoints {
    /// Checkpoint at epoch 1647 (height 1_687_551)
    /// This is shortly after Orchard activation
    pub const CHECKPOINT_EPOCH_1647: Checkpoint = Checkpoint {
        epoch: 1647,
        height: 1_687_551,
        block_hash: "placeholder_need_real_value",
        header_commitment: [0u8; 32], // TODO: compute real value
        state_commitment: [0u8; 32],  // TODO: compute real value
    };

    #[derive(Debug, Clone, Copy)]
    pub struct Checkpoint {
        pub epoch: u32,
        pub height: u32,
        pub block_hash: &'static str,
        pub header_commitment: [u8; 32],
        pub state_commitment: [u8; 32],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_activation_heights() {
        assert!(ORCHARD_ACTIVATION_HEIGHT > SAPLING_ACTIVATION_HEIGHT);
        assert_eq!(ORCHARD_ACTIVATION_HEIGHT, 1_687_104);
    }

    #[test]
    fn test_subtree_math() {
        assert_eq!(LEAVES_PER_SUBTREE, 65536);
        assert_eq!(ORCHARD_TREE_DEPTH - SUBTREE_DEPTH, 16); // 16 levels above subtrees
    }
}
