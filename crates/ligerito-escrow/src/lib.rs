//! Verifiable Secret Sharing and 3-Party Escrow using ZODA/Ligerito
//!
//! This crate implements verifiable Shamir secret sharing using the insight from
//! ZODA (Zero-Overhead Data Availability) that Reed-Solomon codewords ARE Shamir shares.
//!
//! Key insight from Guillermo Angeris:
//! > "ZODA encoding is a form of verifiable secret sharing: the shards ARE Shamir shares,
//! > and verification prevents a malicious dealer from giving inconsistent shares."
//!
//! # Use Case: LocalCrypto-style P2P Escrow
//!
//! A 2-of-3 threshold escrow where:
//! - Buyer (Party A) holds share A
//! - Seller (Party B) holds share B
//! - Arbitrator (Party C) holds share C
//!
//! Any 2 parties can reconstruct the secret and complete the transaction:
//! - Happy path: Buyer + Seller
//! - Dispute won by buyer: Buyer + Arbitrator
//! - Dispute won by seller: Seller + Arbitrator
//!
//! # Security Guarantees
//!
//! - **Share Verification**: Each party can verify their share is consistent with
//!   the polynomial commitment before accepting it
//! - **Threshold Security**: k-1 shares reveal nothing about the secret
//! - **Dealer Honesty**: Verification catches malicious share distribution

#![cfg_attr(not(feature = "std"), no_std)]

#[cfg(not(feature = "std"))]
extern crate alloc;

#[cfg(not(feature = "std"))]
use alloc::{vec, vec::Vec, string::String};

pub mod shares;
pub mod escrow;
pub mod reconstruct;
pub mod frost;

pub use shares::{Share, ShareSet, SecretSharer};
pub use escrow::{EscrowSetup, EscrowParty, EscrowState};
pub use reconstruct::reconstruct_secret;

use ligerito_binary_fields::{BinaryElem32, BinaryElem128};

/// Error types for escrow operations
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EscrowError {
    /// Share verification failed against commitment
    InvalidShare,
    /// Not enough shares to reconstruct (need k of n)
    InsufficientShares { have: usize, need: usize },
    /// Share indices must be distinct
    DuplicateShareIndex,
    /// Threshold must be >= 2 and <= n
    InvalidThreshold,
    /// Secret too large for field
    SecretTooLarge,
    /// Merkle proof verification failed
    MerkleProofInvalid,
    /// Share index out of bounds
    IndexOutOfBounds,
}

#[cfg(feature = "std")]
impl std::error::Error for EscrowError {}

impl core::fmt::Display for EscrowError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            EscrowError::InvalidShare => write!(f, "Share verification failed"),
            EscrowError::InsufficientShares { have, need } => {
                write!(f, "Need {} shares to reconstruct, only have {}", need, have)
            }
            EscrowError::DuplicateShareIndex => write!(f, "Share indices must be distinct"),
            EscrowError::InvalidThreshold => write!(f, "Invalid threshold parameters"),
            EscrowError::SecretTooLarge => write!(f, "Secret too large for field"),
            EscrowError::MerkleProofInvalid => write!(f, "Merkle proof verification failed"),
            EscrowError::IndexOutOfBounds => write!(f, "Share index out of bounds"),
        }
    }
}

pub type Result<T> = core::result::Result<T, EscrowError>;

/// Field element type used for secret sharing
/// Using BinaryElem32 for 32-byte secrets (8 field elements)
pub type ShareField = BinaryElem32;

/// Larger field for commitments
pub type CommitField = BinaryElem128;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_basic_2_of_3() {
        let secret = [42u8; 32]; // 32-byte secret (like a private key)

        let sharer = SecretSharer::new(2, 3).unwrap();
        let share_set = sharer.share_secret(&secret).unwrap();

        // Verify all shares
        for share in share_set.shares() {
            assert!(share_set.verify_share(share).is_ok());
        }

        // Reconstruct with shares 0 and 1
        let reconstructed = reconstruct_secret(
            &[share_set.shares()[0].clone(), share_set.shares()[1].clone()],
            2,
        ).unwrap();

        assert_eq!(reconstructed, secret);
    }

    #[test]
    fn test_2_of_3_all_pairs() {
        let secret: [u8; 32] = [
            0x74, 0x68, 0x69, 0x73, 0x20, 0x69, 0x73, 0x20, // "this is "
            0x61, 0x20, 0x33, 0x32, 0x20, 0x62, 0x79, 0x74, // "a 32 byt"
            0x65, 0x20, 0x73, 0x65, 0x63, 0x72, 0x65, 0x74, // "e secret"
            0x20, 0x6b, 0x65, 0x79, 0x21, 0x21, 0x21, 0x21, // " key!!!!"
        ];

        let sharer = SecretSharer::new(2, 3).unwrap();
        let share_set = sharer.share_secret(&secret).unwrap();

        let shares = share_set.shares();

        // All 3 pairs should reconstruct correctly
        let pairs = [(0, 1), (0, 2), (1, 2)];
        for (i, j) in pairs {
            let reconstructed = reconstruct_secret(
                &[shares[i].clone(), shares[j].clone()],
                2,
            ).unwrap();
            assert_eq!(reconstructed, secret, "Pair ({}, {}) failed", i, j);
        }
    }

    #[test]
    fn test_insufficient_shares() {
        let secret = [0u8; 32];

        let sharer = SecretSharer::new(2, 3).unwrap();
        let share_set = sharer.share_secret(&secret).unwrap();

        // Only 1 share - should fail
        let result = reconstruct_secret(
            &[share_set.shares()[0].clone()],
            2,
        );

        assert!(matches!(result, Err(EscrowError::InsufficientShares { .. })));
    }
}
