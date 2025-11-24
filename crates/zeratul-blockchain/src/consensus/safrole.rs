//! Safrole Consensus State
//!
//! JAM-style simplified SASSAFRAS for block production.

use super::entropy::EntropyAccumulator;
use super::tickets::{
    fallback_key_sequence, outside_in_sequence, BandersnatchKey, SealTickets, SafroleTicket,
    TicketAccumulator, TicketExtrinsic,
};
use crate::governance::{AccountId, EraIndex};
use anyhow::{bail, Result};
use serde::{Deserialize, Serialize};
use sp_core::bandersnatch::{
    ring_vrf::{RingContext, RingVerifierKey},
    Public as BandersnatchPublic,
};

/// Ring context size (must match validator set size, using 16 for now)
pub const RING_SIZE: usize = 16;

/// Ring verifier key (Pedersen commitment to validator set)
/// This is what validators submit tickets against
pub type RingRoot = RingVerifierKey;

/// Safrole configuration
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct SafroleConfig {
    /// Epoch length in slots (e.g., 600 slots = 1 hour at 6-second slots)
    pub epoch_length: usize,

    /// Submission period end (2/3 of epoch)
    /// After this point, no more tickets accepted
    pub submission_tail_start: usize,

    /// Maximum tickets per block
    pub max_block_tickets: usize,

    /// Maximum ticket entries per validator
    pub max_ticket_entries: u32,
}

impl Default for SafroleConfig {
    fn default() -> Self {
        const EPOCH_LENGTH: usize = 600; // 1 hour at 6-second slots

        Self {
            epoch_length: EPOCH_LENGTH,
            submission_tail_start: (EPOCH_LENGTH * 2) / 3, // 400 slots
            max_block_tickets: 20,
            max_ticket_entries: 3,
        }
    }
}

// Use serde_big_array for large arrays
use serde_big_array::BigArray;

/// Validator info (minimal for Safrole)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ValidatorInfo {
    /// Account ID
    pub account: AccountId,

    /// Bandersnatch key (for block sealing)
    pub bandersnatch_key: BandersnatchKey,

    /// Ed25519 key (for identification)
    pub ed25519_key: [u8; 32],

    /// BLS key (for BEEFY finality)
    #[serde(with = "BigArray")]
    pub bls_key: [u8; 144],

    /// Metadata (network address, etc.)
    #[serde(with = "BigArray")]
    pub metadata: [u8; 128],
}

/// Validator set
pub struct ValidatorSet {
    /// Validators
    pub validators: Vec<ValidatorInfo>,

    /// Ring root (Bandersnatch commitment)
    pub ring_root: RingRoot,
}

impl Clone for ValidatorSet {
    fn clone(&self) -> Self {
        use parity_scale_codec::{Encode, Decode};
        // RingVerifierKey doesn't implement Clone, so encode/decode it
        let encoded = self.ring_root.encode();
        let ring_root = RingVerifierKey::decode(&mut &encoded[..])
            .expect("Failed to decode ring verifier key");
        Self {
            validators: self.validators.clone(),
            ring_root,
        }
    }
}

// Manual Serialize/Deserialize for ValidatorSet (RingRoot doesn't support serde)
impl Serialize for ValidatorSet {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        use parity_scale_codec::Encode;
        use serde::ser::SerializeStruct;

        let mut state = serializer.serialize_struct("ValidatorSet", 2)?;
        state.serialize_field("validators", &self.validators)?;
        state.serialize_field("ring_root", &self.ring_root.encode())?;
        state.end()
    }
}

impl<'de> Deserialize<'de> for ValidatorSet {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        use parity_scale_codec::Decode;
        use serde::de::{self, MapAccess, Visitor};

        #[derive(Deserialize)]
        #[serde(field_identifier, rename_all = "snake_case")]
        enum Field {
            Validators,
            RingRoot,
        }

        struct ValidatorSetVisitor;

        impl<'de> Visitor<'de> for ValidatorSetVisitor {
            type Value = ValidatorSet;

            fn expecting(&self, formatter: &mut std::fmt::Formatter) -> std::fmt::Result {
                formatter.write_str("struct ValidatorSet")
            }

