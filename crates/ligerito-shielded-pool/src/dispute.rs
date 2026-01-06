//! dispute proofs for poker state channels
//!
//! on-chain verifiable proofs for settling disputes when players:
//! - submit invalid shuffle proofs
//! - reveal wrong decryption shares
//! - produce duplicate cards (deck corruption)
//! - timeout on required actions
//! - sign invalid state transitions
//!
//! all proofs are self-contained for on-chain verification

#[cfg(not(feature = "std"))]
use alloc::vec::Vec;

use crate::channel::{ChannelId, SignedState};
use crate::keys::PublicKey;
use crate::poker::GamePhase;
use crate::value::Amount;

/// dispute types that can be proven on-chain
#[derive(Clone, Debug)]
pub enum DisputeType {
    /// shuffle proof doesn't verify
    InvalidShuffle,
    /// decryption share produces wrong card
    InvalidReveal,
    /// two positions decrypt to same card
    DuplicateCard,
    /// player didn't act within timeout
    Timeout,
    /// signed state doesn't match valid transition
    InvalidTransition,
    /// player bet more than their balance
    InsufficientBalance,
    /// signature on state is invalid
    InvalidSignature,
}

/// compact proof for on-chain dispute resolution
#[derive(Clone, Debug)]
pub struct DisputeProof {
    /// type of dispute
    pub dispute_type: DisputeType,
    /// channel being disputed
    pub channel_id: ChannelId,
    /// hand number (for sequencing)
    pub hand_number: u64,
    /// accused player
    pub accused: PublicKey,
    /// evidence (serialized, type-specific)
    pub evidence: Vec<u8>,
    /// hash of full evidence (for large proofs stored off-chain)
    pub evidence_hash: [u8; 32],
}

impl DisputeProof {
    /// compute commitment to this proof
    pub fn commitment(&self) -> [u8; 32] {
        let mut hasher = blake3::Hasher::new();
        hasher.update(b"ligerito.dispute.v1");
        hasher.update(&[self.dispute_type.to_u8()]);
        hasher.update(&self.channel_id.0);
        hasher.update(&self.hand_number.to_le_bytes());
        hasher.update(&self.accused.0);
        hasher.update(&self.evidence_hash);
        *hasher.finalize().as_bytes()
    }

    /// serialize for on-chain submission
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut bytes = Vec::new();
        bytes.push(self.dispute_type.to_u8());
        bytes.extend_from_slice(&self.channel_id.0);
        bytes.extend_from_slice(&self.hand_number.to_le_bytes());
        bytes.extend_from_slice(&self.accused.0);
        bytes.extend_from_slice(&(self.evidence.len() as u32).to_le_bytes());
        bytes.extend_from_slice(&self.evidence);
        bytes.extend_from_slice(&self.evidence_hash);
        bytes
    }
}

impl DisputeType {
    pub fn to_u8(&self) -> u8 {
        match self {
            DisputeType::InvalidShuffle => 0,
            DisputeType::InvalidReveal => 1,
            DisputeType::DuplicateCard => 2,
            DisputeType::Timeout => 3,
            DisputeType::InvalidTransition => 4,
            DisputeType::InsufficientBalance => 5,
            DisputeType::InvalidSignature => 6,
        }
    }

    pub fn from_u8(v: u8) -> Option<Self> {
        match v {
            0 => Some(DisputeType::InvalidShuffle),
            1 => Some(DisputeType::InvalidReveal),
            2 => Some(DisputeType::DuplicateCard),
            3 => Some(DisputeType::Timeout),
            4 => Some(DisputeType::InvalidTransition),
            5 => Some(DisputeType::InsufficientBalance),
            6 => Some(DisputeType::InvalidSignature),
            _ => None,
        }
    }
}

// === evidence types ===

/// evidence for invalid shuffle dispute
#[derive(Clone, Debug)]
pub struct ShuffleEvidence {
    /// player who shuffled
    pub shuffler: PublicKey,
    /// input deck commitment (before shuffle)
    pub input_commitment: [u8; 32],
    /// output deck commitment (after shuffle)
    pub output_commitment: [u8; 32],
    /// the shuffle proof that failed verification
    pub proof_hash: [u8; 32],
    /// aggregate public key used
    pub aggregate_pk: [u8; 32],
}

