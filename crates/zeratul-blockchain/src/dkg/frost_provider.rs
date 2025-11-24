//! FROST DKG Provider (decaf377-frost)
//!
//! MVP implementation using Penumbra's decaf377-frost library.
//! This is a 3-round interactive DKG protocol.
//!
//! ## Protocol Flow
//!
//! ```text
//! Round 1: Commitment phase
//!   - Each validator generates commitments
//!   - Broadcast commitments to all
//!
//! Round 2: Shares phase
//!   - Each validator generates secret shares
//!   - Send shares to each validator (point-to-point)
//!
//! Round 3: Verification phase
//!   - Verify received shares
//!   - Compute group public key
//!   - DKG complete!
//! ```

use anyhow::{Result, bail};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use rand_core::OsRng;

use decaf377_frost as frost;
use frost::Identifier;

use super::{DKGProvider, EpochIndex, ValidatorIndex};

/// FROST DKG message (combines all rounds)
/// Note: For MVP, these are kept as FROST types (not serialized)
/// TODO: Add proper serialization when wiring up network layer
#[derive(Clone)]
pub enum FrostMessage {
    /// Round 1: Commitment
    Round1 {
        /// VSS commitments package
        package: frost::keys::dkg::round1::Package,
    },

    /// Round 2: Secret shares
    Round2 {
        /// Secret share packages for each participant
        packages: HashMap<ValidatorIndex, frost::keys::dkg::round2::Package>,
    },

    /// Round 3: Verification complete
    Round3 {
        /// Group public key package
        public_key_package: frost::keys::PublicKeyPackage,
    },
}

/// FROST DKG state for one epoch
struct FrostEpochState {
    epoch: EpochIndex,
    our_index: ValidatorIndex,
    our_frost_id: Identifier,
    validator_count: u32,
    threshold: u32,

    /// Current round (1, 2, or 3)
    round: u8,

    /// Round 1 secret (internal state for round 2)
    round1_secret: Option<frost::keys::dkg::round1::SecretPackage>,

    /// Round 2 secret (internal state for round 3)
    round2_secret: Option<frost::keys::dkg::round2::SecretPackage>,

    /// Round 1: Received packages from others
    round1_packages: HashMap<Identifier, frost::keys::dkg::round1::Package>,

    /// Round 2: Received packages for us from others
    round2_packages: HashMap<Identifier, frost::keys::dkg::round2::Package>,

    /// Final key package (our secret share)
    key_package: Option<frost::keys::KeyPackage>,

    /// Final group public key package
    public_key_package: Option<frost::keys::PublicKeyPackage>,

    /// Completion flag
    completed: bool,
}

/// FROST DKG provider
pub struct FrostProvider {
    /// Active ceremonies by epoch
    ceremonies: HashMap<EpochIndex, FrostEpochState>,
}

impl FrostProvider {
    pub fn new() -> Self {
        Self {
            ceremonies: HashMap::new(),
        }
    }
}

impl Default for FrostProvider {
    fn default() -> Self {
        Self::new()
    }
}

impl DKGProvider for FrostProvider {
    type Message = FrostMessage;
    type SecretShare = Vec<u8>; // Serialized for now
    type GroupPublicKey = Vec<u8>; // Serialized for now

    fn start_ceremony(
        &mut self,
        epoch: EpochIndex,
        our_index: ValidatorIndex,
        validator_count: u32,
        threshold: u32,
    ) -> Result<Self::Message> {
        // Generate FROST identifier from our validator index
        let our_frost_id = Identifier::derive(&our_index.to_le_bytes())
            .map_err(|e| anyhow::anyhow!("Failed to derive FROST identifier: {:?}", e))?;

        // FROST Round 1: Generate commitments
        let (round1_secret, round1_package) = frost::keys::dkg::part1(
            our_frost_id,
            validator_count as u16,
            threshold as u16,
            &mut OsRng,
        )?;

        let state = FrostEpochState {
            epoch,
            our_index,
            our_frost_id,
            validator_count,
            threshold,
            round: 1,
            round1_secret: Some(round1_secret),
            round2_secret: None,
            round1_packages: HashMap::new(),
            round2_packages: HashMap::new(),
            key_package: None,
            public_key_package: None,
            completed: false,
        };

        self.ceremonies.insert(epoch, state);

        Ok(FrostMessage::Round1 {
            package: round1_package,
        })
    }

