//! Note-Based Staking (Penumbra-style)
//!
//! Staking without accounts - everything is notes with commitments and nullifiers.
//! Designed for maximum privacy and composability with Penumbra's shielded pool.

use super::{AccountId, Balance, EraIndex, ValidatorIndex};
use anyhow::{bail, Result};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

/// Stake Note
///
/// Similar to Penumbra's Note, but represents staked tokens:
/// - amount: How much ZT is staked
/// - validator_commitment: Which validators nominated (can be hidden)
/// - nullifier: Prevents double-spending the stake
/// - note_commitment: Pedersen commitment to (amount, validator_choice, blinding)
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct StakeNote {
    /// Note commitment (Pedersen commitment)
    /// commit(amount || validator_choices || blinding)
    pub note_commitment: NoteCommitment,

    /// Nullifier (prevents double-spend)
    /// nk = hash(note_commitment || position)
    pub nullifier: Nullifier,

    /// Era when note was created
    pub creation_era: EraIndex,

    /// Era when note can be consumed (for unbonding)
    pub maturity_era: EraIndex,

    /// Encrypted payload (only validator can decrypt if nominated)
    pub encrypted_payload: EncryptedStakePayload,
}

/// Note commitment (Pedersen commitment)
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct NoteCommitment(pub [u8; 32]);

/// Nullifier (prevents double-spend)
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Nullifier(pub [u8; 32]);

/// Encrypted stake payload
///
/// Only the nominated validators can decrypt to see:
/// - amount: How much was staked
/// - validator_choices: Which validators were nominated
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct EncryptedStakePayload {
    /// Encrypted amount and validator choices
    /// Can be trial-decrypted by validators
    pub ciphertext: Vec<u8>,

    /// Ephemeral public key (for ECDH key agreement)
    pub ephemeral_key: [u8; 32],
}

/// Decrypted stake payload (known only to nominated validators)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StakePayload {
    /// Amount staked
    pub amount: Balance,

    /// Validator choices (up to 16)
    pub validator_choices: Vec<ValidatorIndex>,

    /// Blinding factor (for commitment)
    pub blinding: [u8; 32],
}

impl StakePayload {
    /// Calculate note commitment
    pub fn compute_commitment(&self) -> NoteCommitment {
        // TODO: Use Pedersen commitment with decaf377
        // commit = amount * G + validator_hash * H + blinding * B
        let mut hasher = blake3::Hasher::new();
        hasher.update(&self.amount.to_le_bytes());

        for validator_idx in &self.validator_choices {
            hasher.update(&validator_idx.to_le_bytes());
        }

        hasher.update(&self.blinding);

        let hash = hasher.finalize();
        NoteCommitment(hash.into())
    }

    /// Calculate nullifier
    pub fn compute_nullifier(&self, position: u64) -> Nullifier {
        // nk = hash(note_commitment || position)
        let commitment = self.compute_commitment();

        let mut hasher = blake3::Hasher::new();
        hasher.update(&commitment.0);
        hasher.update(&position.to_le_bytes());

        let hash = hasher.finalize();
        Nullifier(hash.into())
    }
}

/// Note Tree State (ZODA-encoded)
///
/// All stake notes in the current era.
/// No account balances - just a set of unspent notes!
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NoteTreeState {
    /// Current era
    pub era: EraIndex,

    /// All unspent stake notes
    pub unspent_notes: BTreeMap<NoteCommitment, StakeNote>,

    /// Spent nullifiers (prevents double-spend)
    pub spent_nullifiers: BTreeSet<Nullifier>,

    /// Validator set for this era
    pub validator_set: ValidatorSet,

    /// Total staked (public aggregate, but individual amounts hidden)
    pub total_staked: Balance,

    /// Note tree root (for Merkle proofs)
    pub note_tree_root: [u8; 32],
}

impl NoteTreeState {
    /// Create new note tree for era
    pub fn new(era: EraIndex, validator_set: ValidatorSet) -> Self {
        Self {
            era,
            unspent_notes: BTreeMap::new(),
            spent_nullifiers: BTreeSet::new(),
            validator_set,
            total_staked: 0,
            note_tree_root: [0u8; 32],
        }
    }

    /// Add stake note (create new stake)
    pub fn add_note(&mut self, note: StakeNote) -> Result<()> {
        // Check nullifier not already spent
        if self.spent_nullifiers.contains(&note.nullifier) {
            bail!("Nullifier already spent - double-spend attempt!");
        }

        // Add to unspent set
        self.unspent_notes.insert(note.note_commitment, note);

        // Update tree root
        self.recompute_tree_root();

        Ok(())
    }