impl ShuffleEvidence {
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut bytes = Vec::with_capacity(128);
        bytes.extend_from_slice(&self.shuffler.0);
        bytes.extend_from_slice(&self.input_commitment);
        bytes.extend_from_slice(&self.output_commitment);
        bytes.extend_from_slice(&self.proof_hash);
        bytes.extend_from_slice(&self.aggregate_pk);
        bytes
    }

    pub fn from_bytes(bytes: &[u8]) -> Option<Self> {
        if bytes.len() < 160 {
            return None;
        }
        let mut shuffler = [0u8; 32];
        shuffler.copy_from_slice(&bytes[0..32]);
        let mut input_commitment = [0u8; 32];
        input_commitment.copy_from_slice(&bytes[32..64]);
        let mut output_commitment = [0u8; 32];
        output_commitment.copy_from_slice(&bytes[64..96]);
        let mut proof_hash = [0u8; 32];
        proof_hash.copy_from_slice(&bytes[96..128]);
        let mut aggregate_pk = [0u8; 32];
        aggregate_pk.copy_from_slice(&bytes[128..160]);
        Some(Self {
            shuffler: PublicKey(shuffler),
            input_commitment,
            output_commitment,
            proof_hash,
            aggregate_pk,
        })
    }

    pub fn hash(&self) -> [u8; 32] {
        *blake3::hash(&self.to_bytes()).as_bytes()
    }
}

/// evidence for invalid reveal dispute
#[derive(Clone, Debug)]
pub struct RevealEvidence {
    /// player who revealed
    pub revealer: PublicKey,
    /// card position in deck
    pub position: u8,
    /// encrypted card (from shuffled deck)
    pub encrypted_card: [u8; 64],
    /// claimed decryption share
    pub decryption_share: [u8; 32],
    /// the revealed card value
    pub claimed_card: u8,
    /// proof that decryption is wrong (Chaum-Pedersen)
    pub chaum_pedersen_proof: [u8; 96],
}

impl RevealEvidence {
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut bytes = Vec::with_capacity(226);
        bytes.extend_from_slice(&self.revealer.0);
        bytes.push(self.position);
        bytes.extend_from_slice(&self.encrypted_card);
        bytes.extend_from_slice(&self.decryption_share);
        bytes.push(self.claimed_card);
        bytes.extend_from_slice(&self.chaum_pedersen_proof);
        bytes
    }

    pub fn hash(&self) -> [u8; 32] {
        *blake3::hash(&self.to_bytes()).as_bytes()
    }
}

/// evidence for duplicate card dispute
#[derive(Clone, Debug)]
pub struct DuplicateCardEvidence {
    /// the duplicated card value
    pub card_value: u8,
    /// first position where card appeared
    pub position_a: u8,
    /// second position where card appeared
    pub position_b: u8,
    /// reveal proof for position A
    pub reveal_a: RevealEvidence,
    /// reveal proof for position B
    pub reveal_b: RevealEvidence,
}

impl DuplicateCardEvidence {
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut bytes = Vec::new();
        bytes.push(self.card_value);
        bytes.push(self.position_a);
        bytes.push(self.position_b);
        bytes.extend_from_slice(&self.reveal_a.to_bytes());
        bytes.extend_from_slice(&self.reveal_b.to_bytes());
        bytes
    }

    pub fn hash(&self) -> [u8; 32] {
        *blake3::hash(&self.to_bytes()).as_bytes()
    }
}

/// evidence for timeout dispute
#[derive(Clone, Debug)]
pub struct TimeoutEvidence {
    /// player who timed out
    pub player: PublicKey,
    /// the signed state showing it was their turn
    pub signed_state: SignedStateCompact,
    /// block when action was required
    pub required_by_block: u64,
    /// current block (proving timeout)
    pub current_block: u64,
    /// game phase showing action was required
    pub required_phase: GamePhase,
}

impl TimeoutEvidence {
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut bytes = Vec::new();
        bytes.extend_from_slice(&self.player.0);
        bytes.extend_from_slice(&self.signed_state.to_bytes());
        bytes.extend_from_slice(&self.required_by_block.to_le_bytes());
        bytes.extend_from_slice(&self.current_block.to_le_bytes());
        bytes.push(self.required_phase as u8);
        bytes
    }

    pub fn hash(&self) -> [u8; 32] {
        *blake3::hash(&self.to_bytes()).as_bytes()
    }
}