            fn visit_map<V>(self, mut map: V) -> Result<ValidatorSet, V::Error>
            where
                V: MapAccess<'de>,
            {
                let mut validators = None;
                let mut ring_root = None;

                while let Some(key) = map.next_key()? {
                    match key {
                        Field::Validators => validators = Some(map.next_value()?),
                        Field::RingRoot => {
                            let bytes: Vec<u8> = map.next_value()?;
                            ring_root = Some(
                                RingVerifierKey::decode(&mut &bytes[..])
                                    .map_err(de::Error::custom)?,
                            );
                        }
                    }
                }

                Ok(ValidatorSet {
                    validators: validators.ok_or_else(|| de::Error::missing_field("validators"))?,
                    ring_root: ring_root.ok_or_else(|| de::Error::missing_field("ring_root"))?,
                })
            }
        }

        const FIELDS: &[&str] = &["validators", "ring_root"];
        deserializer.deserialize_struct("ValidatorSet", FIELDS, ValidatorSetVisitor)
    }
}

impl ValidatorSet {
    /// Create new validator set with ring context
    pub fn new(validators: Vec<ValidatorInfo>, ring_ctx: &RingContext<RING_SIZE>) -> Self {
        let ring_root = Self::compute_ring_root(&validators, ring_ctx);

        Self {
            validators,
            ring_root,
        }
    }

    /// Compute ring verifier key (Pedersen commitment to validator set)
    ///
    /// This creates a cryptographic commitment to the validator set that allows
    /// validators to prove membership without revealing their identity (Ring VRF).
    fn compute_ring_root(
        validators: &[ValidatorInfo],
        ring_ctx: &RingContext<RING_SIZE>,
    ) -> RingRoot {
        use parity_scale_codec::Decode;

        // Convert validator bandersnatch keys to Public type
        let public_keys: Vec<BandersnatchPublic> = validators
            .iter()
            .map(|v| {
                BandersnatchPublic::decode(&mut &v.bandersnatch_key[..])
                    .expect("Valid 32-byte compressed Bandersnatch public key")
            })
            .collect();

        // Compute ring verifier key (Pedersen commitment)
        ring_ctx.verifier_key(&public_keys)
    }

    /// Get Bandersnatch keys
    pub fn bandersnatch_keys(&self) -> Vec<BandersnatchKey> {
        self.validators
            .iter()
            .map(|v| v.bandersnatch_key)
            .collect()
    }
}

/// Safrole consensus state
pub struct SafroleState {
    /// Configuration
    pub config: SafroleConfig,

    /// Ring context for computing verifier keys
    /// Note: This is large (~1KB) but necessary for computing ring roots
    pub ring_context: RingContext<RING_SIZE>,

    /// Pending validator set (for next epoch)
    pub pending_set: ValidatorSet,

    /// Active validator set (current epoch)
    pub active_set: ValidatorSet,

    /// Previous validator set (last epoch)
    pub previous_set: ValidatorSet,

    /// Epoch's ring root (for ticket verification)
    pub epoch_root: RingRoot,

    /// Seal tickets for current epoch
    pub seal_tickets: SealTickets,

    /// Ticket accumulator (for next epoch)
    pub ticket_accumulator: TicketAccumulator,

    /// Entropy accumulator
    pub entropy: EntropyAccumulator,

    /// Current slot index
    pub current_slot: u64,

    /// Current epoch index
    pub current_epoch: u64,
}

impl Clone for SafroleState {
    fn clone(&self) -> Self {
        use parity_scale_codec::{Encode, Decode};
        // RingVerifierKey doesn't implement Clone, so encode/decode epoch_root
        let encoded_root = self.epoch_root.encode();
        let epoch_root = RingVerifierKey::decode(&mut &encoded_root[..])
            .expect("Failed to decode ring verifier key");

        Self {
            config: self.config.clone(),
            ring_context: self.ring_context.clone(),
            pending_set: self.pending_set.clone(),
            active_set: self.active_set.clone(),
            previous_set: self.previous_set.clone(),
            epoch_root,
            seal_tickets: self.seal_tickets.clone(),
            ticket_accumulator: self.ticket_accumulator.clone(),
            entropy: self.entropy.clone(),
            current_slot: self.current_slot,
            current_epoch: self.current_epoch,
        }
    }
}

// Manual Serialize/Deserialize implementation
// (RingContext implements these via parity_scale_codec)
impl Serialize for SafroleState {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        use parity_scale_codec::Encode;
        use serde::ser::SerializeStruct;

