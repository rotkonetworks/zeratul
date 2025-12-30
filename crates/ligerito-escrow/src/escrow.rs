//! 3-Party Escrow Protocol using Verifiable Secret Sharing
//!
//! This module implements a LocalCrypto-style P2P escrow system where:
//! - Buyer (Party A) and Seller (Party B) trade directly
//! - Arbitrator (Party C) can resolve disputes
//! - Any 2-of-3 parties can complete the transaction
//!
//! # Integration with FROST Multisigs
//!
//! This system is designed to work with FROST (Flexible Round-Optimized Schnorr
//! Threshold signatures) on Zcash and Penumbra:
//!
//! 1. Seller generates FROST key shares for 2-of-3 threshold
//! 2. Shares distributed to Buyer, Seller, Arbitrator
//! 3. Each party verifies their share using Ligerito commitment
//! 4. Funds locked to the threshold address
//! 5. Any 2 parties can collaboratively sign to release funds
//!
//! # Security Model
//!
//! - **Verifiable shares**: ZODA-style encoding prevents malicious dealer
//! - **2-of-3 threshold**: No single party can steal funds
//! - **Non-custodial arbitrator**: Arbitrator can only release to buyer OR seller
//! - **Cryptographic escrow**: Unlike LocalCryptos' scripts, this uses pure threshold crypto

#[cfg(not(feature = "std"))]
use alloc::{string::String, vec::Vec};

use crate::{EscrowError, Result};
use crate::shares::{Share, ShareSet, SecretSharer};
use sha2::{Sha256, Digest};

/// Escrow party identifier
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EscrowParty {
    /// The buyer (receives goods/services, pays fiat)
    Buyer,
    /// The seller (provides crypto, receives fiat)
    Seller,
    /// The arbitrator (resolves disputes)
    Arbitrator,
}

impl EscrowParty {
    /// Get the share index for this party
    pub fn share_index(&self) -> usize {
        match self {
            EscrowParty::Buyer => 0,
            EscrowParty::Seller => 1,
            EscrowParty::Arbitrator => 2,
        }
    }

    /// Get party from share index
    pub fn from_index(index: usize) -> Option<Self> {
        match index {
            0 => Some(EscrowParty::Buyer),
            1 => Some(EscrowParty::Seller),
            2 => Some(EscrowParty::Arbitrator),
            _ => None,
        }
    }
}

/// Current state of the escrow
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EscrowState {
    /// Escrow created but not yet funded
    Created,
    /// Funds locked, waiting for buyer payment
    Funded,
    /// Buyer marked payment as sent
    PaymentSent,
    /// Dispute raised, awaiting arbitration
    Disputed,
    /// Escrow released to buyer
    ReleasedToBuyer,
    /// Escrow returned to seller
    ReturnedToSeller,
    /// Escrow cancelled by mutual agreement
    Cancelled,
}

/// Escrow setup containing shares and metadata
#[derive(Clone)]
pub struct EscrowSetup {
    /// Unique escrow identifier
    pub escrow_id: [u8; 32],
    /// The secret shares (one per party)
    pub share_set: ShareSet,
    /// Current state
    pub state: EscrowState,
    /// Buyer's public key (for FROST)
    pub buyer_pubkey: Option<[u8; 32]>,
    /// Seller's public key (for FROST)
    pub seller_pubkey: Option<[u8; 32]>,
    /// Arbitrator's public key (for FROST)
    pub arbitrator_pubkey: Option<[u8; 32]>,
}

impl EscrowSetup {
    /// Create a new escrow with a generated secret
    ///
    /// The secret can be:
    /// - A FROST signing key share seed
    /// - A nonce for threshold signing
    /// - Any 32-byte secret needed for transaction completion
    pub fn new(escrow_id: [u8; 32], secret: &[u8; 32]) -> Result<Self> {
        let sharer = SecretSharer::new(2, 3)?;
        let share_set = sharer.share_secret(secret)?;

        Ok(Self {
            escrow_id,
            share_set,
            state: EscrowState::Created,
            buyer_pubkey: None,
            seller_pubkey: None,
            arbitrator_pubkey: None,
        })
    }

