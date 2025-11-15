//! Simple stake-weighted BFT consensus
//!
//! ## How it works:
//!
//! 1. **Batch proposal**: Any participant can propose a batch
//! 2. **Verification**: All stakers verify the batch execution proof
//! 3. **Signing**: Stakers sign if batch is correct
//! 4. **Finalization**: Batch finalizes when 2/3+ of stake signs
//!
//! ## No traditional validators:
//!
//! - Anyone with MIN_STAKE_ZT can participate
//! - No leader election, no rotation
//! - Pure Byzantine agreement on batch validity

use crate::{
    zswap::{SwapIntent, DexState, TradingPair, MIN_STAKE_ZT},
    consensus::BlockNumber,
};
use ed25519_dalek::{Signature, Signer, SigningKey, VerifyingKey, Verifier};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Batch proposal with stake-weighted signatures
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BatchProposal {
    /// Batch number (= block number)
    pub batch_id: BlockNumber,

    /// Trading pair for this batch
    pub pair: TradingPair,

    /// All swap intents in batch
    pub swaps: Vec<SwapIntent>,

    /// Total burn amounts by direction
    pub total_burn_1_to_2: u64,
    pub total_burn_2_to_1: u64,

    /// Computed clearing price
    pub clearing_price: f64,

    /// PolkaVM execution proof
    pub pvm_proof: Vec<u8>,

    /// Stake-weighted signatures
    pub signatures: Vec<StakeSignature>,
}

/// Signature with stake weight
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StakeSignature {
    /// Validator's public key
    pub validator_pubkey: [u8; 32],
    /// Ed25519 signature
    pub signature: [u8; 64],
    /// Amount of ZT staked by this validator
    pub stake_amount: u64,
}

impl BatchProposal {
    /// Create new batch proposal (unsigned)
    pub fn new(
        batch_id: BlockNumber,
        pair: TradingPair,
        swaps: Vec<SwapIntent>,
        total_burn_1_to_2: u64,
        total_burn_2_to_1: u64,
        clearing_price: f64,
        pvm_proof: Vec<u8>,
    ) -> Self {
        Self {
            batch_id,
            pair,
            swaps,
            total_burn_1_to_2,
            total_burn_2_to_1,
            clearing_price,
            pvm_proof,
            signatures: vec![],
        }
    }

    /// Get bytes to sign (deterministic serialization)
    pub fn signing_bytes(&self) -> Vec<u8> {
        let mut bytes = Vec::new();

        // Batch ID
        bytes.extend_from_slice(&self.batch_id.to_le_bytes());

        // Trading pair
        bytes.extend_from_slice(&self.pair.asset_1);
        bytes.extend_from_slice(&self.pair.asset_2);

        // Totals
        bytes.extend_from_slice(&self.total_burn_1_to_2.to_le_bytes());
        bytes.extend_from_slice(&self.total_burn_2_to_1.to_le_bytes());

        // Clearing price (as u64 bits to avoid float issues)
        bytes.extend_from_slice(&self.clearing_price.to_bits().to_le_bytes());

        // PVM proof hash (don't include full proof in signature)
        use sha2::{Sha256, Digest};
        let proof_hash = Sha256::digest(&self.pvm_proof);
        bytes.extend_from_slice(&proof_hash);

        bytes
    }

    /// Sign batch as a validator
    pub fn sign(
        &mut self,
        signing_key: &SigningKey,
        stake_amount: u64,
    ) -> Result<(), BatchError> {
        if stake_amount < MIN_STAKE_ZT {
            return Err(BatchError::InsufficientStake);
        }

        let message = self.signing_bytes();
        let signature = signing_key.sign(&message);

        self.signatures.push(StakeSignature {
            validator_pubkey: signing_key.verifying_key().to_bytes(),
            signature: signature.to_bytes(),
            stake_amount,
        });

        Ok(())
    }

    /// Verify all signatures
    pub fn verify_signatures(&self, dex_state: &DexState) -> Result<(), BatchError> {
        let message = self.signing_bytes();

        for sig in &self.signatures {
            // Verify stake amount matches on-chain record
            let stake = dex_state
                .get_stake(&sig.validator_pubkey)
                .ok_or(BatchError::UnknownValidator)?;

            if stake.total_stake() != sig.stake_amount {
                return Err(BatchError::InvalidStake);
            }

            // Verify signature
            let verifying_key = VerifyingKey::from_bytes(&sig.validator_pubkey)
                .map_err(|_| BatchError::InvalidPublicKey)?;
            let signature = Signature::from_bytes(&sig.signature);

            verifying_key
                .verify(&message, &signature)
                .map_err(|_| BatchError::InvalidSignature)?;
        }

        Ok(())
    }

    /// Check if batch has reached 2/3+ stake threshold
    pub fn is_finalized(&self, total_stake: u64) -> bool {
        let signed_stake: u64 = self.signatures.iter().map(|s| s.stake_amount).sum();

        // 2/3 + 1 threshold (BFT)
        signed_stake * 3 >= total_stake * 2
    }

    /// Get current stake support %
    pub fn stake_support(&self, total_stake: u64) -> f64 {
        if total_stake == 0 {
            return 0.0;
        }

        let signed_stake: u64 = self.signatures.iter().map(|s| s.stake_amount).sum();
        (signed_stake as f64 / total_stake as f64) * 100.0
    }
}

/// Simple BFT consensus engine
pub struct BftConsensus {
    /// Current DEX state (includes stake registry)
    dex_state: DexState,

    /// Pending batch proposals by block number
    pending_batches: HashMap<BlockNumber, Vec<BatchProposal>>,