    /// Consume note (spend stake)
    pub fn consume_note(&mut self, note_commitment: NoteCommitment) -> Result<StakeNote> {
        // Remove from unspent
        let note = self
            .unspent_notes
            .remove(&note_commitment)
            .ok_or_else(|| anyhow::anyhow!("Note not found"))?;

        // Mark nullifier as spent
        self.spent_nullifiers.insert(note.nullifier);

        // Update tree root
        self.recompute_tree_root();

        Ok(note)
    }

    /// Check if note exists and is unspent
    pub fn is_unspent(&self, note_commitment: &NoteCommitment) -> bool {
        self.unspent_notes.contains_key(note_commitment)
    }

    /// Check if nullifier was spent
    pub fn is_spent(&self, nullifier: &Nullifier) -> bool {
        self.spent_nullifiers.contains(nullifier)
    }

    /// Recompute Merkle tree root
    fn recompute_tree_root(&mut self) {
        // TODO: Proper Merkle tree implementation
        let mut hasher = blake3::Hasher::new();

        for (commitment, _) in &self.unspent_notes {
            hasher.update(&commitment.0);
        }

        self.note_tree_root = hasher.finalize().into();
    }

    /// Get note count
    pub fn note_count(&self) -> usize {
        self.unspent_notes.len()
    }
}

/// Validator Set (15 validators from Phragmén election)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ValidatorSet {
    /// Era number
    pub era: EraIndex,

    /// 15 elected validators
    pub validators: Vec<ValidatorInfo>,

    /// FROST public key (11/15 threshold)
    pub frost_pubkey: Option<[u8; 32]>,
}

impl ValidatorSet {
    pub fn new(era: EraIndex) -> Self {
        Self {
            era,
            validators: Vec::new(),
            frost_pubkey: None,
        }
    }

    pub fn validator_count(&self) -> usize {
        self.validators.len()
    }

    pub fn get_validator(&self, index: ValidatorIndex) -> Option<&ValidatorInfo> {
        self.validators.get(index as usize)
    }
}

/// Validator Info (minimal, privacy-preserving)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ValidatorInfo {
    /// Validator index (0-14)
    pub index: ValidatorIndex,

    /// Validator account (public)
    pub account: AccountId,

    /// Consensus key (for block production)
    pub consensus_key: [u8; 32],

    /// FROST key share (for multisig)
    pub frost_key: [u8; 32],

    /// Total backing (aggregate, not per-nominator)
    pub total_backing: Balance,
}

/// Era Transition Action
///
/// Describes how to transform era N state → era N+1 state.
/// This is what gets ZODA-encoded and proven with Ligerito!
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum EraTransitionAction {
    /// Consume old stake note → produce new stake note
    /// (continues staking into next era)
    RolloverStake {
        old_note: NoteCommitment,
        new_note: StakeNote,
    },

    /// Consume stake note → produce reward note
    /// (unstaking, receives rewards)
    ClaimRewards {
        old_note: NoteCommitment,
        reward_amount: Balance,
        reward_note: StakeNote,
    },

    /// New stake (no old note consumed)
    NewStake {
        new_note: StakeNote,
    },

    /// Update validator set
    UpdateValidators {
        old_validator_set: ValidatorSet,
        new_validator_set: ValidatorSet,
    },
}

/// Era Transition (ZODA-encoded state transition)
///
/// This is the core data structure that gets:
/// 1. ZODA-encoded (becomes executable + commitment)
/// 2. Proven with Ligerito (validity proof)
/// 3. Executed in PolkaVM (state transition)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EraTransition {
    /// Source era
    pub from_era: EraIndex,

    /// Target era
    pub to_era: EraIndex,

    /// Input state root (era N note tree)
    pub input_state_root: [u8; 32],

    /// Output state root (era N+1 note tree)
    pub output_state_root: [u8; 32],

    /// Actions to apply
    pub actions: Vec<EraTransitionAction>,

    /// Phragmén election result (new validator set)
    pub new_validator_set: ValidatorSet,

    /// Total rewards distributed this era
    pub total_rewards: Balance,

    /// FROST signature (11/15 validators authorize transition)
    pub frost_signature: Option<[u8; 64]>,
}

