//! DKG Coordinator - Epoch-based Distributed Key Generation
//!
//! Coordinates Golden DKG protocol at epoch boundaries to generate threshold signing keys.
//! Integrates with governance module for validator selection and slashing.
//!
//! ## Architecture
//!
//! At each epoch transition:
//! 1. Governance selects active validators via Phragmén election
//! 2. Selected validators participate in Golden DKG
//! 3. DKG produces group public key + individual secret shares
//! 4. Group key used for threshold signatures during the epoch
//! 5. Non-participating validators are slashed
//!
//! ## Golden DKG Properties
//!
//! - **Non-interactive**: One round of broadcast (no coordinator needed)
//! - **Fast**: 223kb communication vs 27.8MB traditional DKG (50 participants)
//! - **Threshold security**: t = 2f + 1 (Byzantine fault tolerance)
//! - **Public verifiability**: Anyone can verify shares are valid

use anyhow::{bail, Result};
use commonware_cryptography::bls12381::primitives::group::{Element, Scalar, G1};
use commonware_cryptography::bls12381::primitives::ops::{
    sign_message, threshold_signature_recover,
};
use commonware_cryptography::bls12381::primitives::poly::Eval;
use commonware_cryptography::bls12381::primitives::variant::MinPk;
use commonware_cryptography::bls12381::PublicKey;
use commonware_utils::set::Ordered;
use commonware_utils::quorum;
use golden_rs::dkg::broadcast::BroadcastMsg;
use golden_rs::dkg::participant::{evrf::EVRF, registry::Registry, Participant};
use rand_core::CryptoRngCore;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};
use tracing::{debug, error, info, warn};

use crate::governance::{AccountId, ElectionResult};

/// DKG state for a single epoch
#[derive(Clone)]
pub struct EpochDKG {
    /// The epoch number
    pub epoch: u64,

    /// Local participant (if we're a validator this epoch)
    pub participant: Option<Participant>,

    /// Registry of broadcast messages received
    pub received_broadcasts: HashMap<PublicKey, BroadcastMsg>,

    /// Ordered list of validator public keys
    pub validators: Ordered<PublicKey>,

    /// Our index in the validator set (if we're a validator)
    pub our_index: Option<u32>,

    /// Whether DKG has completed
    pub completed: bool,

    /// Group public key (available after DKG completes)
    pub group_pubkey: Option<PublicKey>,

    /// Our secret share (if we're a validator)
    pub secret_share: Option<Scalar>,

    /// Public key shares of all validators
    pub pubkey_shares: Option<HashMap<u32, G1>>,

    /// Threshold for signatures
    pub threshold: u32,
}

impl EpochDKG {
    /// Create a new DKG instance for an epoch
    pub fn new(
        epoch: u64,
        election_result: &ElectionResult,
        our_pubkey: &PublicKey,
        evrf: EVRF,
    ) -> Self {
        // Convert elected validators to BLS public keys
        let validators: Vec<PublicKey> = election_result
            .validators
            .iter()
            .map(|v| {
                // TODO: Map AccountId to BLS PublicKey
                // For now, we'll need to maintain a registry
                PublicKey::from(G1::one()) // Placeholder
            })
            .collect();

        let validators = Ordered::from(validators);
        let threshold = quorum(validators.len() as u32);

        // Find our index if we're a validator
        let our_index = validators
            .iter()
            .position(|pk| pk == our_pubkey)
            .map(|i| i as u32);

        let participant = if our_index.is_some() {
            Some(Participant::new(evrf, Registry::default()))
        } else {
            None
        };

        Self {
            epoch,
            participant,
            received_broadcasts: HashMap::new(),
            validators,
            our_index,
            completed: false,
            group_pubkey: None,
            secret_share: None,
            pubkey_shares: None,
            threshold,
        }
    }