        let mut state = serializer.serialize_struct("SafroleState", 11)?;
        state.serialize_field("config", &self.config)?;
        state.serialize_field("ring_context", &self.ring_context.encode())?;
        state.serialize_field("pending_set", &self.pending_set)?;
        state.serialize_field("active_set", &self.active_set)?;
        state.serialize_field("previous_set", &self.previous_set)?;
        state.serialize_field("epoch_root", &self.epoch_root.encode())?;
        state.serialize_field("seal_tickets", &self.seal_tickets)?;
        state.serialize_field("ticket_accumulator", &self.ticket_accumulator)?;
        state.serialize_field("entropy", &self.entropy)?;
        state.serialize_field("current_slot", &self.current_slot)?;
        state.serialize_field("current_epoch", &self.current_epoch)?;
        state.end()
    }
}

impl<'de> Deserialize<'de> for SafroleState {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        use parity_scale_codec::Decode;
        use serde::de::{self, MapAccess, Visitor};

        #[derive(Deserialize)]
        #[serde(field_identifier, rename_all = "snake_case")]
        enum Field {
            Config,
            RingContext,
            PendingSet,
            ActiveSet,
            PreviousSet,
            EpochRoot,
            SealTickets,
            TicketAccumulator,
            Entropy,
            CurrentSlot,
            CurrentEpoch,
        }

        struct SafroleStateVisitor;

        impl<'de> Visitor<'de> for SafroleStateVisitor {
            type Value = SafroleState;

            fn expecting(&self, formatter: &mut std::fmt::Formatter) -> std::fmt::Result {
                formatter.write_str("struct SafroleState")
            }

            fn visit_map<V>(self, mut map: V) -> Result<SafroleState, V::Error>
            where
                V: MapAccess<'de>,
            {
                let mut config = None;
                let mut ring_context = None;
                let mut pending_set = None;
                let mut active_set = None;
                let mut previous_set = None;
                let mut epoch_root = None;
                let mut seal_tickets = None;
                let mut ticket_accumulator = None;
                let mut entropy = None;
                let mut current_slot = None;
                let mut current_epoch = None;

                while let Some(key) = map.next_key()? {
                    match key {
                        Field::Config => config = Some(map.next_value()?),
                        Field::RingContext => {
                            let bytes: Vec<u8> = map.next_value()?;
                            ring_context = Some(
                                RingContext::decode(&mut &bytes[..])
                                    .map_err(de::Error::custom)?,
                            );
                        }
                        Field::PendingSet => pending_set = Some(map.next_value()?),
                        Field::ActiveSet => active_set = Some(map.next_value()?),
                        Field::PreviousSet => previous_set = Some(map.next_value()?),
                        Field::EpochRoot => {
                            let bytes: Vec<u8> = map.next_value()?;
                            epoch_root = Some(
                                RingVerifierKey::decode(&mut &bytes[..])
                                    .map_err(de::Error::custom)?,
                            );
                        }
                        Field::SealTickets => seal_tickets = Some(map.next_value()?),
                        Field::TicketAccumulator => ticket_accumulator = Some(map.next_value()?),
                        Field::Entropy => entropy = Some(map.next_value()?),
                        Field::CurrentSlot => current_slot = Some(map.next_value()?),
                        Field::CurrentEpoch => current_epoch = Some(map.next_value()?),
                    }
                }

                Ok(SafroleState {
                    config: config.ok_or_else(|| de::Error::missing_field("config"))?,
                    ring_context: ring_context
                        .ok_or_else(|| de::Error::missing_field("ring_context"))?,
                    pending_set: pending_set
                        .ok_or_else(|| de::Error::missing_field("pending_set"))?,
                    active_set: active_set
                        .ok_or_else(|| de::Error::missing_field("active_set"))?,
                    previous_set: previous_set
                        .ok_or_else(|| de::Error::missing_field("previous_set"))?,
                    epoch_root: epoch_root
                        .ok_or_else(|| de::Error::missing_field("epoch_root"))?,
                    seal_tickets: seal_tickets
                        .ok_or_else(|| de::Error::missing_field("seal_tickets"))?,
                    ticket_accumulator: ticket_accumulator
                        .ok_or_else(|| de::Error::missing_field("ticket_accumulator"))?,
                    entropy: entropy.ok_or_else(|| de::Error::missing_field("entropy"))?,
                    current_slot: current_slot
                        .ok_or_else(|| de::Error::missing_field("current_slot"))?,
                    current_epoch: current_epoch
                        .ok_or_else(|| de::Error::missing_field("current_epoch"))?,
                })
            }
        }