impl EraTransition {
    /// Create new era transition
    pub fn new(from_era: EraIndex, to_era: EraIndex) -> Self {
        Self {
            from_era,
            to_era,
            input_state_root: [0u8; 32],
            output_state_root: [0u8; 32],
            actions: Vec::new(),
            new_validator_set: ValidatorSet::new(to_era),
            total_rewards: 0,
            frost_signature: None,
        }
    }

    /// Add action to transition
    pub fn add_action(&mut self, action: EraTransitionAction) {
        self.actions.push(action);
    }

    /// Apply transition to state
    ///
    /// This is what gets executed in PolkaVM!
    pub fn apply(&self, state: &mut NoteTreeState) -> Result<()> {
        // Verify era progression
        if state.era != self.from_era {
            bail!("State era mismatch: expected {}, got {}", self.from_era, state.era);
        }

        // Verify input state root
        if state.note_tree_root != self.input_state_root {
            bail!("Input state root mismatch");
        }

        // Apply each action
        for action in &self.actions {
            match action {
                EraTransitionAction::RolloverStake { old_note, new_note } => {
                    // Consume old note
                    state.consume_note(*old_note)?;

                    // Produce new note
                    state.add_note(new_note.clone())?;
                }

                EraTransitionAction::ClaimRewards { old_note, reward_amount, reward_note } => {
                    // Consume old note
                    state.consume_note(*old_note)?;

                    // Produce reward note
                    state.add_note(reward_note.clone())?;

                    // Update total rewards
                    state.total_staked = state
                        .total_staked
                        .checked_add(*reward_amount)
                        .ok_or_else(|| anyhow::anyhow!("Overflow adding rewards"))?;
                }

                EraTransitionAction::NewStake { new_note } => {
                    // Just add new note
                    state.add_note(new_note.clone())?;
                }

                EraTransitionAction::UpdateValidators { new_validator_set, .. } => {
                    // Update validator set
                    state.validator_set = new_validator_set.clone();
                }
            }
        }

        // Update era
        state.era = self.to_era;

        // Verify output state root
        if state.note_tree_root != self.output_state_root {
            bail!("Output state root mismatch");
        }

        tracing::info!(
            "Applied era transition {} → {} ({} actions, {} rewards distributed)",
            self.from_era,
            self.to_era,
            self.actions.len(),
            self.total_rewards
        );

        Ok(())
    }

    /// Encode transition as ZODA
    ///
    /// This makes the transition:
    /// 1. Executable in PolkaVM
    /// 2. Committable with Ligerito
    /// 3. Verifiable by light clients
    pub fn encode_as_zoda(&self) -> Result<Vec<u8>> {
        // TODO: Actual ZODA encoding
        // For now, just serialize
        bincode::serialize(self).map_err(|e| anyhow::anyhow!("ZODA encoding failed: {}", e))
    }

    /// Verify Ligerito proof of transition
    pub fn verify_ligerito_proof(&self, proof: &[u8]) -> Result<bool> {
        // TODO: Actual Ligerito verification
        // Should verify that:
        // 1. Phragmén election was run correctly
        // 2. All note consumptions are valid
        // 3. Reward distribution is correct
        // 4. FROST signature is valid (11/15)

        tracing::info!("Verifying Ligerito proof for era transition {} → {}", self.from_era, self.to_era);

        Ok(true)
    }
}

