//! replay protection for relay-based coordination
//!
//! messages include state_hash and sequence number to prevent replay attacks.
//! validators reject messages that reference stale state or duplicate sequences.
//!
//! # state binding
//!
//! each message is bound to a specific syndicate state via state_hash.
//! if state changes (new proposal passed, membership changed, reshare),
//! messages referencing old state become invalid.
//!
//! # sequence numbers
//!
//! monotonically increasing per-sender sequence prevents replay within
//! the same state epoch. each member tracks their own sequence.

use alloc::collections::BTreeMap;
use sha2::{Digest, Sha256};

use crate::wire::{Hash32, Envelope, SyndicateState};

/// result of replay validation
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ReplayCheck {
    /// message is valid
    Valid,
    /// state hash doesn't match current state
    StaleState {
        expected: Hash32,
        got: Hash32,
    },
    /// sequence already seen or too low
    DuplicateSequence {
        sender: Hash32,
        expected_min: u64,
        got: u64,
    },
    /// message is from the future (sequence too high)
    FutureSequence {
        sender: Hash32,
        current_max: u64,
        got: u64,
    },
    /// sender not recognized
    UnknownSender {
        pubkey: Hash32,
    },
}

/// replay protection validator
#[derive(Clone, Debug)]
pub struct ReplayValidator {
    /// current state hash
    state_hash: Hash32,
    /// highest sequence seen per sender
    sequences: BTreeMap<Hash32, u64>,
    /// maximum allowed sequence gap (prevents DoS via large sequences)
    max_sequence_gap: u64,
}

impl ReplayValidator {
    /// create validator with initial state
    pub fn new(state_hash: Hash32) -> Self {
        Self {
            state_hash,
            sequences: BTreeMap::new(),
            max_sequence_gap: 1000, // reasonable gap for async coordination
        }
    }

    /// create from syndicate state
    pub fn from_state(state: &SyndicateState) -> Self {
        let state_hash = compute_state_hash(state);
        let mut sequences = BTreeMap::new();

        // initialize sequences for all members
        for member in &state.members {
            sequences.insert(member.pubkey, 0);
        }

        Self {
            state_hash,
            sequences,
            max_sequence_gap: 1000,
        }
    }

    /// set maximum allowed sequence gap
    pub fn with_max_gap(mut self, gap: u64) -> Self {
        self.max_sequence_gap = gap;
        self
    }

    /// get current state hash
    pub fn state_hash(&self) -> &Hash32 {
        &self.state_hash
    }

    /// update state hash (after state change)
    pub fn update_state(&mut self, new_state_hash: Hash32) {
        self.state_hash = new_state_hash;
        // sequences persist across state changes - they're per-sender
    }

    /// add known sender (new member)
    pub fn add_sender(&mut self, pubkey: Hash32) {
        self.sequences.entry(pubkey).or_insert(0);
    }

    /// remove sender (member left)
    pub fn remove_sender(&mut self, pubkey: &Hash32) {
        self.sequences.remove(pubkey);
    }

    /// validate envelope against replay
    pub fn validate(&self, envelope: &Envelope, sender_pubkey: &Hash32) -> ReplayCheck {
        // check state hash
        if envelope.state_hash != self.state_hash {
            return ReplayCheck::StaleState {
                expected: self.state_hash,
                got: envelope.state_hash,
            };
        }

        // check sender is known
        let last_seq = match self.sequences.get(sender_pubkey) {
            Some(&seq) => seq,
            None => {
                return ReplayCheck::UnknownSender {
                    pubkey: *sender_pubkey,
                };
            }
        };

        // check sequence
        if envelope.sequence <= last_seq {
            return ReplayCheck::DuplicateSequence {
                sender: *sender_pubkey,
                expected_min: last_seq + 1,
                got: envelope.sequence,
            };
        }

        // check for sequence too far in future (DoS prevention)
        if envelope.sequence > last_seq + self.max_sequence_gap {
            return ReplayCheck::FutureSequence {
                sender: *sender_pubkey,
                current_max: last_seq + self.max_sequence_gap,
                got: envelope.sequence,
            };
        }

        ReplayCheck::Valid
    }

    /// record envelope as seen (call after successful validation)
    pub fn record(&mut self, sender_pubkey: &Hash32, sequence: u64) {
        if let Some(seq) = self.sequences.get_mut(sender_pubkey) {
            if sequence > *seq {
                *seq = sequence;
            }
        }
    }

    /// validate and record in one step
    pub fn validate_and_record(
        &mut self,
        envelope: &Envelope,
        sender_pubkey: &Hash32,
    ) -> ReplayCheck {
        let result = self.validate(envelope, sender_pubkey);
        if result == ReplayCheck::Valid {
            self.record(sender_pubkey, envelope.sequence);
        }
        result
    }

    /// get next sequence for sender
    pub fn next_sequence(&self, sender_pubkey: &Hash32) -> Option<u64> {
        self.sequences.get(sender_pubkey).map(|&s| s + 1)
    }
}

/// compute state hash from syndicate state
#[cfg(feature = "borsh")]
pub fn compute_state_hash(state: &SyndicateState) -> Hash32 {
    let bytes = borsh::to_vec(state).unwrap_or_default();
    Sha256::digest(&bytes).into()
}

#[cfg(not(feature = "borsh"))]
pub fn compute_state_hash(state: &SyndicateState) -> Hash32 {
    // without borsh, use a simpler hash of key fields
    let mut hasher = Sha256::new();
    hasher.update(&state.syndicate_id);
    hasher.update(&state.epoch.to_le_bytes());
    hasher.update(&state.sequence.to_le_bytes());
    hasher.finalize().into()
}