        const FIELDS: &[&str] = &[
            "config",
            "ring_context",
            "pending_set",
            "active_set",
            "previous_set",
            "epoch_root",
            "seal_tickets",
            "ticket_accumulator",
            "entropy",
            "current_slot",
            "current_epoch",
        ];
        deserializer.deserialize_struct("SafroleState", FIELDS, SafroleStateVisitor)
    }
}

impl SafroleState {
    /// Create new Safrole state
    pub fn new(
        config: SafroleConfig,
        ring_context: RingContext<RING_SIZE>,
        genesis_validators: ValidatorSet,
        genesis_entropy: [u8; 32],
    ) -> Self {
        // Initial fallback keys (no tickets yet)
        let fallback_keys = fallback_key_sequence(
            &genesis_entropy,
            &genesis_validators.bandersnatch_keys(),
            config.epoch_length,
        );

        Self {
            config: config.clone(),
            ring_context,
            pending_set: genesis_validators.clone(),
            active_set: genesis_validators.clone(),
            previous_set: genesis_validators.clone(),
            epoch_root: genesis_validators.ring_root,
            seal_tickets: SealTickets::Fallback(fallback_keys),
            ticket_accumulator: TicketAccumulator::new(config.epoch_length),
            entropy: EntropyAccumulator::new(genesis_entropy),
            current_slot: 0,
            current_epoch: 0,
        }
    }

    /// Get current epoch index
    pub fn epoch_index(&self) -> u64 {
        self.current_epoch
    }

    /// Get slot phase within epoch (0..epoch_length)
    pub fn slot_phase(&self) -> usize {
        (self.current_slot % self.config.epoch_length as u64) as usize
    }

    /// Check if we're in ticket submission period
    pub fn is_submission_period(&self) -> bool {
        self.slot_phase() < self.config.submission_tail_start
    }

    /// Check if epoch is changing
    fn is_epoch_change(&self, new_slot: u64) -> bool {
        let old_epoch = self.current_slot / self.config.epoch_length as u64;
        let new_epoch = new_slot / self.config.epoch_length as u64;

        new_epoch > old_epoch
    }

    /// Process ticket extrinsics
    pub fn process_tickets(&mut self, extrinsics: Vec<TicketExtrinsic>) -> Result<()> {
        // Check submission period
        if !self.is_submission_period() {
            bail!(
                "Ticket submission closed (slot phase: {}/{})",
                self.slot_phase(),
                self.config.submission_tail_start
            );
        }

        // Check ticket limit
        if extrinsics.len() > self.config.max_block_tickets {
            bail!(
                "Too many tickets: {} > {}",
                extrinsics.len(),
                self.config.max_block_tickets
            );
        }

        // Verify each ticket's Ring VRF proof
        for ext in &extrinsics {
            // Verify entry index
            if ext.entry_index >= self.config.max_ticket_entries {
                bail!("Entry index {} exceeds max {}", ext.entry_index, self.config.max_ticket_entries);
            }

            // Verify Ring VRF proof
            let context = self.ticket_verification_context();
            if !ext.verify(&self.epoch_root, &context)? {
                bail!("Invalid Ring VRF proof");
            }
        }

        // Add to accumulator
        self.ticket_accumulator.add_tickets(extrinsics)?;

        tracing::debug!(
            "Processed tickets, accumulator: {}/{} (saturated: {})",
            self.ticket_accumulator.tickets().len(),
            self.config.epoch_length,
            self.ticket_accumulator.is_saturated()
        );

        Ok(())
    }

    /// Ticket verification context (includes entropy)
    fn ticket_verification_context(&self) -> Vec<u8> {
        // Context: "$jam_ticket" || entropy_2 || entry_index
        // (entry_index added per-ticket in verification)
        let mut context = Vec::new();
        context.extend_from_slice(b"$jam_ticket");
        context.extend_from_slice(self.entropy.ticket_entropy());
        context
    }

