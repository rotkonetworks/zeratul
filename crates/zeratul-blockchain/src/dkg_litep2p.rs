//! DKG Protocol over litep2p
//!
//! Implements Golden DKG using litep2p for networking instead of commonware-p2p.
//!
//! ## Architecture
//!
//! - **Crypto**: Uses golden-rs primitives (EVRF, Participant, BroadcastMsg)
//! - **Networking**: litep2p with QUIC transport (not commonware-p2p)
//! - **Coordination**: EpochDKGManager handles DKG lifecycle
//!
//! ## Protocol Flow
//!
//! ```text
//! Epoch N starts
//!   ↓
//! 1. Each validator generates EVRF witness
//! 2. Each validator creates BroadcastMsg
//! 3. Broadcast own message via litep2p
//! 4. Collect messages from other validators
//! 5. When threshold reached, compute group key + shares
//! 6. Start using new threshold keys for epoch N
//! ```
//!
//! ## Message Types
//!
//! - `DKGBroadcast`: Contains BroadcastMsg from a validator
//! - `DKGRequest`: Request missing broadcasts
//! - `DKGComplete`: Announce DKG completion

use anyhow::{bail, Result};
use commonware_cryptography::bls12381::primitives::group::Scalar;
use commonware_cryptography::bls12381::PublicKey;
use commonware_utils::set::Ordered;
use golden_rs::dkg::broadcast::BroadcastMsg;
use parity_scale_codec::{Encode, Decode};
use golden_rs::dkg::participant::{evrf::EVRF, registry::Registry, Participant};
use rand_core::CryptoRngCore;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use tracing::{debug, info, warn};

use crate::governance::EpochIndex;

/// DKG message types for litep2p gossip
///
/// Note: We use Vec<u8> for PublicKey/BroadcastMsg since they don't implement Serialize.
/// Use encode/decode methods from parity-scale-codec for conversion.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum DKGMessage {
    /// Broadcast message from a validator
    Broadcast {
        epoch: EpochIndex,
        /// Sender public key (encoded via parity-scale-codec)
        sender_bytes: Vec<u8>,
        /// BroadcastMsg (stored as opaque bytes - we don't have Encode impl for it yet)
        /// TODO: Add Encode derive to BroadcastMsg in golden-rs
        bmsg_bytes: Vec<u8>,
    },

    /// Request missing broadcasts (for sync/recovery)
    Request {
        epoch: EpochIndex,
        /// Missing validator public keys (encoded via parity-scale-codec)
        missing_validators_bytes: Vec<Vec<u8>>,
    },

    /// Announce DKG completion (for optimization)
    Complete {
        epoch: EpochIndex,
        /// Group public key (encoded via parity-scale-codec)
        group_pubkey_bytes: Vec<u8>,
    },
}

/// DKG state for a single epoch (litep2p version)
pub struct EpochDKGState {
    pub epoch: EpochIndex,
    pub participant: Option<Participant>,
    pub validators: Ordered<PublicKey>,
    pub our_index: Option<u32>,
    pub received_broadcasts: HashMap<PublicKey, BroadcastMsg>,
    pub completed: bool,
    pub group_pubkey: Option<PublicKey>,
    pub secret_share: Option<Scalar>,
    pub threshold: u32,
}

impl EpochDKGState {
    /// Create new DKG state for an epoch
    pub fn new(
        epoch: EpochIndex,
        validators: Ordered<PublicKey>,
        our_pubkey: &PublicKey,
        evrf: EVRF,
    ) -> Self {
        let threshold = commonware_utils::quorum(validators.len() as u32);

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
            validators,
            our_index,
            received_broadcasts: HashMap::new(),
            completed: false,
            group_pubkey: None,
            secret_share: None,
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
    pub fn on_broadcast(&mut self, sender: PublicKey, bmsg: BroadcastMsg) -> Result<()> {
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

                info!(
                    epoch = self.epoch,
                    group_key = ?self.group_pubkey,
                    "DKG completed successfully"
                );
            }
        }

        Ok(())
    }

    /// Check progress (received / total validators)
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

/// DKG Manager for litep2p
///
/// Coordinates DKG across epochs using litep2p for message transport
pub struct DKGManager {
    /// Current epoch
    current_epoch: Arc<Mutex<EpochIndex>>,

    /// DKG state per epoch
    epoch_states: Arc<Mutex<HashMap<EpochIndex, EpochDKGState>>>,

