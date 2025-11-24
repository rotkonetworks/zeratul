//! Block Production with Safrole
//!
//! Handles block authoring, checking Safrole state for authorization.

use super::safrole::SafroleState;
use super::tickets::TicketExtrinsic;
use crate::block::{Block, EpochMarker};
use anyhow::{bail, Result};
use commonware_cryptography::sha256::Digest;
use tracing::{debug, info, warn};

/// Block production configuration
#[derive(Clone, Debug)]
pub struct BlockProducerConfig {
    /// Maximum ticket extrinsics per block
    pub max_ticket_extrinsics: usize,

    /// Maximum transaction proofs per block
    pub max_transaction_proofs: usize,

    /// Block time target (milliseconds)
    pub block_time_ms: u64,
}

impl Default for BlockProducerConfig {
    fn default() -> Self {
        Self {
            max_ticket_extrinsics: 20,
            max_transaction_proofs: 1000,
            block_time_ms: 6000, // 6 seconds (JAM timeslot)
        }
    }
}

/// Block producer
pub struct BlockProducer {
    config: BlockProducerConfig,
}

impl BlockProducer {
    /// Create new block producer
    pub fn new(config: BlockProducerConfig) -> Self {
        Self { config }
    }

    /// Check if we're authorized to produce a block at this timeslot
    ///
    /// Returns Ok(seal_key) if authorized, Err otherwise
    pub fn check_authorization(
        &self,
        safrole: &SafroleState,
        timeslot: u64,
        our_bandersnatch_key: &[u8; 32],
    ) -> Result<()> {
        let slot_index = (timeslot % safrole.config.epoch_length as u64) as usize;

        // Get expected seal key for this slot
        let seal_key = safrole
            .seal_tickets
            .get_seal_for_slot(slot_index, safrole.config.epoch_length)?;

        // Check if we're authorized
        match seal_key {
            super::tickets::SealKey::Ticket(ticket) => {
                // In ticket mode, we need to check if this ticket corresponds to our key
                // TODO: This requires mapping entry_index to validator keys
                // For now, just log
                debug!(
                    timeslot,
                    ticket_id = ?ticket.id,
                    entry_index = ticket.entry_index,
                    "Slot assigned via ticket"
                );

                // Placeholder: check if we have the private key for this ticket
                // In real implementation, we'd need to track our submitted tickets
                bail!("Ticket authorization check not fully implemented");
            }
            super::tickets::SealKey::Fallback(expected_key) => {
                // In fallback mode, check if our key matches
                if expected_key == *our_bandersnatch_key {
                    debug!(timeslot, "Authorized via fallback key");
                    Ok(())
                } else {
                    bail!(
                        "Not authorized: expected key {:?}, our key {:?}",
                        expected_key,
                        our_bandersnatch_key
                    );
                }
            }
        }
    }

    /// Produce a block
    ///
    /// This is called when we're authorized for a timeslot.
    /// Creates a block with:
    /// - Ticket extrinsics (if in submission period)
    /// - Transaction proofs
    /// - Epoch markers (if epoch change)
    /// - Winners markers (if end of submission period)
    #[allow(clippy::too_many_arguments)]
    pub fn produce_block(
        &self,
        safrole: &SafroleState,
        parent: Digest,
        height: u64,
        timeslot: u64,
        timestamp: u64,
        state_root: [u8; 32],
        ticket_extrinsics: Vec<TicketExtrinsic>,
        transaction_proofs: Vec<zeratul_circuit::AccidentalComputerProof>,
        our_bandersnatch_key: [u8; 32],
        seal_signature: Vec<u8>,
        vrf_signature: Vec<u8>,
    ) -> Result<Block> {
        info!(height, timeslot, "Producing block");

        // Check authorization
        self.check_authorization(safrole, timeslot, &our_bandersnatch_key)?;

        // Limit ticket extrinsics
        let ticket_extrinsics = ticket_extrinsics
            .into_iter()
            .take(self.config.max_ticket_extrinsics)
            .collect::<Vec<_>>();

        // Limit transaction proofs
        let transaction_proofs = transaction_proofs
            .into_iter()
            .take(self.config.max_transaction_proofs)
            .collect();

        // Check if we should include epoch marker
        let epoch_marker = self.create_epoch_marker(safrole, timeslot)?;

        // Check if we should include winners marker
        let winners_marker = self.create_winners_marker(safrole, timeslot)?;

        // Determine if ticketed
        let is_ticketed = safrole.seal_tickets.is_ticketed();

        // Create block
        let block = Block::new(
            parent,
            height,
            timeslot,
            timestamp,
            state_root,
            transaction_proofs,
            our_bandersnatch_key,
            seal_signature,
            vrf_signature,
            epoch_marker,
            winners_marker,
            is_ticketed,
        );

        info!(
            height = block.height,
            timeslot = block.timeslot,
            is_ticketed = block.is_ticketed,
            "Block produced"
        );

        Ok(block)
    }

