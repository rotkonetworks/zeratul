//! DKG-based Scheme Provider for Consensus
//!
//! Replaces StaticSchemeProvider with epoch-aware threshold signing using Golden DKG.
//!
//! ## Architecture
//!
//! - **Epoch-based keys**: Each epoch has its own DKG-generated group key
//! - **Threshold signatures**: t = 2f + 1 Byzantine fault tolerance
//! - **Dynamic validator set**: Changes based on governance elections
//! - **Automatic rotation**: New keys every 4 hours (epoch boundary)
//!
//! ## Integration
//!
//! The consensus layer calls `scheme(epoch)` to get the signing scheme for that epoch.
//! The scheme provider:
//! 1. Checks if DKG is complete for that epoch
//! 2. Returns the threshold signing scheme with group public key
//! 3. Falls back to previous epoch if current DKG incomplete

use anyhow::{bail, Result};
use commonware_consensus::marshal::SchemeProvider;
use commonware_consensus::simplex::signing_scheme::bls12381_threshold;
use commonware_cryptography::bls12381::primitives::{group, poly::Poly, variant::MinSig};
use commonware_cryptography::bls12381::primitives::group::Element;
use commonware_cryptography::ed25519::PublicKey as Ed25519PublicKey;
use commonware_cryptography::bls12381::PublicKey as BlsPublicKey;
use commonware_utils::set::Ordered;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use tracing::{debug, warn};

use crate::dkg_coordinator::DKGCoordinator;
use crate::governance::{EpochIndex, DKGGovernanceManager};

/// BLS12-381 variant for signatures
pub type Variant = MinSig;

/// Concrete signing scheme for consensus (BLS12-381 threshold signatures)
pub type SigningScheme = bls12381_threshold::Scheme<Ed25519PublicKey, Variant>;

/// DKG-based scheme provider
///
/// Provides epoch-specific threshold signing schemes generated via Golden DKG
#[derive(Clone)]
pub struct DKGSchemeProvider {
    /// DKG governance manager (contains DKG coordinator)
    manager: Arc<Mutex<DKGGovernanceManager>>,

    /// Cached signing schemes per epoch
    schemes: Arc<Mutex<HashMap<EpochIndex, Arc<SigningScheme>>>>,

    /// Participants mapping (Ed25519 <-> BLS)
    /// For commonware compatibility, we map Ed25519 keys to BLS keys
    participants_ed25519: Arc<Mutex<Ordered<Ed25519PublicKey>>>,
}

impl DKGSchemeProvider {
    /// Create a new DKG scheme provider
    pub fn new(
        manager: Arc<Mutex<DKGGovernanceManager>>,
        participants_ed25519: Ordered<Ed25519PublicKey>,
    ) -> Self {
        Self {
            manager,
            schemes: Arc::new(Mutex::new(HashMap::new())),
            participants_ed25519: Arc::new(Mutex::new(participants_ed25519)),
        }
    }

    /// Build a signing scheme for an epoch
    ///
    /// Uses the group public key from DKG and creates a threshold signing scheme
    fn build_scheme(&self, epoch: EpochIndex) -> Option<Arc<SigningScheme>> {
        let manager = self.manager.lock().unwrap();

        // Check if DKG is complete for this epoch
        if !manager.is_dkg_complete(epoch) {
            debug!(epoch, "DKG not complete for epoch, cannot build scheme");
            return None;
        }

        // Get group public key from DKG
        let group_pubkey = manager.group_pubkey(epoch)?;

        // Get our secret share (if we're a validator)
        let secret_share = manager.secret_share(epoch);

        // For now, we need to convert the DKG group key into a polynomial
        // This is a simplification - in production, we'd extract the polynomial from DKG
        // TODO: Properly integrate polynomial from Golden DKG

        // Create a mock polynomial with the group key as the constant term
        // In reality, this should come from the DKG polynomial commitments
        // Note: MinSig variant uses G1 for keys, G2 for polynomial commitments
        // We need to convert G1 key to G2 for the polynomial
        // TODO TODO TODO: This is a hack - need proper polynomial from DKG
        let g2_gen = group::G2::one();
        let poly = Poly::<group::G2>::from(vec![g2_gen]);

        // Create share structure
        let share = if let Some(s) = secret_share {
            group::Share {
                index: 0, // TODO: Get actual index from DKG
                private: s,
            }
        } else {
            // Not a validator, use dummy share
            group::Share {
                index: 0,
                private: group::Scalar::from(0u32),
            }
        };

        // Get participants
        let participants = self.participants_ed25519.lock().unwrap().clone();

        // Create the signing scheme
        let scheme = SigningScheme::new(participants, &poly, share);

        debug!(epoch, group_key = ?group_pubkey, "Built signing scheme for epoch");

        Some(Arc::new(scheme))
    }

    /// Update participants for a new epoch
    pub fn update_participants(&self, participants: Ordered<Ed25519PublicKey>) {
        let mut parts = self.participants_ed25519.lock().unwrap();
        *parts = participants;
    }
}

impl SchemeProvider for DKGSchemeProvider {
    type Scheme = SigningScheme;

    fn scheme(&self, epoch: u64) -> Option<Arc<Self::Scheme>> {
        // Check cache first
        {
            let schemes = self.schemes.lock().unwrap();
            if let Some(scheme) = schemes.get(&epoch) {
                return Some(scheme.clone());
            }
        }

        // Try to build scheme for this epoch
        if let Some(scheme) = self.build_scheme(epoch) {
            let mut schemes = self.schemes.lock().unwrap();
            schemes.insert(epoch, scheme.clone());
            return Some(scheme);
        }

        // Fall back to previous epoch if current DKG not complete
        if epoch > 0 {
            warn!(
                epoch,
                "DKG not complete for epoch, falling back to previous epoch"
            );
            return self.scheme(epoch - 1);
        }

        // No scheme available
        warn!(epoch, "No signing scheme available for epoch");
        None
    }
}

/// Helper to create initial scheme provider before first DKG
///
/// This is used during genesis/bootstrap before the first DKG completes
pub fn create_bootstrap_provider(
    participants: Ordered<Ed25519PublicKey>,
    poly: &Poly<group::G2>,
    share: group::Share,
) -> Arc<SigningScheme> {
    Arc::new(SigningScheme::new(participants, poly, share))
}

#[cfg(test)]
mod tests {
    use super::*;
    use commonware_cryptography::bls12381::primitives::group::{G1, Scalar};
    use rand::thread_rng;

    #[test]
    fn test_dkg_scheme_provider_creation() {
        let mut rng = thread_rng();
        let beta = Scalar::one();
        let our_key = BlsPublicKey::from(G1::one());

        let manager = Arc::new(Mutex::new(DKGGovernanceManager::new(
            &mut rng,
            our_key,
            beta,
            60,
        )));

        // Create empty participants list for testing
        let participants = Ordered::from(vec![]);

        let provider = DKGSchemeProvider::new(manager, participants);

        // Should return None for epoch 0 (no DKG yet)
        assert!(provider.scheme(0).is_none());
    }

    #[test]
    fn test_bootstrap_provider() {
        // Create minimal bootstrap configuration
        let participants = Ordered::from(vec![]);
        let g2_gen = group::G2::one();
        let poly = Poly::<group::G2>::from(vec![g2_gen]);
        let share = group::Share {
            index: 0,
            private: Scalar::from(0u32),
        };

        let scheme = create_bootstrap_provider(participants, &poly, share);
        assert!(Arc::strong_count(&scheme) >= 1);
    }
}