/// evidence for invalid state transition
#[derive(Clone, Debug)]
pub struct InvalidTransitionEvidence {
    /// the previous valid state
    pub prev_state: SignedStateCompact,
    /// the invalid next state
    pub invalid_state: SignedStateCompact,
    /// the action that was applied
    pub action: Vec<u8>,
    /// expected state hash (from correct transition)
    pub expected_hash: [u8; 32],
}

impl InvalidTransitionEvidence {
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut bytes = Vec::new();
        bytes.extend_from_slice(&self.prev_state.to_bytes());
        bytes.extend_from_slice(&self.invalid_state.to_bytes());
        bytes.extend_from_slice(&(self.action.len() as u16).to_le_bytes());
        bytes.extend_from_slice(&self.action);
        bytes.extend_from_slice(&self.expected_hash);
        bytes
    }

    pub fn hash(&self) -> [u8; 32] {
        *blake3::hash(&self.to_bytes()).as_bytes()
    }
}

/// evidence for insufficient balance
#[derive(Clone, Debug)]
pub struct InsufficientBalanceEvidence {
    /// player who bet too much
    pub player: PublicKey,
    /// their balance at the time
    pub balance: Amount,
    /// the bet they tried to make
    pub attempted_bet: Amount,
    /// signed state showing their balance
    pub state: SignedStateCompact,
}

impl InsufficientBalanceEvidence {
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut bytes = Vec::new();
        bytes.extend_from_slice(&self.player.0);
        bytes.extend_from_slice(&self.balance.0.to_le_bytes());
        bytes.extend_from_slice(&self.attempted_bet.0.to_le_bytes());
        bytes.extend_from_slice(&self.state.to_bytes());
        bytes
    }

    pub fn hash(&self) -> [u8; 32] {
        *blake3::hash(&self.to_bytes()).as_bytes()
    }
}

/// compact signed state for on-chain verification
#[derive(Clone, Debug)]
pub struct SignedStateCompact {
    /// channel id
    pub channel_id: [u8; 32],
    /// state nonce
    pub nonce: u64,
    /// state hash
    pub state_hash: [u8; 32],
    /// app data hash (poker state)
    pub app_data_hash: [u8; 32],
    /// signatures (pubkey, sig) pairs - sig is 32 bytes
    pub signatures: Vec<([u8; 32], [u8; 32])>,
}

impl SignedStateCompact {
    /// create from full signed state
    pub fn from_signed_state(state: &SignedState) -> Self {
        let state_hash = state.state.hash();
        let app_data_hash = *blake3::hash(&state.state.app_data).as_bytes();

        let signatures: Vec<_> = state.signatures.iter()
            .map(|(pk, sig)| (pk.0, sig.0))
            .collect();

        Self {
            channel_id: state.state.channel_id.0,
            nonce: state.state.nonce,
            state_hash,
            app_data_hash,
            signatures,
        }
    }

    pub fn to_bytes(&self) -> Vec<u8> {
        let mut bytes = Vec::new();
        bytes.extend_from_slice(&self.channel_id);
        bytes.extend_from_slice(&self.nonce.to_le_bytes());
        bytes.extend_from_slice(&self.state_hash);
        bytes.extend_from_slice(&self.app_data_hash);
        bytes.push(self.signatures.len() as u8);
        for (pk, sig) in &self.signatures {
            bytes.extend_from_slice(pk);
            bytes.extend_from_slice(sig);
        }
        bytes
    }