    /// Generate our broadcast message (if we're a validator)
    pub fn generate_broadcast<R: CryptoRngCore>(
        &mut self,
        rng: &mut R,
    ) -> Result<Option<BroadcastMsg>> {
        let Some(ref mut participant) = self.participant else {
            return Ok(None);
        };

        let bmsg = participant.generate_bmsg(rng, self.validators.clone());

        // Process our own broadcast immediately
        let our_pk = participant.pk_i().clone();
        let our_idx = self.our_index.expect("Must have index if we have participant");
        participant.on_incoming_bmsg(&our_pk, our_idx, bmsg.clone(), &self.validators)?;

        Ok(Some(bmsg))
    }

    /// Process incoming broadcast from a validator
    pub fn on_broadcast(
        &mut self,
        sender: PublicKey,
        bmsg: BroadcastMsg,
    ) -> Result<()> {
        // Check if we've already received from this sender
        if self.received_broadcasts.contains_key(&sender) {
            return Ok(());
        }

        // Validate sender is in validator set
        let Some(sender_idx) = self.validators.iter().position(|pk| pk == &sender) else {
            bail!("Broadcast from non-validator: {}", sender);
        };

        // Store the broadcast
        self.received_broadcasts.insert(sender.clone(), bmsg.clone());

        // If we're a validator, process it
        if let Some(ref mut participant) = self.participant {
            let our_idx = self.our_index.expect("Must have index if we have participant");
            participant.on_incoming_bmsg(&sender, our_idx, bmsg, &self.validators)?;

            // Check if DKG is complete
            if participant.is_ready() {
                self.completed = true;
                self.group_pubkey = participant.get_group_pubkey();
                self.secret_share = participant.get_share().cloned();
                self.pubkey_shares = participant.pubkey_shares().cloned();

                info!(
                    epoch = self.epoch,
                    group_key = ?self.group_pubkey,
                    "DKG completed successfully"
                );
            }
        }

        Ok(())
    }

    /// Check if we have enough broadcasts to complete DKG
    pub fn progress(&self) -> (usize, usize) {
        (self.received_broadcasts.len(), self.validators.len())
    }

    /// Get validators who haven't broadcast yet (for slashing)
    pub fn missing_validators(&self) -> Vec<PublicKey> {
        self.validators
            .iter()
            .filter(|pk| !self.received_broadcasts.contains_key(pk))
            .cloned()
            .collect()
    }
}

/// DKG Coordinator - manages DKG across epochs
pub struct DKGCoordinator {
    /// Current epoch
    current_epoch: Arc<Mutex<u64>>,

    /// DKG state for each epoch
    epoch_dkgs: Arc<Mutex<HashMap<u64, EpochDKG>>>,

    /// Our BLS public key
    our_pubkey: PublicKey,

    /// Our EVRF instance (persists across epochs)
    evrf: EVRF,

    /// Beta parameter for EVRF (shared across all validators)
    beta: Scalar,
}

impl DKGCoordinator {
    /// Create a new DKG coordinator
    pub fn new<R: CryptoRngCore>(
        rng: &mut R,
        our_pubkey: PublicKey,
        beta: Scalar,
    ) -> Self {
        let evrf = EVRF::random(rng, beta.clone());

        Self {
            current_epoch: Arc::new(Mutex::new(0)),
            epoch_dkgs: Arc::new(Mutex::new(HashMap::new())),
            our_pubkey,
            evrf,
            beta,
        }
    }

    /// Start DKG for a new epoch
    pub fn start_epoch<R: CryptoRngCore>(
        &mut self,
        rng: &mut R,
        epoch: u64,
        election_result: ElectionResult,
    ) -> Result<Option<BroadcastMsg>> {
        let mut current = self.current_epoch.lock().unwrap();
        *current = epoch;

        let mut epoch_dkg = EpochDKG::new(
            epoch,
            &election_result,
            &self.our_pubkey,
            self.evrf.clone(),
        );

        // Generate our broadcast if we're a validator
        let bmsg = epoch_dkg.generate_broadcast(rng)?;

        // Store the DKG state
        let mut dkgs = self.epoch_dkgs.lock().unwrap();
        dkgs.insert(epoch, epoch_dkg);

        info!(
            epoch,
            num_validators = election_result.validators.len(),
            participating = bmsg.is_some(),
            "Started new epoch DKG"
        );

        Ok(bmsg)
    }