/// envelope builder with replay protection
pub struct EnvelopeBuilder {
    syndicate_id: Hash32,
    state_hash: Hash32,
    sequence: u64,
}

impl EnvelopeBuilder {
    /// create builder from validator
    pub fn new(
        syndicate_id: Hash32,
        validator: &ReplayValidator,
        sender_pubkey: &Hash32,
    ) -> Option<Self> {
        let sequence = validator.next_sequence(sender_pubkey)?;
        Some(Self {
            syndicate_id,
            state_hash: *validator.state_hash(),
            sequence,
        })
    }

    /// create builder with explicit values
    pub fn with_values(syndicate_id: Hash32, state_hash: Hash32, sequence: u64) -> Self {
        Self {
            syndicate_id,
            state_hash,
            sequence,
        }
    }

    /// get syndicate id
    pub fn syndicate_id(&self) -> &Hash32 {
        &self.syndicate_id
    }

    /// get state hash
    pub fn state_hash(&self) -> &Hash32 {
        &self.state_hash
    }

    /// get sequence
    pub fn sequence(&self) -> u64 {
        self.sequence
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::wire::{MessagePayload, SyncRequest};

    fn make_envelope(state_hash: Hash32, sequence: u64) -> Envelope {
        Envelope {
            version: 1,
            syndicate_id: [1u8; 32],
            state_hash,
            sequence,
            payload: MessagePayload::SyncRequest(SyncRequest {
                current_state_hash: [0u8; 32],
                current_sequence: 0,
            }),
            signature: [0u8; 64],
        }
    }

    #[test]
    fn test_replay_validator_basic() {
        let state_hash = [1u8; 32];
        let mut validator = ReplayValidator::new(state_hash);

        let alice = [2u8; 32];
        validator.add_sender(alice);

        // first message is valid
        let env1 = make_envelope(state_hash, 1);
        assert_eq!(validator.validate(&env1, &alice), ReplayCheck::Valid);
        validator.record(&alice, 1);

        // replay fails
        let env1_replay = make_envelope(state_hash, 1);
        assert!(matches!(
            validator.validate(&env1_replay, &alice),
            ReplayCheck::DuplicateSequence { .. }
        ));

        // next sequence works
        let env2 = make_envelope(state_hash, 2);
        assert_eq!(validator.validate(&env2, &alice), ReplayCheck::Valid);
    }

    #[test]
    fn test_stale_state() {
        let state_hash = [1u8; 32];
        let mut validator = ReplayValidator::new(state_hash);

        let alice = [2u8; 32];
        validator.add_sender(alice);

        // message with wrong state hash
        let old_state = [0u8; 32];
        let env = make_envelope(old_state, 1);
        assert!(matches!(
            validator.validate(&env, &alice),
            ReplayCheck::StaleState { .. }
        ));
    }

    #[test]
    fn test_unknown_sender() {
        let state_hash = [1u8; 32];
        let validator = ReplayValidator::new(state_hash);

        let unknown = [99u8; 32];
        let env = make_envelope(state_hash, 1);
        assert!(matches!(
            validator.validate(&env, &unknown),
            ReplayCheck::UnknownSender { .. }
        ));
    }

    #[test]
    fn test_future_sequence() {
        let state_hash = [1u8; 32];
        let mut validator = ReplayValidator::new(state_hash).with_max_gap(10);

        let alice = [2u8; 32];
        validator.add_sender(alice);

        // sequence too far in future
        let env = make_envelope(state_hash, 100);
        assert!(matches!(
            validator.validate(&env, &alice),
            ReplayCheck::FutureSequence { .. }
        ));

        // within gap is ok
        let env = make_envelope(state_hash, 5);
        assert_eq!(validator.validate(&env, &alice), ReplayCheck::Valid);
    }

    #[test]
    fn test_validate_and_record() {
        let state_hash = [1u8; 32];
        let mut validator = ReplayValidator::new(state_hash);

        let alice = [2u8; 32];
        validator.add_sender(alice);

        let env1 = make_envelope(state_hash, 1);
        assert_eq!(
            validator.validate_and_record(&env1, &alice),
            ReplayCheck::Valid
        );

        // now 1 is recorded, replay fails
        let env1_replay = make_envelope(state_hash, 1);
        assert!(matches!(
            validator.validate_and_record(&env1_replay, &alice),
            ReplayCheck::DuplicateSequence { .. }
        ));
    }

    #[test]
    fn test_state_update() {
        let state1 = [1u8; 32];
        let mut validator = ReplayValidator::new(state1);

        let alice = [2u8; 32];
        validator.add_sender(alice);

        // valid for state1
        let env = make_envelope(state1, 1);
        assert_eq!(
            validator.validate_and_record(&env, &alice),
            ReplayCheck::Valid
        );

        // update state
        let state2 = [2u8; 32];
        validator.update_state(state2);

        // message with old state is stale
        let env_old = make_envelope(state1, 2);
        assert!(matches!(
            validator.validate(&env_old, &alice),
            ReplayCheck::StaleState { .. }
        ));

        // message with new state works
        // sequences persist, so need seq > 1
        let env_new = make_envelope(state2, 2);
        assert_eq!(validator.validate(&env_new, &alice), ReplayCheck::Valid);
    }

    #[test]
    fn test_envelope_builder() {
        let state_hash = [1u8; 32];
        let mut validator = ReplayValidator::new(state_hash);

        let alice = [2u8; 32];
        validator.add_sender(alice);

        let syndicate_id = [3u8; 32];
        let builder = EnvelopeBuilder::new(syndicate_id, &validator, &alice).unwrap();

        assert_eq!(builder.sequence(), 1);
        assert_eq!(builder.state_hash(), &state_hash);
    }
}