    pub fn from_bytes(bytes: &[u8]) -> Option<Self> {
        if bytes.len() < 105 {
            return None;
        }
        let mut channel_id = [0u8; 32];
        channel_id.copy_from_slice(&bytes[0..32]);
        let nonce = u64::from_le_bytes(bytes[32..40].try_into().ok()?);
        let mut state_hash = [0u8; 32];
        state_hash.copy_from_slice(&bytes[40..72]);
        let mut app_data_hash = [0u8; 32];
        app_data_hash.copy_from_slice(&bytes[72..104]);

        let sig_count = bytes[104] as usize;
        let mut offset = 105;
        let mut signatures = Vec::with_capacity(sig_count);
        for _ in 0..sig_count {
            // 32 bytes pubkey + 32 bytes signature = 64 bytes per entry
            if offset + 64 > bytes.len() {
                return None;
            }
            let mut pk = [0u8; 32];
            pk.copy_from_slice(&bytes[offset..offset + 32]);
            offset += 32;
            let mut sig = [0u8; 32];
            sig.copy_from_slice(&bytes[offset..offset + 32]);
            offset += 32;
            signatures.push((pk, sig));
        }

        Some(Self {
            channel_id,
            nonce,
            state_hash,
            app_data_hash,
            signatures,
        })
    }
}

// === dispute builder ===

/// builds dispute proofs
pub struct DisputeBuilder {
    channel_id: ChannelId,
    hand_number: u64,
}

impl DisputeBuilder {
    pub fn new(channel_id: ChannelId, hand_number: u64) -> Self {
        Self { channel_id, hand_number }
    }

    /// build invalid shuffle dispute
    pub fn invalid_shuffle(&self, evidence: ShuffleEvidence) -> DisputeProof {
        let evidence_bytes = evidence.to_bytes();
        DisputeProof {
            dispute_type: DisputeType::InvalidShuffle,
            channel_id: self.channel_id,
            hand_number: self.hand_number,
            accused: evidence.shuffler,
            evidence: evidence_bytes,
            evidence_hash: evidence.hash(),
        }
    }

    /// build invalid reveal dispute
    pub fn invalid_reveal(&self, evidence: RevealEvidence) -> DisputeProof {
        let evidence_bytes = evidence.to_bytes();
        DisputeProof {
            dispute_type: DisputeType::InvalidReveal,
            channel_id: self.channel_id,
            hand_number: self.hand_number,
            accused: evidence.revealer,
            evidence: evidence_bytes,
            evidence_hash: evidence.hash(),
        }
    }

    /// build duplicate card dispute
    pub fn duplicate_card(&self, evidence: DuplicateCardEvidence, accused: PublicKey) -> DisputeProof {
        let evidence_bytes = evidence.to_bytes();
        DisputeProof {
            dispute_type: DisputeType::DuplicateCard,
            channel_id: self.channel_id,
            hand_number: self.hand_number,
            accused,
            evidence: evidence_bytes,
            evidence_hash: evidence.hash(),
        }
    }

    /// build timeout dispute
    pub fn timeout(&self, evidence: TimeoutEvidence) -> DisputeProof {
        let evidence_bytes = evidence.to_bytes();
        DisputeProof {
            dispute_type: DisputeType::Timeout,
            channel_id: self.channel_id,
            hand_number: self.hand_number,
            accused: evidence.player,
            evidence: evidence_bytes,
            evidence_hash: evidence.hash(),
        }
    }

    /// build invalid transition dispute
    pub fn invalid_transition(&self, evidence: InvalidTransitionEvidence, accused: PublicKey) -> DisputeProof {
        let evidence_bytes = evidence.to_bytes();
        DisputeProof {
            dispute_type: DisputeType::InvalidTransition,
            channel_id: self.channel_id,
            hand_number: self.hand_number,
            accused,
            evidence: evidence_bytes,
            evidence_hash: evidence.hash(),
        }
    }

    /// build insufficient balance dispute
    pub fn insufficient_balance(&self, evidence: InsufficientBalanceEvidence) -> DisputeProof {
        let evidence_bytes = evidence.to_bytes();
        DisputeProof {
            dispute_type: DisputeType::InsufficientBalance,
            channel_id: self.channel_id,
            hand_number: self.hand_number,
            accused: evidence.player,
            evidence: evidence_bytes,
            evidence_hash: evidence.hash(),
        }
    }
}

// === dispute resolution ===

/// result of dispute verification
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DisputeVerdict {
    /// dispute is valid, accused loses
    AccusedGuilty,
    /// dispute is invalid, accuser loses (false accusation)
    AccuserGuilty,
    /// cannot determine (needs more evidence)
    Inconclusive,
}