    /// Our BLS public key
    our_pubkey: PublicKey,

    /// EVRF instance (persists across epochs)
    evrf: EVRF,

    /// Beta parameter for EVRF (shared across all validators)
    beta: Scalar,
}

impl DKGManager {
    /// Create a new DKG manager
    pub fn new<R: CryptoRngCore>(
        rng: &mut R,
        our_pubkey: PublicKey,
        beta: Scalar,
    ) -> Self {
        let evrf = EVRF::random(rng, beta.clone());

        Self {
            current_epoch: Arc::new(Mutex::new(0)),
            epoch_states: Arc::new(Mutex::new(HashMap::new())),
            our_pubkey,
            evrf,
            beta,
        }
    }

    /// Start DKG for a new epoch
    pub fn start_epoch<R: CryptoRngCore>(
        &mut self,
        rng: &mut R,
        epoch: EpochIndex,
        validators: Ordered<PublicKey>,
    ) -> Result<Option<BroadcastMsg>> {
        let mut current = self.current_epoch.lock().unwrap();
        *current = epoch;

        let mut state = EpochDKGState::new(
            epoch,
            validators.clone(),
            &self.our_pubkey,
            self.evrf.clone(),
        );

        // Generate our broadcast if we're a validator
        let bmsg = state.generate_broadcast(rng)?;

        // Store the DKG state
        let mut states = self.epoch_states.lock().unwrap();
        states.insert(epoch, state);

        info!(
            epoch,
            num_validators = validators.len(),
            participating = bmsg.is_some(),
            "Started new epoch DKG"
        );

        Ok(bmsg)
    }

    /// Process incoming DKG message from litep2p
    pub fn handle_message(&self, msg: DKGMessage) -> Result<()> {
        match msg {
            DKGMessage::Broadcast { epoch, sender_bytes, bmsg_bytes } => {
                // TODO TODO TODO: Decode sender and bmsg from bytes
                // For now, just log
                debug!(epoch, "Received DKG broadcast message");
                Ok(())
            }
            DKGMessage::Request { epoch, missing_validators_bytes } => {
                debug!(epoch, num_missing = missing_validators_bytes.len(), "Received DKG request");
                // TODO: Respond with our broadcasts for missing validators
                Ok(())
            }
            DKGMessage::Complete { epoch, group_pubkey_bytes } => {
                debug!(epoch, "Received DKG complete announcement");
                Ok(())
            }
        }
    }

    /// Get group public key for an epoch (if DKG completed)
    pub fn group_pubkey(&self, epoch: EpochIndex) -> Option<PublicKey> {
        let states = self.epoch_states.lock().unwrap();
        states.get(&epoch)?.group_pubkey.clone()
    }

    /// Get our secret share for an epoch (if we're a validator and DKG completed)
    pub fn secret_share(&self, epoch: EpochIndex) -> Option<Scalar> {
        let states = self.epoch_states.lock().unwrap();
        states.get(&epoch)?.secret_share.clone()
    }

    /// Check if DKG is complete for an epoch
    pub fn is_complete(&self, epoch: EpochIndex) -> bool {
        let states = self.epoch_states.lock().unwrap();
        states.get(&epoch).map_or(false, |s| s.completed)
    }

    /// Get progress for current epoch
    pub fn current_progress(&self) -> Option<(usize, usize)> {
        let current = *self.current_epoch.lock().unwrap();
        let states = self.epoch_states.lock().unwrap();
        states.get(&current).map(|s| s.progress())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use commonware_cryptography::bls12381::primitives::group::{Element, G1};
    use rand::thread_rng;

    #[test]
    fn test_dkg_manager_creation() {
        let mut rng = thread_rng();
        let beta = Scalar::one();
        let our_key = PublicKey::from(G1::one());

        let manager = DKGManager::new(&mut rng, our_key, beta);
        assert!(manager.current_progress().is_none());
    }

    #[test]
    fn test_epoch_state_creation() {
        let mut rng = thread_rng();
        let beta = Scalar::one();
        let evrf = EVRF::random(&mut rng, beta);

        let validators = Ordered::from(vec![
            PublicKey::from(G1::one()),
        ]);
        let our_key = validators[0].clone();

        let state = EpochDKGState::new(0, validators, &our_key, evrf);
        assert_eq!(state.epoch, 0);
        assert!(state.our_index.is_some());
        assert!(state.participant.is_some());
    }
}
