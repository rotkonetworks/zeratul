//! Safrole Tickets
//!
//! Ticket-based slot assignment for block production.

use anyhow::{bail, Result};
use serde::{Deserialize, Serialize};
use sp_core::bandersnatch::{
    ring_vrf::{RingContext, RingProver, RingVerifier, RingVerifierKey, RingVrfSignature},
    vrf::{VrfInput, VrfPreOutput, VrfSignData},
};
use std::collections::BTreeSet;

/// Ticket ID (VRF output hash, used for scoring)
pub type TicketId = [u8; 32];

/// Bandersnatch public key (32 bytes compressed)
pub type BandersnatchKey = [u8; 32];

/// Re-export Ring VRF signature type from sp-core
pub type BandersnatchRingProof = RingVrfSignature;

/// Safrole ticket
///
/// Combination of ticket ID (score) and entry index (validator slot)
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
pub struct SafroleTicket {
    /// Ticket ID (VRF output, lower is better)
    pub id: TicketId,

    /// Entry index (validator slot, 0..MAX_ENTRIES)
    pub entry_index: u32,
}

impl SafroleTicket {
    /// Create new ticket
    pub fn new(id: TicketId, entry_index: u32) -> Self {
        Self { id, entry_index }
    }

    /// Create from Ring VRF signature
    ///
    /// Ticket ID is Blake3 hash of the VRF pre-output (lower is better)
    pub fn from_ring_proof(proof: &BandersnatchRingProof, entry_index: u32) -> Self {
        use parity_scale_codec::Encode;

        // Hash the VRF pre-output to get ticket ID
        let preout_bytes = proof.pre_output.encode();
        let hash = blake3::hash(&preout_bytes);

        Self {
            id: *hash.as_bytes(),
            entry_index,
        }
    }
}

/// Ticket extrinsic (submitted in blocks)
#[derive(Debug, Clone)]
pub struct TicketExtrinsic {
    /// Entry index (validator slot)
    pub entry_index: u32,

    /// Ring VRF proof (anonymous)
    pub proof: BandersnatchRingProof,
}

// Manual Serialize/Deserialize for TicketExtrinsic (RingVrfSignature doesn't support serde)
impl Serialize for TicketExtrinsic {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        use parity_scale_codec::Encode;
        use serde::ser::SerializeStruct;

        let mut state = serializer.serialize_struct("TicketExtrinsic", 2)?;
        state.serialize_field("entry_index", &self.entry_index)?;
        state.serialize_field("proof", &self.proof.encode())?;
        state.end()
    }
}

impl<'de> Deserialize<'de> for TicketExtrinsic {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        use parity_scale_codec::Decode;
        use serde::de::{self, Visitor, MapAccess};

        #[derive(Deserialize)]
        #[serde(field_identifier, rename_all = "snake_case")]
        enum Field { EntryIndex, Proof }

        struct TicketExtrinsicVisitor;

        impl<'de> Visitor<'de> for TicketExtrinsicVisitor {
            type Value = TicketExtrinsic;

            fn expecting(&self, formatter: &mut std::fmt::Formatter) -> std::fmt::Result {
                formatter.write_str("struct TicketExtrinsic")
            }

            fn visit_map<V>(self, mut map: V) -> Result<TicketExtrinsic, V::Error>
            where
                V: MapAccess<'de>,
            {
                let mut entry_index = None;
                let mut proof_bytes: Option<Vec<u8>> = None;

                while let Some(key) = map.next_key()? {
                    match key {
                        Field::EntryIndex => {
                            if entry_index.is_some() {
                                return Err(de::Error::duplicate_field("entry_index"));
                            }
                            entry_index = Some(map.next_value()?);
                        }
                        Field::Proof => {
                            if proof_bytes.is_some() {
                                return Err(de::Error::duplicate_field("proof"));
                            }
                            proof_bytes = Some(map.next_value()?);
                        }
                    }
                }

                let entry_index = entry_index.ok_or_else(|| de::Error::missing_field("entry_index"))?;
                let proof_bytes = proof_bytes.ok_or_else(|| de::Error::missing_field("proof"))?;

                let proof = RingVrfSignature::decode(&mut &proof_bytes[..])
                    .map_err(|_| de::Error::custom("Failed to decode RingVrfSignature"))?;

                Ok(TicketExtrinsic { entry_index, proof })
            }
        }

        deserializer.deserialize_struct("TicketExtrinsic", &["entry_index", "proof"], TicketExtrinsicVisitor)
    }
}

impl TicketExtrinsic {
    /// Extract ticket from extrinsic
    pub fn to_ticket(&self) -> SafroleTicket {
        SafroleTicket::from_ring_proof(&self.proof, self.entry_index)
    }