/// Staking Action (what users submit)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum StakingAction {
    /// Stake tokens (create new note)
    Stake {
        amount: Balance,
        validator_choices: Vec<ValidatorIndex>,
        blinding: [u8; 32],
    },

    /// Unstake (will be included in next era transition)
    Unstake {
        note_commitment: NoteCommitment,
        auth_signature: [u8; 64], // Proves ownership
    },

    /// Restake (automatically rollover to next era)
    Restake {
        note_commitment: NoteCommitment,
        auth_signature: [u8; 64],
    },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_note_commitment() {
        let payload = StakePayload {
            amount: 1000 * 10u128.pow(18),
            validator_choices: vec![0, 1, 2],
            blinding: [42u8; 32],
        };

        let commitment = payload.compute_commitment();

        // Same payload → same commitment
        let commitment2 = payload.compute_commitment();
        assert_eq!(commitment, commitment2);

        // Different payload → different commitment
        let payload3 = StakePayload {
            amount: 2000 * 10u128.pow(18),
            ..payload.clone()
        };
        let commitment3 = payload3.compute_commitment();
        assert_ne!(commitment, commitment3);
    }

    #[test]
    fn test_nullifier() {
        let payload = StakePayload {
            amount: 1000 * 10u128.pow(18),
            validator_choices: vec![0, 1, 2],
            blinding: [42u8; 32],
        };

        let nullifier1 = payload.compute_nullifier(0);
        let nullifier2 = payload.compute_nullifier(1);

        // Different positions → different nullifiers
        assert_ne!(nullifier1, nullifier2);
    }

    #[test]
    fn test_note_tree_state() {
        let mut state = NoteTreeState::new(1, ValidatorSet::new(1));

        let payload = StakePayload {
            amount: 1000 * 10u128.pow(18),
            validator_choices: vec![0, 1, 2],
            blinding: [42u8; 32],
        };

        let note = StakeNote {
            note_commitment: payload.compute_commitment(),
            nullifier: payload.compute_nullifier(0),
            creation_era: 1,
            maturity_era: 1,
            encrypted_payload: EncryptedStakePayload {
                ciphertext: vec![],
                ephemeral_key: [0u8; 32],
            },
        };

        // Add note
        state.add_note(note.clone()).unwrap();
        assert_eq!(state.note_count(), 1);
        assert!(state.is_unspent(&note.note_commitment));
        assert!(!state.is_spent(&note.nullifier));

        // Consume note
        let consumed = state.consume_note(note.note_commitment).unwrap();
        assert_eq!(consumed.nullifier, note.nullifier);
        assert_eq!(state.note_count(), 0);
        assert!(!state.is_unspent(&note.note_commitment));
        assert!(state.is_spent(&note.nullifier));
    }

    #[test]
    fn test_double_spend_prevention() {
        let mut state = NoteTreeState::new(1, ValidatorSet::new(1));

        let payload = StakePayload {
            amount: 1000 * 10u128.pow(18),
            validator_choices: vec![0, 1, 2],
            blinding: [42u8; 32],
        };

        let note = StakeNote {
            note_commitment: payload.compute_commitment(),
            nullifier: payload.compute_nullifier(0),
            creation_era: 1,
            maturity_era: 1,
            encrypted_payload: EncryptedStakePayload {
                ciphertext: vec![],
                ephemeral_key: [0u8; 32],
            },
        };

        // Add note
        state.add_note(note.clone()).unwrap();

        // Consume it
        state.consume_note(note.note_commitment).unwrap();

        // Try to add same note again (double-spend)
        let result = state.add_note(note.clone());
        assert!(result.is_err());
    }

    #[test]
    fn test_era_transition() {
        let mut state = NoteTreeState::new(1, ValidatorSet::new(1));

        // Create initial note
        let payload1 = StakePayload {
            amount: 1000 * 10u128.pow(18),
            validator_choices: vec![0, 1, 2],
            blinding: [42u8; 32],
        };

        let note1 = StakeNote {
            note_commitment: payload1.compute_commitment(),
            nullifier: payload1.compute_nullifier(0),
            creation_era: 1,
            maturity_era: 1,
            encrypted_payload: EncryptedStakePayload {
                ciphertext: vec![],
                ephemeral_key: [0u8; 32],
            },
        };

        state.add_note(note1.clone()).unwrap();

        let input_root = state.note_tree_root;

        // Create transition to era 2
        let mut transition = EraTransition::new(1, 2);
        transition.input_state_root = input_root;

        // Create new note for era 2
        let payload2 = StakePayload {
            amount: 1000 * 10u128.pow(18),
            validator_choices: vec![0, 1, 2],
            blinding: [43u8; 32],
        };

        let note2 = StakeNote {
            note_commitment: payload2.compute_commitment(),
            nullifier: payload2.compute_nullifier(1),
            creation_era: 2,
            maturity_era: 2,
            encrypted_payload: EncryptedStakePayload {
                ciphertext: vec![],
                ephemeral_key: [0u8; 32],
            },
        };

        // Rollover stake
        transition.add_action(EraTransitionAction::RolloverStake {
            old_note: note1.note_commitment,
            new_note: note2.clone(),
        });

        transition.output_state_root = {
            let mut temp_state = state.clone();
            temp_state.consume_note(note1.note_commitment).unwrap();
            temp_state.add_note(note2.clone()).unwrap();
            temp_state.note_tree_root
        };

        // Apply transition
        transition.apply(&mut state).unwrap();

        // Verify state
        assert_eq!(state.era, 2);
        assert!(!state.is_unspent(&note1.note_commitment));
        assert!(state.is_unspent(&note2.note_commitment));
        assert!(state.is_spent(&note1.nullifier));
    }
}