    fn handle_message(
        &mut self,
        epoch: EpochIndex,
        from: ValidatorIndex,
        message: Self::Message,
    ) -> Result<Option<Self::Message>> {
        let state = self.ceremonies.get_mut(&epoch)
            .ok_or_else(|| anyhow::anyhow!("No ceremony for epoch {}", epoch))?;

        match message {
            FrostMessage::Round1 { package } => {
                // Derive FROST identifier from validator index
                let from_frost_id = Identifier::derive(&from.to_le_bytes())
                    .map_err(|e| anyhow::anyhow!("Failed to derive FROST identifier: {:?}", e))?;

                // Store package from this validator
                state.round1_packages.insert(from_frost_id, package);

                // Check if we have all commitments (N-1, since we don't send to ourselves)
                if state.round1_packages.len() == (state.validator_count - 1) as usize {
                    // Move to round 2
                    state.round = 2;

                    // FROST Round 2: Generate secret shares
                    let round1_secret = state.round1_secret.take()
                        .ok_or_else(|| anyhow::anyhow!("Missing round1 secret"))?;

                    let (round2_secret, round2_packages) = frost::keys::dkg::part2(
                        round1_secret,
                        &state.round1_packages,
                    )?;

                    state.round2_secret = Some(round2_secret);

                    // Convert FROST identifier map to ValidatorIndex map
                    let mut packages = HashMap::new();
                    for (frost_id, package) in round2_packages {
                        // Convert Identifier back to ValidatorIndex
                        // This assumes we used to_le_bytes() to create the Identifier
                        let validator_idx = u32::from_le_bytes(
                            frost_id.serialize()[..4].try_into()
                                .map_err(|_| anyhow::anyhow!("Invalid identifier"))?
                        );
                        packages.insert(validator_idx, package);
                    }

                    return Ok(Some(FrostMessage::Round2 { packages }));
                }

                Ok(None) // Waiting for more commitments
            }

            FrostMessage::Round2 { packages } => {
                // Extract our share from this validator
                if let Some(our_package) = packages.get(&state.our_index) {
                    // Derive FROST identifier from sender's validator index
                    let from_frost_id = Identifier::derive(&from.to_le_bytes())
                        .map_err(|e| anyhow::anyhow!("Failed to derive FROST identifier: {:?}", e))?;

                    // Store package from this validator
                    state.round2_packages.insert(from_frost_id, our_package.clone());
                }

                // Check if we have all shares (N-1, since we don't send to ourselves)
                if state.round2_packages.len() == (state.validator_count - 1) as usize {
                    // Move to round 3
                    state.round = 3;

                    // FROST Round 3: Verify and finalize
                    let round2_secret = state.round2_secret.take()
                        .ok_or_else(|| anyhow::anyhow!("Missing round2 secret"))?;

                    let (key_package, public_key_package) = frost::keys::dkg::part3(
                        &round2_secret,
                        &state.round1_packages,
                        &state.round2_packages,
                    )?;

                    state.key_package = Some(key_package);
                    let pubkey_clone = public_key_package.clone();
                    state.public_key_package = Some(public_key_package);
                    state.completed = true;

                    return Ok(Some(FrostMessage::Round3 {
                        public_key_package: pubkey_clone,
                    }));
                }

                Ok(None) // Waiting for more shares
            }

            FrostMessage::Round3 { public_key_package } => {
                // Verify it matches ours
                if let Some(our_pubkey) = &state.public_key_package {
                    if our_pubkey.group_public().serialize() != public_key_package.group_public().serialize() {
                        bail!("Group public key mismatch! Possible Byzantine behavior.");
                    }
                }

                // Mark as completed
                state.completed = true;
                Ok(None)
            }
        }
    }

    fn is_complete(&self, epoch: EpochIndex) -> bool {
        self.ceremonies.get(&epoch)
            .map(|s| s.completed)
            .unwrap_or(false)
    }

    fn get_secret_share(&self, epoch: EpochIndex) -> Result<Self::SecretShare> {
        let state = self.ceremonies.get(&epoch)
            .ok_or_else(|| anyhow::anyhow!("No ceremony for epoch {}", epoch))?;

        let _key_package = state.key_package.as_ref()
            .ok_or_else(|| anyhow::anyhow!("DKG not complete for epoch {}", epoch))?;

        // TODO: Implement proper serialization when needed
        // For MVP, return placeholder since FROST types don't implement Serialize
        Ok(Vec::new())
    }

    fn get_group_pubkey(&self, epoch: EpochIndex) -> Result<Self::GroupPublicKey> {
        let state = self.ceremonies.get(&epoch)
            .ok_or_else(|| anyhow::anyhow!("No ceremony for epoch {}", epoch))?;

        let pubkey = state.public_key_package.as_ref()
            .ok_or_else(|| anyhow::anyhow!("DKG not complete for epoch {}", epoch))?;

        // Return serialized group public key (32 bytes)
        Ok(pubkey.group_public().serialize().to_vec())
    }

    fn threshold_sign(&self, epoch: EpochIndex, message: &[u8]) -> Result<Vec<u8>> {
        // TODO: Full FROST threshold signing
        //
        // FROST signing is interactive (2 rounds):
        //   Round 1: Each signer generates nonce + commitment
        //   Round 2: Each signer creates signature share
        //   Coordinator aggregates shares into final signature
        //
        // This would require extending FrostMessage with:
        //   - SignRound1 { commitments }
        //   - SignRound2 { signature_share }
        //
        // For now, return error indicating signing needs full protocol

        bail!("FROST threshold signing not yet implemented")
    }

    fn combine_signatures(&self, shares: Vec<(ValidatorIndex, Vec<u8>)>) -> Result<Vec<u8>> {
        // TODO TODO TODO: Implement FROST signature aggregation
        //
        // 1. Verify we have threshold number of shares
        // 2. Aggregate shares using Lagrange interpolation
        // 3. Return final signature

        bail!("FROST signature combination not yet implemented")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_frost_provider_creation() {
        let provider = FrostProvider::new();
        assert_eq!(provider.ceremonies.len(), 0);
    }

    #[test]
    fn test_start_ceremony() {
        let mut provider = FrostProvider::new();
        let result = provider.start_ceremony(0, 0, 4, 3);
        assert!(result.is_ok());
    }

    // TODO: Add full DKG ceremony tests
    // - Test 4-validator ceremony
    // - Test threshold signing
    // - Test signature combination
}