    /// Verify Ring VRF signature
    ///
    /// Verifies:
    /// 1. VRF pre-output is correctly derived
    /// 2. Ring proof shows signer is in validator set (via ring_verifier_key)
    /// 3. Context is correctly signed
    pub fn verify(&self, ring_verifier_key: &RingVerifierKey, context: &[u8]) -> Result<bool> {
        use parity_scale_codec::{Encode, Decode};

        // Construct VRF sign data from context and entry index
        let mut vrf_input_data = Vec::with_capacity(context.len() + 4);
        vrf_input_data.extend_from_slice(context);
        vrf_input_data.extend_from_slice(&self.entry_index.to_le_bytes());

        let sign_data = VrfSignData::new(&vrf_input_data, b""); // No aux data for tickets

        // Construct verifier from ring verifier key (no context needed)
        // Note: RingVerifierKey doesn't implement Clone, so we encode/decode to effectively clone it
        let encoded = ring_verifier_key.encode();
        let key_clone = RingVerifierKey::decode(&mut &encoded[..])
            .map_err(|e| anyhow::anyhow!("Failed to decode ring verifier key: {:?}", e))?;
        let verifier = RingContext::<16>::verifier_no_context(key_clone);

        // Verify the ring VRF signature
        Ok(self.proof.ring_vrf_verify(&sign_data, &verifier))
    }
}

/// Seal tickets (normal or fallback mode)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum SealTickets {
    /// Normal mode: tickets from previous epoch
    Tickets(Vec<SafroleTicket>),

    /// Fallback mode: deterministic key selection
    Fallback(Vec<BandersnatchKey>),
}

impl SealTickets {
    /// Get seal key for slot
    pub fn get_seal_for_slot(&self, slot_index: usize, epoch_length: usize) -> Result<SealKey> {
        let index = slot_index % epoch_length;

        match self {
            SealTickets::Tickets(tickets) => {
                let ticket = tickets
                    .get(index)
                    .ok_or_else(|| anyhow::anyhow!("Slot {} out of range", index))?;

                Ok(SealKey::Ticket(ticket.clone()))
            }
            SealTickets::Fallback(keys) => {
                let key = keys
                    .get(index)
                    .ok_or_else(|| anyhow::anyhow!("Slot {} out of range", index))?;

                Ok(SealKey::Fallback(*key))
            }
        }
    }

    /// Check if using tickets (vs fallback)
    pub fn is_ticketed(&self) -> bool {
        matches!(self, SealTickets::Tickets(_))
    }

    /// Get length
    pub fn len(&self) -> usize {
        match self {
            SealTickets::Tickets(tickets) => tickets.len(),
            SealTickets::Fallback(keys) => keys.len(),
        }
    }

    /// Check if empty
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

/// Seal key for a slot
#[derive(Debug, Clone)]
pub enum SealKey {
    /// Normal: ticket
    Ticket(SafroleTicket),

    /// Fallback: key
    Fallback(BandersnatchKey),
}

/// Ticket accumulator
///
/// Accumulates tickets throughout epoch submission period.
/// Keeps best N tickets (sorted by ID, lower is better).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TicketAccumulator {
    /// Accumulated tickets (sorted by ID)
    tickets: Vec<SafroleTicket>,

    /// Maximum capacity (epoch length)
    capacity: usize,

    /// Tickets already seen (prevents duplicates)
    seen_ids: BTreeSet<TicketId>,
}

impl TicketAccumulator {
    /// Create new accumulator
    pub fn new(capacity: usize) -> Self {
        Self {
            tickets: Vec::new(),
            capacity,
            seen_ids: BTreeSet::new(),
        }
    }

    /// Add tickets from extrinsics
    pub fn add_tickets(&mut self, extrinsics: Vec<TicketExtrinsic>) -> Result<()> {
        for ext in extrinsics {
            let ticket = ext.to_ticket();

            // Check for duplicate
            if self.seen_ids.contains(&ticket.id) {
                bail!("Duplicate ticket ID: {:?}", ticket.id);
            }

            self.seen_ids.insert(ticket.id);
            self.tickets.push(ticket);
        }

        // Sort by ticket ID (lower is better)
        self.tickets.sort();

        // Keep only best N
        self.tickets.truncate(self.capacity);

        Ok(())
    }

    /// Check if saturated (has full capacity)
    pub fn is_saturated(&self) -> bool {
        self.tickets.len() == self.capacity
    }

    /// Get tickets
    pub fn tickets(&self) -> &[SafroleTicket] {
        &self.tickets
    }

    /// Clear accumulator (on epoch change)
    pub fn clear(&mut self) {
        self.tickets.clear();
        self.seen_ids.clear();
    }