    /// Process incoming broadcast for an epoch
    pub fn on_broadcast(
        &self,
        epoch: u64,
        sender: PublicKey,
        bmsg: BroadcastMsg,
    ) -> Result<()> {
        let mut dkgs = self.epoch_dkgs.lock().unwrap();
        let Some(dkg) = dkgs.get_mut(&epoch) else {
            bail!("No DKG state for epoch {}", epoch);
        };

        dkg.on_broadcast(sender, bmsg)
    }

    /// Get the group public key for an epoch (if DKG completed)
    pub fn group_pubkey(&self, epoch: u64) -> Option<PublicKey> {
        let dkgs = self.epoch_dkgs.lock().unwrap();
        dkgs.get(&epoch)?.group_pubkey.clone()
    }

    /// Get our secret share for an epoch (if we're a validator and DKG completed)
    pub fn secret_share(&self, epoch: u64) -> Option<Scalar> {
        let dkgs = self.epoch_dkgs.lock().unwrap();
        dkgs.get(&epoch)?.secret_share.clone()
    }

    /// Check if DKG is complete for an epoch
    pub fn is_complete(&self, epoch: u64) -> bool {
        let dkgs = self.epoch_dkgs.lock().unwrap();
        dkgs.get(&epoch).map_or(false, |dkg| dkg.completed)
    }

    /// Get validators who haven't participated (for slashing)
    pub fn missing_validators(&self, epoch: u64) -> Vec<PublicKey> {
        let dkgs = self.epoch_dkgs.lock().unwrap();
        dkgs.get(&epoch)
            .map_or(Vec::new(), |dkg| dkg.missing_validators())
    }

    /// Sign a message with our secret share (partial signature)
    pub fn partial_sign(&self, epoch: u64, message: &[u8]) -> Option<Eval<commonware_cryptography::bls12381::primitives::group::G2>> {
        let dkgs = self.epoch_dkgs.lock().unwrap();
        let dkg = dkgs.get(&epoch)?;
        let share = dkg.secret_share.as_ref()?;
        let index = dkg.our_index?;

        let signature = sign_message::<MinPk>(share, None, message);
        Some(Eval {
            index,
            value: signature,
        })
    }

    /// Recover threshold signature from partial signatures
    pub fn recover_signature(
        &self,
        epoch: u64,
        partials: Vec<Eval<commonware_cryptography::bls12381::primitives::group::G2>>,
    ) -> Result<commonware_cryptography::bls12381::primitives::group::G2> {
        let dkgs = self.epoch_dkgs.lock().unwrap();
        let dkg = dkgs.get(&epoch).ok_or_else(|| anyhow::anyhow!("No DKG for epoch {}", epoch))?;

        let sig = threshold_signature_recover::<MinPk, _>(dkg.threshold, &partials)?;
        Ok(sig)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rand::thread_rng;

    #[test]
    fn test_dkg_coordinator() {
        let mut rng = thread_rng();
        let beta = Scalar::one();

        // Create mock election result
        let election = ElectionResult {
            era: 0,
            validators: vec![],
            total_stake: 0,
            min_backing: 0,
            max_backing: 0,
            avg_backing: 0,
        };

        let mut coordinator = DKGCoordinator::new(
            &mut rng,
            PublicKey::from(G1::one()),
            beta,
        );

        // Start epoch
        let bmsg = coordinator.start_epoch(&mut rng, 1, election).unwrap();
        assert!(bmsg.is_none()); // No validators in mock election
    }
}