    /// Create escrow from existing share set
    pub fn from_shares(escrow_id: [u8; 32], share_set: ShareSet) -> Self {
        Self {
            escrow_id,
            share_set,
            state: EscrowState::Created,
            buyer_pubkey: None,
            seller_pubkey: None,
            arbitrator_pubkey: None,
        }
    }

    /// Get the share for a specific party
    pub fn get_share(&self, party: EscrowParty) -> Option<&Share> {
        self.share_set.get_share(party.share_index())
    }

    /// Verify a share received from the dealer
    pub fn verify_share(&self, share: &Share) -> Result<()> {
        self.share_set.verify_share(share)
    }

    /// Get the commitment (Merkle root) that all parties should agree on
    pub fn commitment(&self) -> [u8; 32] {
        self.share_set.commitment()
    }

    /// Set party public keys (for FROST integration)
    pub fn set_public_keys(
        &mut self,
        buyer: [u8; 32],
        seller: [u8; 32],
        arbitrator: [u8; 32],
    ) {
        self.buyer_pubkey = Some(buyer);
        self.seller_pubkey = Some(seller);
        self.arbitrator_pubkey = Some(arbitrator);
    }

    /// Transition to funded state
    pub fn mark_funded(&mut self) -> Result<()> {
        if self.state != EscrowState::Created {
            return Err(EscrowError::InvalidShare); // TODO: proper error
        }
        self.state = EscrowState::Funded;
        Ok(())
    }

    /// Buyer marks payment as sent
    pub fn mark_payment_sent(&mut self) -> Result<()> {
        if self.state != EscrowState::Funded {
            return Err(EscrowError::InvalidShare);
        }
        self.state = EscrowState::PaymentSent;
        Ok(())
    }

    /// Raise a dispute
    pub fn raise_dispute(&mut self) -> Result<()> {
        if self.state != EscrowState::PaymentSent {
            return Err(EscrowError::InvalidShare);
        }
        self.state = EscrowState::Disputed;
        Ok(())
    }
}

/// A share holder's view of the escrow
#[derive(Clone)]
pub struct EscrowParticipant {
    /// Which party this is
    pub party: EscrowParty,
    /// The escrow ID
    pub escrow_id: [u8; 32],
    /// This party's share
    pub share: Share,
    /// The commitment (shared by all parties)
    pub commitment: [u8; 32],
}

impl EscrowParticipant {
    /// Create a new participant from received share
    pub fn new(
        party: EscrowParty,
        escrow_id: [u8; 32],
        share: Share,
        commitment: [u8; 32],
    ) -> Self {
        Self {
            party,
            escrow_id,
            share,
            commitment,
        }
    }

    /// Verify that our share is consistent with the commitment
    pub fn verify_share(&self, share_set_for_verification: &ShareSet) -> Result<()> {
        share_set_for_verification.verify_share(&self.share)
    }

    /// Get share values (for combining with another party)
    pub fn share_values(&self) -> &[crate::ShareField] {
        &self.share.values
    }

    /// Get share index
    pub fn share_index(&self) -> u32 {
        self.share.index
    }
}

/// Resolution outcome
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Resolution {
    /// Release funds to buyer
    ReleaseToBuyer,
    /// Return funds to seller
    ReturnToSeller,
}

/// Combine shares from two parties to reconstruct the secret
///
/// This is the core operation for releasing/returning escrow:
/// - Happy path: Buyer + Seller shares
/// - Dispute (buyer wins): Buyer + Arbitrator shares
/// - Dispute (seller wins): Seller + Arbitrator shares
pub fn combine_shares(
    share1: &Share,
    share2: &Share,
) -> Result<[u8; 32]> {
    crate::reconstruct_secret(&[share1.clone(), share2.clone()], 2)
}