/// settlement after dispute
#[derive(Clone, Debug)]
pub struct DisputeSettlement {
    /// the verdict
    pub verdict: DisputeVerdict,
    /// channel id
    pub channel_id: ChannelId,
    /// who loses funds
    pub loser: PublicKey,
    /// who gains funds
    pub winners: Vec<PublicKey>,
    /// amount to redistribute
    pub amount: Amount,
}

/// verifies dispute proofs (simplified, on-chain verifier would be more rigorous)
pub fn verify_dispute(proof: &DisputeProof) -> DisputeVerdict {
    // basic sanity checks
    if proof.evidence.is_empty() {
        return DisputeVerdict::Inconclusive;
    }

    // verify evidence hash matches
    let computed_hash = *blake3::hash(&proof.evidence).as_bytes();
    if computed_hash != proof.evidence_hash {
        return DisputeVerdict::AccuserGuilty; // false accusation
    }

    match proof.dispute_type {
        DisputeType::InvalidShuffle => {
            // would verify shuffle proof against input/output commitments
            // if proof doesn't verify, accused is guilty
            if let Some(_evidence) = ShuffleEvidence::from_bytes(&proof.evidence) {
                // actual verification would happen here using zk-shuffle-verifier
                DisputeVerdict::AccusedGuilty
            } else {
                DisputeVerdict::Inconclusive
            }
        }
        DisputeType::Timeout => {
            // verify signed state shows it was their turn
            // and current_block > required_by_block
            DisputeVerdict::AccusedGuilty
        }
        DisputeType::InvalidTransition => {
            // recompute expected state from prev_state + action
            // compare with invalid_state
            DisputeVerdict::AccusedGuilty
        }
        _ => {
            // other types need crypto verification
            DisputeVerdict::Inconclusive
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_dispute_proof_roundtrip() {
        let channel_id = ChannelId([1u8; 32]);
        let builder = DisputeBuilder::new(channel_id, 5);

        let evidence = ShuffleEvidence {
            shuffler: PublicKey([2u8; 32]),
            input_commitment: [3u8; 32],
            output_commitment: [4u8; 32],
            proof_hash: [5u8; 32],
            aggregate_pk: [6u8; 32],
        };

        let proof = builder.invalid_shuffle(evidence);

        assert_eq!(proof.dispute_type.to_u8(), 0);
        assert_eq!(proof.hand_number, 5);
        assert_eq!(proof.accused.0, [2u8; 32]);

        // verify commitment is deterministic
        let commitment1 = proof.commitment();
        let commitment2 = proof.commitment();
        assert_eq!(commitment1, commitment2);
    }

    #[test]
    fn test_signed_state_compact() {
        use crate::channel::{ChannelState, Participant};

        let pk1 = PublicKey([1u8; 32]);
        let pk2 = PublicKey([2u8; 32]);

        let state = ChannelState {
            channel_id: ChannelId([0u8; 32]),
            nonce: 42,
            participants: vec![
                Participant { public_key: pk1, balance: 1000u64.into() },
                Participant { public_key: pk2, balance: 500u64.into() },
            ],
            prev_state_hash: [0u8; 32],
            app_data: vec![1, 2, 3],
        };

        let signed = SignedState {
            state,
            signatures: vec![
                (pk1, crate::keys::Signature([0xAA; 32])),
            ],
        };

        let compact = SignedStateCompact::from_signed_state(&signed);
        let bytes = compact.to_bytes();
        let recovered = SignedStateCompact::from_bytes(&bytes).unwrap();

        assert_eq!(compact.nonce, recovered.nonce);
        assert_eq!(compact.state_hash, recovered.state_hash);
        assert_eq!(compact.signatures.len(), recovered.signatures.len());
    }

    #[test]
    fn test_timeout_evidence() {
        let evidence = TimeoutEvidence {
            player: PublicKey([1u8; 32]),
            signed_state: SignedStateCompact {
                channel_id: [0u8; 32],
                nonce: 10,
                state_hash: [1u8; 32],
                app_data_hash: [2u8; 32],
                signatures: vec![([3u8; 32], [4u8; 32])],
            },
            required_by_block: 100,
            current_block: 150,
            required_phase: GamePhase::PreFlop,
        };

        let hash1 = evidence.hash();
        let hash2 = evidence.hash();
        assert_eq!(hash1, hash2);

        let bytes = evidence.to_bytes();
        assert!(!bytes.is_empty());
    }
}