    /// Transition to new slot
    pub fn transition_slot(
        &mut self,
        new_slot: u64,
        vrf_output: [u8; 32],
        staging_set: Option<ValidatorSet>,
    ) -> Result<()> {
        if new_slot <= self.current_slot {
            bail!("Slot must advance: {} -> {}", self.current_slot, new_slot);
        }

        // Accumulate entropy from VRF
        self.entropy.accumulate(&vrf_output);

        // Check for epoch change
        if self.is_epoch_change(new_slot) {
            self.transition_epoch(staging_set)?;
        }

        self.current_slot = new_slot;
        self.current_epoch = new_slot / self.config.epoch_length as u64;

        Ok(())
    }

    /// Transition to new epoch
    fn transition_epoch(&mut self, staging_set: Option<ValidatorSet>) -> Result<()> {
        tracing::info!(
            "Epoch transition: {} -> {}",
            self.current_epoch,
            self.current_epoch + 1
        );

        // Rotate entropy
        self.entropy.rotate_epoch();

        // Rotate validator sets
        self.previous_set = self.active_set.clone();
        self.active_set = self.pending_set.clone();

        // Update pending set from staging (if provided)
        if let Some(staging) = staging_set {
            self.pending_set = staging;
        }

        // Update epoch root for next epoch
        // Note: RingVerifierKey doesn't implement Clone or Default, so we use encode/decode
        // pending_set will be replaced in the next epoch transition anyway
        use parity_scale_codec::{Encode, Decode};
        let encoded = self.pending_set.ring_root.encode();
        self.epoch_root = RingVerifierKey::decode(&mut &encoded[..])
            .expect("Failed to decode ring verifier key");

        // Generate seal tickets for new epoch
        self.generate_seal_tickets()?;

        tracing::info!(
            "Epoch transition complete. Seal tickets: {} (mode: {})",
            self.seal_tickets.len(),
            if self.seal_tickets.is_ticketed() { "ticketed" } else { "fallback" }
        );

        Ok(())
    }

    /// Generate seal tickets for new epoch
    fn generate_seal_tickets(&mut self) -> Result<()> {
        let old_slot_phase = self.slot_phase();

        // Check if we have full ticket accumulator
        if old_slot_phase >= self.config.submission_tail_start
            && self.ticket_accumulator.is_saturated()
        {
            // Normal mode: use tickets with outside-in sequencer
            let tickets = self.ticket_accumulator.take();
            let sequenced = outside_in_sequence(&tickets);

            self.seal_tickets = SealTickets::Tickets(sequenced);

            tracing::info!("Generated seal tickets from accumulator (normal mode)");
        } else {
            // Fallback mode: deterministic key selection
            let fallback_keys = fallback_key_sequence(
                self.entropy.fallback_entropy(),
                &self.active_set.bandersnatch_keys(),
                self.config.epoch_length,
            );

            self.seal_tickets = SealTickets::Fallback(fallback_keys);

            tracing::warn!(
                "Using fallback seal keys (tickets: {}/{}, phase: {}/{})",
                self.ticket_accumulator.tickets().len(),
                self.config.epoch_length,
                old_slot_phase,
                self.config.submission_tail_start
            );

            // Clear accumulator
            self.ticket_accumulator.clear();
        }

        Ok(())
    }

    /// Get seal key for slot
    pub fn get_seal_key(&self, slot: u64) -> Result<Vec<u8>> {
        let slot_index = (slot % self.config.epoch_length as u64) as usize;

        match self.seal_tickets.get_seal_for_slot(slot_index, self.config.epoch_length)? {
            super::tickets::SealKey::Ticket(ticket) => {
                // For tickets, we need to resolve which validator
                // TODO: Map entry_index to actual validator key
                // For now, return ticket ID as placeholder
                Ok(ticket.id.to_vec())
            }
            super::tickets::SealKey::Fallback(key) => Ok(key.to_vec()),
        }
    }

    /// Check if current mode is ticketed
    pub fn is_ticketed(&self) -> bool {
        self.seal_tickets.is_ticketed()
    }