    /// Create epoch marker if this is the first block in a new epoch
    fn create_epoch_marker(
        &self,
        safrole: &SafroleState,
        timeslot: u64,
    ) -> Result<Option<EpochMarker>> {
        let epoch_index = timeslot / safrole.config.epoch_length as u64;
        let prev_epoch_index = safrole.current_slot / safrole.config.epoch_length as u64;

        if epoch_index > prev_epoch_index {
            // First block in new epoch - include epoch marker
            let validator_keys = safrole
                .pending_set
                .validators
                .iter()
                .map(|v| (v.bandersnatch_key, v.ed25519_key))
                .collect();

            Ok(Some(EpochMarker {
                current_entropy: *safrole.entropy.current_entropy(),
                previous_entropy: *safrole.entropy.epoch_entropy(1),
                validator_keys,
            }))
        } else {
            Ok(None)
        }
    }

    /// Create winners marker if submission period just ended
    fn create_winners_marker(
        &self,
        safrole: &SafroleState,
        timeslot: u64,
    ) -> Result<Option<Vec<super::tickets::SafroleTicket>>> {
        let slot_phase = (timeslot % safrole.config.epoch_length as u64) as usize;
        let prev_slot_phase = (safrole.current_slot % safrole.config.epoch_length as u64) as usize;

        // Check if we just crossed submission tail start
        if prev_slot_phase < safrole.config.submission_tail_start
            && slot_phase >= safrole.config.submission_tail_start
            && safrole.ticket_accumulator.is_saturated()
        {
            // Submission period ended - include winners
            let winners = safrole.ticket_accumulator.tickets().to_vec();

            // Apply outside-in sequencing
            let sequenced = super::tickets::outside_in_sequence(&winners);

            Ok(Some(sequenced))
        } else {
            Ok(None)
        }
    }

    /// Calculate next block timeslot
    pub fn next_timeslot(&self, current_time_ms: u64) -> u64 {
        // JAM Common Era starts at Unix timestamp 0
        // Each timeslot is 6 seconds
        current_time_ms / self.config.block_time_ms
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::consensus::safrole::{SafroleConfig, SafroleState, ValidatorInfo, ValidatorSet};
    use sp_core::bandersnatch::ring_vrf::RingContext;

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

    fn create_test_state() -> SafroleState {
        let config = SafroleConfig::default();
        let ring_ctx = RingContext::<16>::new_testing();

        let validators: Vec<_> = (0..3).map(|i| create_test_validator(i as u8)).collect();
        let validator_set = ValidatorSet::new(validators, &ring_ctx);

        SafroleState::new(config, ring_ctx, validator_set, [42u8; 32])
    }

    #[test]
    fn test_next_timeslot() {
        let producer = BlockProducer::new(BlockProducerConfig::default());

        // 6000ms = timeslot 1
        assert_eq!(producer.next_timeslot(6000), 1);

        // 12000ms = timeslot 2
        assert_eq!(producer.next_timeslot(12000), 2);

        // 5999ms = timeslot 0 (not reached timeslot 1 yet)
        assert_eq!(producer.next_timeslot(5999), 0);
    }

    #[test]
    fn test_epoch_marker_creation() {
        let mut safrole = create_test_state();
        let producer = BlockProducer::new(BlockProducerConfig::default());

        // Epoch 0, slot 0 → no marker
        let marker = producer.create_epoch_marker(&safrole, 0).unwrap();
        assert!(marker.is_none());

        // Advance to epoch 1
        safrole.current_slot = safrole.config.epoch_length as u64;
        let marker = producer
            .create_epoch_marker(&safrole, safrole.config.epoch_length as u64 + 1)
            .unwrap();
        assert!(marker.is_some());

        let marker = marker.unwrap();
        assert_eq!(marker.validator_keys.len(), 3);
    }
}