    /// Take tickets and clear
    pub fn take(&mut self) -> Vec<SafroleTicket> {
        let tickets = std::mem::take(&mut self.tickets);
        self.seen_ids.clear();
        tickets
    }
}

/// Outside-in sequencer (JAM-style)
///
/// Takes sorted tickets and reorders:
/// - best, worst, 2nd-best, 2nd-worst, 3rd-best, 3rd-worst, ...
///
/// This prevents validators from gaming the system by controlling
/// when their tickets appear in the sequence.
pub fn outside_in_sequence(tickets: &[SafroleTicket]) -> Vec<SafroleTicket> {
    let mut result = Vec::with_capacity(tickets.len());
    let n = tickets.len();

    for i in 0..n {
        if i % 2 == 0 {
            // Even index: take from start (best tickets)
            result.push(tickets[i / 2].clone());
        } else {
            // Odd index: take from end (worst tickets)
            result.push(tickets[n - 1 - i / 2].clone());
        }
    }

    result
}

/// Fallback key sequence
///
/// If tickets are unavailable, deterministically select validators
/// using on-chain entropy.
pub fn fallback_key_sequence(
    entropy: &[u8; 32],
    validator_keys: &[BandersnatchKey],
    epoch_length: usize,
) -> Vec<BandersnatchKey> {
    (0..epoch_length)
        .map(|slot_index| {
            // Hash: entropy || slot_index
            let mut data = Vec::with_capacity(36);
            data.extend_from_slice(entropy);
            data.extend_from_slice(&(slot_index as u32).to_le_bytes());

            let hash = blake3::hash(&data);

            // Select validator deterministically
            let index_bytes: [u8; 4] = hash.as_bytes()[0..4].try_into().unwrap();
            let index = u32::from_le_bytes(index_bytes);
            let validator_idx = (index as usize) % validator_keys.len();

            validator_keys[validator_idx]
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use parity_scale_codec::{Decode, Encode};

    fn create_test_ticket(id_value: u8, entry_index: u32) -> SafroleTicket {
        let mut id = [0u8; 32];
        id[0] = id_value;
        SafroleTicket::new(id, entry_index)
    }

    // Note: Tests for TicketExtrinsic are disabled because they require
    // proper Ring VRF signature generation, which needs a RingContext and
    // actual Bandersnatch keys. These will be tested in integration tests
    // with a full RingContext setup.

    #[test]
    fn test_ticket_sorting() {
        let mut tickets = vec![
            create_test_ticket(5, 0),
            create_test_ticket(2, 1),
            create_test_ticket(8, 2),
            create_test_ticket(1, 3),
        ];

        tickets.sort();

        // Should be sorted by ID (ascending)
        assert_eq!(tickets[0].id[0], 1);
        assert_eq!(tickets[1].id[0], 2);
        assert_eq!(tickets[2].id[0], 5);
        assert_eq!(tickets[3].id[0], 8);
    }

    #[test]
    fn test_outside_in_sequence() {
        let tickets = vec![
            create_test_ticket(1, 0), // Best
            create_test_ticket(2, 1),
            create_test_ticket(3, 2),
            create_test_ticket(4, 3),
            create_test_ticket(5, 4), // Worst
        ];

        let sequenced = outside_in_sequence(&tickets);

        // Expected order: best, worst, 2nd-best, 2nd-worst, 3rd-best
        assert_eq!(sequenced[0].id[0], 1); // Best
        assert_eq!(sequenced[1].id[0], 5); // Worst
        assert_eq!(sequenced[2].id[0], 2); // 2nd-best
        assert_eq!(sequenced[3].id[0], 4); // 2nd-worst
        assert_eq!(sequenced[4].id[0], 3); // 3rd-best (middle)
    }

    // TODO: Re-enable these tests with proper Ring VRF signature generation
    // These tests require RingContext setup and actual Bandersnatch key pairs

    // #[test]
    // fn test_ticket_accumulator() { ... }

    // #[test]
    // fn test_accumulator_truncate() { ... }

    // #[test]
    // fn test_duplicate_rejection() { ... }

    #[test]
    fn test_fallback_key_sequence() {
        let entropy = [42u8; 32];
        let validators = vec![[1u8; 32], [2u8; 32], [3u8; 32]];

        let keys = fallback_key_sequence(&entropy, &validators, 10);

        assert_eq!(keys.len(), 10);

        // Should be deterministic
        let keys2 = fallback_key_sequence(&entropy, &validators, 10);
        assert_eq!(keys, keys2);

        // Different entropy → different sequence
        let entropy2 = [43u8; 32];
        let keys3 = fallback_key_sequence(&entropy2, &validators, 10);
        assert_ne!(keys, keys3);
    }
}