/// Generate escrow ID from trade parameters
pub fn generate_escrow_id(
    trade_id: &[u8],
    buyer_pubkey: &[u8; 32],
    seller_pubkey: &[u8; 32],
    amount: u64,
) -> [u8; 32] {
    let mut hasher = Sha256::new();
    hasher.update(b"ligerito-escrow-v1");
    hasher.update(trade_id);
    hasher.update(buyer_pubkey);
    hasher.update(seller_pubkey);
    hasher.update(&amount.to_le_bytes());
    hasher.finalize().into()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::reconstruct_secret;

    #[test]
    fn test_full_escrow_flow_happy_path() {
        // Setup: Seller creates escrow
        let secret = [42u8; 32]; // This would be FROST key material
        let escrow_id = generate_escrow_id(
            b"trade-123",
            &[1u8; 32], // buyer pubkey
            &[2u8; 32], // seller pubkey
            1_000_000, // amount
        );

        let mut escrow = EscrowSetup::new(escrow_id, &secret).unwrap();

        // Distribute shares
        let buyer_share = escrow.get_share(EscrowParty::Buyer).unwrap().clone();
        let seller_share = escrow.get_share(EscrowParty::Seller).unwrap().clone();
        let _arb_share = escrow.get_share(EscrowParty::Arbitrator).unwrap().clone();

        // Each party verifies their share
        assert!(escrow.verify_share(&buyer_share).is_ok());
        assert!(escrow.verify_share(&seller_share).is_ok());

        // State transitions
        escrow.mark_funded().unwrap();
        assert_eq!(escrow.state, EscrowState::Funded);

        escrow.mark_payment_sent().unwrap();
        assert_eq!(escrow.state, EscrowState::PaymentSent);

        // Happy path: Seller releases by sharing their share with buyer
        let reconstructed = combine_shares(&buyer_share, &seller_share).unwrap();
        assert_eq!(reconstructed, secret);
    }

    #[test]
    fn test_escrow_dispute_buyer_wins() {
        let secret = [0xABu8; 32];
        let escrow_id = [0u8; 32];

        let escrow = EscrowSetup::new(escrow_id, &secret).unwrap();

        let buyer_share = escrow.get_share(EscrowParty::Buyer).unwrap().clone();
        let arb_share = escrow.get_share(EscrowParty::Arbitrator).unwrap().clone();

        // Arbitrator sides with buyer
        let reconstructed = combine_shares(&buyer_share, &arb_share).unwrap();
        assert_eq!(reconstructed, secret);
    }

    #[test]
    fn test_escrow_dispute_seller_wins() {
        let secret = [0xCDu8; 32];
        let escrow_id = [0u8; 32];

        let escrow = EscrowSetup::new(escrow_id, &secret).unwrap();

        let seller_share = escrow.get_share(EscrowParty::Seller).unwrap().clone();
        let arb_share = escrow.get_share(EscrowParty::Arbitrator).unwrap().clone();

        // Arbitrator sides with seller
        let reconstructed = combine_shares(&seller_share, &arb_share).unwrap();
        assert_eq!(reconstructed, secret);
    }

    #[test]
    fn test_arbitrator_cannot_act_alone() {
        let secret = [0xEFu8; 32];
        let escrow_id = [0u8; 32];

        let escrow = EscrowSetup::new(escrow_id, &secret).unwrap();
        let arb_share = escrow.get_share(EscrowParty::Arbitrator).unwrap().clone();

        // Single share should fail
        let result = reconstruct_secret(&[arb_share], 2);
        assert!(matches!(result, Err(EscrowError::InsufficientShares { .. })));
    }

    #[test]
    fn test_share_verification_prevents_tampering() {
        let secret = [0x11u8; 32];
        let escrow_id = [0u8; 32];

        let escrow = EscrowSetup::new(escrow_id, &secret).unwrap();

        // Create tampered share
        let mut bad_share = escrow.get_share(EscrowParty::Buyer).unwrap().clone();
        bad_share.values[0] = crate::ShareField::from(999u32);

        // Verification should fail
        assert!(escrow.verify_share(&bad_share).is_err());
    }

    #[test]
    fn test_escrow_id_generation() {
        let id1 = generate_escrow_id(
            b"trade-1",
            &[1u8; 32],
            &[2u8; 32],
            100,
        );

        let id2 = generate_escrow_id(
            b"trade-2",
            &[1u8; 32],
            &[2u8; 32],
            100,
        );

        // Different trade IDs should give different escrow IDs
        assert_ne!(id1, id2);

        // Same parameters should give same ID (deterministic)
        let id1_again = generate_escrow_id(
            b"trade-1",
            &[1u8; 32],
            &[2u8; 32],
            100,
        );
        assert_eq!(id1, id1_again);
    }
}