    /// Finalized batches
    finalized_batches: Vec<BatchProposal>,
}

impl BftConsensus {
    pub fn new(dex_state: DexState) -> Self {
        Self {
            dex_state,
            pending_batches: HashMap::new(),
            finalized_batches: Vec::new(),
        }
    }

    /// Submit batch proposal
    pub fn propose_batch(&mut self, batch: BatchProposal) -> Result<(), BatchError> {
        // Verify PVM proof
        // TODO: Actually verify proof
        if batch.pvm_proof.len() != 101_000 {
            return Err(BatchError::InvalidProof);
        }

        // Verify signatures
        batch.verify_signatures(&self.dex_state)?;

        // Add to pending
        self.pending_batches
            .entry(batch.batch_id)
            .or_insert_with(Vec::new)
            .push(batch);

        Ok(())
    }

    /// Sign a batch (as validator)
    pub fn sign_batch(
        &mut self,
        batch_id: BlockNumber,
        pair: TradingPair,
        signing_key: &SigningKey,
        stake_amount: u64,
    ) -> Result<(), BatchError> {
        // Find batch
        let batches = self
            .pending_batches
            .get_mut(&batch_id)
            .ok_or(BatchError::BatchNotFound)?;

        let batch = batches
            .iter_mut()
            .find(|b| b.pair == pair)
            .ok_or(BatchError::BatchNotFound)?;

        // Sign it
        batch.sign(signing_key, stake_amount)?;

        // Check if finalized
        let total_stake = self.dex_state.total_stake();
        if batch.is_finalized(total_stake) {
            // Move to finalized
            let finalized = batch.clone();
            self.finalized_batches.push(finalized);
        }

        Ok(())
    }

    /// Get finalized batches
    pub fn finalized(&self) -> &[BatchProposal] {
        &self.finalized_batches
    }

    /// Get pending batches
    pub fn pending(&self, block: BlockNumber) -> Option<&[BatchProposal]> {
        self.pending_batches.get(&block).map(|v| v.as_slice())
    }

    /// Get total staked ZT
    pub fn total_stake(&self) -> u64 {
        self.dex_state.total_stake()
    }
}

/// Evidence of validator misbehavior
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum SlashingEvidence {
    /// Validator signed invalid batch proof
    InvalidBatchProof {
        batch_id: BlockNumber,
        validator_pubkey: [u8; 32],
        invalid_signature: [u8; 64],
    },

    /// Validator double-signed (two signatures for same batch)
    DoubleSigning {
        batch_id: BlockNumber,
        validator_pubkey: [u8; 32],
        signature_1: [u8; 64],
        signature_2: [u8; 64],
    },

    /// Validator offline for too long
    LivenessFailure {
        validator_pubkey: [u8; 32],
        missed_blocks: u64,
    },
}

impl SlashingEvidence {
    /// Get slashing penalty in basis points
    pub fn penalty_bps(&self) -> u64 {
        match self {
            SlashingEvidence::InvalidBatchProof { .. } => 1000, // 10%
            SlashingEvidence::DoubleSigning { .. } => 2000,     // 20%
            SlashingEvidence::LivenessFailure { .. } => 100,    // 1%
        }
    }

    /// Get validator being slashed
    pub fn validator(&self) -> [u8; 32] {
        match self {
            SlashingEvidence::InvalidBatchProof { validator_pubkey, .. } => *validator_pubkey,
            SlashingEvidence::DoubleSigning { validator_pubkey, .. } => *validator_pubkey,
            SlashingEvidence::LivenessFailure { validator_pubkey, .. } => *validator_pubkey,
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum BatchError {
    #[error("Insufficient stake (need {MIN_STAKE_ZT} ZT)")]
    InsufficientStake,

    #[error("Unknown validator")]
    UnknownValidator,

    #[error("Invalid stake amount")]
    InvalidStake,

    #[error("Invalid public key")]
    InvalidPublicKey,

    #[error("Invalid signature")]
    InvalidSignature,

    #[error("Invalid proof")]
    InvalidProof,

    #[error("Batch not found")]
    BatchNotFound,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::zswap::{TradingPair, StakePosition};

    #[test]
    fn test_batch_signing_and_finalization() {
        let mut dex_state = DexState::new();

        // Create 3 validators with different stakes
        let mut validators = vec![];
        for i in 0..3 {
            let signing_key = SigningKey::generate(&mut rand::thread_rng());
            let stake_amount = (i + 1) as u64 * 1000 * MIN_STAKE_ZT; // 1000, 2000, 3000 ZT

            dex_state.add_stake(StakePosition {
                validator_pubkey: signing_key.verifying_key().to_bytes(),
                stake_amount,
                staked_at_block: 0,
                delegators: vec![],
            });

            validators.push((signing_key, stake_amount));
        }

        // Total stake = 6000 units, need 4000 for 2/3

        let pair = TradingPair::new([1; 32], [2; 32]);
        let mut batch = BatchProposal::new(
            1,
            pair,
            vec![],
            100,
            0,
            2.0,
            vec![0; 101_000],
        );

        let total_stake = dex_state.total_stake();

        // Validator 0 signs (1000 stake) - not finalized yet
        batch.sign(&validators[0].0, validators[0].1).unwrap();
        assert!(!batch.is_finalized(total_stake));
        assert!(batch.stake_support(total_stake) < 67.0);

        // Validator 2 signs (3000 stake) - now 4000/6000 = finalized!
        batch.sign(&validators[2].0, validators[2].1).unwrap();
        assert!(batch.is_finalized(total_stake));
        assert!(batch.stake_support(total_stake) >= 67.0);
    }
}
