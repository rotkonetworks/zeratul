//! Distributed Key Generation (DKG) for Zeratul
//!
//! Abstraction layer over DKG implementations:
//! - **MVP**: decaf377-frost (3-round interactive DKG)
//! - **Future**: golden_decaf377 (1-round non-interactive DKG)
//!
//! ## Architecture
//!
//! ```text
//! DKGCoordinator (this module)
//!     │
//!     ├─ DKGProvider trait (abstraction)
//!     │   ├─ FrostProvider (MVP - decaf377-frost)
//!     │   └─ GoldenProvider (future - golden_decaf377)
//!     │
//!     └─ Network (message broadcast)
//! ```

pub mod frost_provider;
// pub mod golden_provider;  // TODO: Implement when porting golden to decaf377

use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Epoch index for DKG rotation
pub type EpochIndex = u64;

/// Validator identifier (for now, just an index)
pub type ValidatorIndex = u32;

/// DKG provider trait - abstraction over implementations
pub trait DKGProvider: Send + Sync {
    /// DKG round message type
    /// TODO: Add Serialize + Deserialize when implementing network layer
    type Message: Clone + Send;

    /// Secret share type (kept by each validator)
    type SecretShare: Clone + Send;

    /// Group public key type (consensus key for epoch)
    type GroupPublicKey: Clone + Send;

    /// Start a new DKG ceremony for an epoch
    fn start_ceremony(
        &mut self,
        epoch: EpochIndex,
        our_index: ValidatorIndex,
        validator_count: u32,
        threshold: u32,
    ) -> Result<Self::Message>;

    /// Process a DKG message from another validator
    fn handle_message(
        &mut self,
        epoch: EpochIndex,
        from: ValidatorIndex,
        message: Self::Message,
    ) -> Result<Option<Self::Message>>; // Returns next round message if ready

    /// Check if DKG ceremony is complete
    fn is_complete(&self, epoch: EpochIndex) -> bool;

    /// Get the secret share (after completion)
    fn get_secret_share(&self, epoch: EpochIndex) -> Result<Self::SecretShare>;

    /// Get the group public key (after completion)
    fn get_group_pubkey(&self, epoch: EpochIndex) -> Result<Self::GroupPublicKey>;

    /// Sign a message using threshold signature
    fn threshold_sign(
        &self,
        epoch: EpochIndex,
        message: &[u8],
    ) -> Result<Vec<u8>>; // Returns signature share

    /// Combine signature shares into final signature
    fn combine_signatures(
        &self,
        shares: Vec<(ValidatorIndex, Vec<u8>)>,
    ) -> Result<Vec<u8>>;
}

/// DKG state for a single epoch
#[derive(Clone)]
pub struct EpochDKG<P: DKGProvider> {
    pub epoch: EpochIndex,
    pub our_index: ValidatorIndex,
    pub validator_count: u32,
    pub threshold: u32,

    /// Received messages from validators
    pub received_messages: HashMap<ValidatorIndex, P::Message>,

    /// Our secret share (after completion)
    pub secret_share: Option<P::SecretShare>,

    /// Group public key (after completion)
    pub group_pubkey: Option<P::GroupPublicKey>,

    /// Completion status
    pub completed: bool,
}

impl<P: DKGProvider> EpochDKG<P> {
    pub fn new(
        epoch: EpochIndex,
        our_index: ValidatorIndex,
        validator_count: u32,
        threshold: u32,
    ) -> Self {
        Self {
            epoch,
            our_index,
            validator_count,
            threshold,
            received_messages: HashMap::new(),
            secret_share: None,
            group_pubkey: None,
            completed: false,
        }
    }
}

/// DKG coordinator - manages ceremonies across epochs
pub struct DKGCoordinator<P: DKGProvider> {
    /// DKG provider implementation
    provider: P,

    /// Active DKG ceremonies (by epoch)
    ceremonies: HashMap<EpochIndex, EpochDKG<P>>,

    /// Current epoch
    current_epoch: EpochIndex,
}

impl<P: DKGProvider> DKGCoordinator<P> {
    pub fn new(provider: P) -> Self {
        Self {
            provider,
            ceremonies: HashMap::new(),
            current_epoch: 0,
        }
    }

    /// Start a new DKG ceremony for an epoch
    pub fn start_ceremony(
        &mut self,
        epoch: EpochIndex,
        our_index: ValidatorIndex,
        validator_count: u32,
        threshold: u32,
    ) -> Result<P::Message> {
        let message = self.provider.start_ceremony(
            epoch,
            our_index,
            validator_count,
            threshold,
        )?;

        let ceremony = EpochDKG::new(epoch, our_index, validator_count, threshold);
        self.ceremonies.insert(epoch, ceremony);
        self.current_epoch = epoch;

        Ok(message)
    }

    /// Handle a DKG message from another validator
    pub fn handle_message(
        &mut self,
        epoch: EpochIndex,
        from: ValidatorIndex,
        message: P::Message,
    ) -> Result<Option<P::Message>> {
        // Store the message
        if let Some(ceremony) = self.ceremonies.get_mut(&epoch) {
            ceremony.received_messages.insert(from, message.clone());
        }

        // Process with provider
        let next_message = self.provider.handle_message(epoch, from, message)?;

        // Check if complete
        if self.provider.is_complete(epoch) {
            if let Some(ceremony) = self.ceremonies.get_mut(&epoch) {
                ceremony.completed = true;
                ceremony.secret_share = Some(self.provider.get_secret_share(epoch)?);
                ceremony.group_pubkey = Some(self.provider.get_group_pubkey(epoch)?);
            }
        }

        Ok(next_message)
    }

    /// Check if ceremony is complete
    pub fn is_complete(&self, epoch: EpochIndex) -> bool {
        self.provider.is_complete(epoch)
    }

    /// Get the group public key for an epoch
    pub fn get_group_pubkey(&self, epoch: EpochIndex) -> Result<P::GroupPublicKey> {
        self.provider.get_group_pubkey(epoch)
    }

    /// Create a threshold signature share
    pub fn sign(&self, epoch: EpochIndex, message: &[u8]) -> Result<Vec<u8>> {
        self.provider.threshold_sign(epoch, message)
    }

    /// Combine signature shares
    pub fn combine_signatures(
        &self,
        shares: Vec<(ValidatorIndex, Vec<u8>)>,
    ) -> Result<Vec<u8>> {
        self.provider.combine_signatures(shares)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // TODO: Add DKG coordinator tests with mock provider
}