    /// Get statistics
    pub fn stats(&self) -> SafroleStats {
        SafroleStats {
            epoch: self.current_epoch,
            slot: self.current_slot,
            slot_phase: self.slot_phase(),
            submission_period: self.is_submission_period(),
            tickets_accumulated: self.ticket_accumulator.tickets().len(),
            tickets_required: self.config.epoch_length,
            is_ticketed: self.is_ticketed(),
            active_validators: self.active_set.validators.len(),
        }
    }
}

/// Safrole statistics
#[derive(Debug, Clone)]
pub struct SafroleStats {
    pub epoch: u64,
    pub slot: u64,
    pub slot_phase: usize,
    pub submission_period: bool,
    pub tickets_accumulated: usize,
    pub tickets_required: usize,
    pub is_ticketed: bool,
    pub active_validators: usize,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn create_test_validator(id: u8) -> ValidatorInfo {
        ValidatorInfo {
            account: {
                let mut acc = [0u8; 32];
                acc[0] = id;
                acc
            },
            bandersnatch_key: [id; 32],
            ed25519_key: [id; 32],
            bls_key: [id; 144],
            metadata: [id; 128],
        }
    }

    fn create_test_ring_context() -> RingContext<RING_SIZE> {
        RingContext::<RING_SIZE>::new_testing()
    }

    fn create_test_validator_set(count: usize) -> ValidatorSet {
        let validators: Vec<_> = (0..count)
            .map(|i| create_test_validator(i as u8))
            .collect();

        let ring_ctx = create_test_ring_context();
        ValidatorSet::new(validators, &ring_ctx)
    }

    #[test]
    fn test_safrole_init() {
        let config = SafroleConfig::default();
        let ring_ctx = create_test_ring_context();
        let validators = create_test_validator_set(15);
        let genesis_entropy = [42u8; 32];

        let state = SafroleState::new(config, ring_ctx, validators, genesis_entropy);

        assert_eq!(state.current_epoch, 0);
        assert_eq!(state.current_slot, 0);
        assert_eq!(state.active_set.validators.len(), 15);
        assert!(!state.is_ticketed()); // Starts in fallback mode
    }

    #[test]
    fn test_slot_transition() {
        let config = SafroleConfig::default();
        let ring_ctx = create_test_ring_context();
        let validators = create_test_validator_set(15);
        let mut state = SafroleState::new(config, ring_ctx, validators, [0u8; 32]);

        // Advance slot
        let vrf_output = [1u8; 32];
        state.transition_slot(1, vrf_output, None).unwrap();

        assert_eq!(state.current_slot, 1);
        assert_eq!(state.current_epoch, 0);
    }

    #[test]
    fn test_epoch_transition() {
        let mut config = SafroleConfig::default();
        config.epoch_length = 10; // Small epoch for testing

        let ring_ctx = create_test_ring_context();
        let validators = create_test_validator_set(3);
        let mut state = SafroleState::new(config, ring_ctx, validators, [0u8; 32]);

        // Advance to epoch boundary
        for i in 1..=10 {
            state.transition_slot(i, [i as u8; 32], None).unwrap();
        }

        // Should be in epoch 1 now
        assert_eq!(state.current_epoch, 1);

        // Check validator set rotation
        assert_eq!(state.active_set.validators.len(), 3);
    }

    #[test]
    fn test_submission_period() {
        let mut config = SafroleConfig::default();
        config.epoch_length = 300;
        config.submission_tail_start = 200;

        let ring_ctx = create_test_ring_context();
        let validators = create_test_validator_set(3);
        let mut state = SafroleState::new(config, ring_ctx, validators, [0u8; 32]);

        // First 200 slots: submission open
        for i in 0..200 {
            state.current_slot = i;
            assert!(state.is_submission_period());
        }

        // After 200 slots: submission closed
        for i in 200..300 {
            state.current_slot = i;
            assert!(!state.is_submission_period());
        }
    }

    #[test]
    fn test_stats() {
        let config = SafroleConfig::default();
        let ring_ctx = create_test_ring_context();
        let validators = create_test_validator_set(15);
        let mut state = SafroleState::new(config, ring_ctx, validators, [0u8; 32]);

        state.current_slot = 100;
        state.current_epoch = 0;

        let stats = state.stats();

        assert_eq!(stats.epoch, 0);
        assert_eq!(stats.slot, 100);
        assert_eq!(stats.slot_phase, 100);
        assert!(stats.submission_period);
        assert_eq!(stats.active_validators, 15);
    }
}
