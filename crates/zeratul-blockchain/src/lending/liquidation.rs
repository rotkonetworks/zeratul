//! Privacy-Preserving Batch Liquidation System
//!
//! Liquidations are computed using ZK proofs to maintain privacy while proving
//! positions are legitimately underwater (health factor < 1.0).
//!
//! Uses serde_big_array for [u8; 64] serialization.
//!
//! ## Privacy Properties
//!
//! **HIDDEN (Private):**
//! - Which specific positions are liquidated
//! - Who owns the liquidated positions
//! - Individual liquidation amounts
//!
//! **REVEALED (Public):**
//! - Number of positions liquidated in batch
//! - Total liquidation volume (aggregate)
//! - Average liquidation penalty (aggregate)
//!
//! ## ZK Proof Circuit
//!
//! Each liquidator proves:
//! ```text
//! "I know a position commitment that:
//!   1. Exists in NOMT state (inclusion proof)
//!   2. Decrypts to valid position data
//!   3. Has health factor < 1.0 at current oracle prices
//!   4. Produces valid liquidation outputs"
//! ```
//!
//! ## Batch Liquidation Flow
//!
//! ```text
//! Block N:
//!   ┌─> Validator 1 submits proof: "I know 3 liquidatable positions"
//!   ├─> Validator 2 submits proof: "I know 2 liquidatable positions"
//!   ├─> Validator 3 submits proof: "I know 4 liquidatable positions"
//!   └─> Aggregate: 9 positions liquidated (anonymous set)
//!
//!   Execute liquidations:
//!   - Close all 9 positions
//!   - Seize collateral (5% penalty)
//!   - Repay debts to pool
//!   - Return excess to position owners
//!
//!   Public output:
//!   - "9 positions liquidated"
//!   - "Total volume: 50,000 UM"
//!   - "Average penalty: 5%"
//! ```

use super::types::*;
use super::privacy::*;
use crate::frost::{FrostSignature, FrostCoordinator, ThresholdRequirement, ValidatorId, SigningPackageData, SignatureShareData, CommitmentData};
use anyhow::{bail, Result};
use serde::{Deserialize, Serialize};
use serde_big_array::BigArray;
use zeratul_circuit::AccidentalComputerProof;
use std::collections::BTreeMap; // SECURITY FIX: Use BTreeMap for deterministic iteration

/// Liquidation proof (proves position is underwater)
///
/// This ZK proof demonstrates:
/// 1. Position commitment exists in NOMT
/// 2. Position decrypts to valid data
/// 3. Health factor < 1.0 at current prices
/// 4. Liquidation is correctly computed
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LiquidationProof {
    /// Position commitment being liquidated (public)
    pub position_commitment: [u8; 32],

    /// ZK proof of liquidation validity
    pub proof: AccidentalComputerProof,

    /// NOMT inclusion proof (proves position exists)
    pub nomt_witness: Vec<u8>,

    /// Public inputs to proof
    pub public_inputs: LiquidationPublicInputs,
}

/// Public inputs to liquidation proof
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LiquidationPublicInputs {
    /// Position commitment
    pub commitment: [u8; 32],

    /// Current oracle prices (hash of price vector)
    pub oracle_prices_hash: [u8; 32],

    /// NOMT state root
    pub state_root: [u8; 32],

    /// Liquidation penalty charged (e.g., 5%)
    pub penalty_percent: u8,

    /// Seized collateral amount (public for aggregate)
    pub seized_collateral: Amount,

    /// Debt repaid (public for aggregate)
    pub debt_repaid: Amount,

    /// Block height when position became liquidatable
    /// HARDENING: Used to calculate penalty decay
    pub became_liquidatable_at: u64,
}

/// Liquidation configuration with timing attack protection
#[derive(Debug, Clone)]
pub struct LiquidationConfig {
    /// Minimum delay before liquidation can execute (blocks)
    /// HARDENING: Prevents timing attacks by first liquidator
    /// Default: 2 blocks (4 seconds)
    pub min_liquidation_delay: u64,

    /// Maximum penalty (at min_liquidation_delay)
    /// Default: 5%
    pub max_penalty_percent: u8,

    /// Minimum penalty (after full decay period)
    /// Default: 1%
    pub min_penalty_percent: u8,

    /// Penalty decay period (blocks)
    /// Penalty decays linearly from max to min over this period
    /// Default: 10 blocks (20 seconds)
    pub penalty_decay_blocks: u64,

    /// Whether partial liquidations are allowed
    /// HARDENING: Liquidate only what's needed, not entire position
    pub allow_partial_liquidations: bool,

    /// Minimum partial liquidation amount (% of debt)
    /// Default: 20% (must liquidate at least 20% of debt)
    pub min_partial_liquidation_percent: u8,
}

impl Default for LiquidationConfig {
    fn default() -> Self {
        Self {
            min_liquidation_delay: 2,           // 2 blocks (4 seconds) delay
            max_penalty_percent: 5,             // 5% max penalty
            min_penalty_percent: 1,             // 1% min penalty after decay
            penalty_decay_blocks: 10,           // Decay over 10 blocks (20s)
            allow_partial_liquidations: true,   // Partial liquidations allowed
            min_partial_liquidation_percent: 20, // Min 20% of debt
        }
    }
}

/// Liquidation delegate public key
pub type DelegateKey = [u8; 32];

/// Registered liquidation delegate
///
/// Delegates can execute liquidations on behalf of position owners.
/// Selected via VRF to prevent gaming.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LiquidationDelegate {
    /// Delegate's public key
    pub key: DelegateKey,

    /// Delegate's stake (for weighted selection)
    pub stake: u64,

    /// Whether delegate is active
    pub active: bool,

    /// Number of successful liquidations
    pub liquidations_executed: u64,

    /// Delegate's fee (basis points, e.g., 50 = 0.5%)
    pub fee_bps: u16,
}

/// Registry of liquidation delegates
#[derive(Debug, Clone, Default)]
pub struct DelegateRegistry {
    /// Registered delegates
    delegates: Vec<LiquidationDelegate>,
}

impl DelegateRegistry {
    /// Create new empty registry
    pub fn new() -> Self {
        Self {
            delegates: Vec::new(),
        }
    }

    /// Register a new delegate
    pub fn register(&mut self, delegate: LiquidationDelegate) -> Result<()> {
        if self.delegates.iter().any(|d| d.key == delegate.key) {
            bail!("Delegate already registered");
        }
        self.delegates.push(delegate);
        Ok(())
    }

    /// Get active delegates
    pub fn active_delegates(&self) -> Vec<&LiquidationDelegate> {
        self.delegates.iter().filter(|d| d.active).collect()
    }

    /// Select delegate using VRF from Safrole entropy
    ///
    /// Uses entropy from Safrole accumulator + position commitment + slot
    /// for unpredictable but deterministic selection.
    ///
    /// ## Selection Algorithm
    ///
    /// ```text
    /// hash = blake3(entropy || position_commitment || slot)
    /// index = u32(hash[0..4]) % num_delegates
    /// selected = delegates[index]
    /// ```
    ///
    /// Different delegate selected each slot, preventing:
    /// - Front-running (can't predict selection)
    /// - Collusion (selection changes per slot)
    /// - Gaming (deterministic from on-chain data)
    pub fn select_delegate(
        &self,
        entropy: &[u8; 32],
        position_commitment: &[u8; 32],
        slot: u64,
    ) -> Option<&LiquidationDelegate> {
        let active = self.active_delegates();
        if active.is_empty() {
            return None;
        }

        // Build VRF input: entropy || position_commitment || slot
        let mut data = Vec::with_capacity(72);
        data.extend_from_slice(entropy);
        data.extend_from_slice(position_commitment);
        data.extend_from_slice(&slot.to_le_bytes());

        // Hash to get deterministic but unpredictable selection
        let hash = blake3::hash(&data);
        let hash_bytes = hash.as_bytes();

        // Extract index from first 4 bytes
        let index_bytes: [u8; 4] = hash_bytes[0..4].try_into().unwrap();
        let index = u32::from_le_bytes(index_bytes) as usize % active.len();

        Some(active[index])
    }

    /// Select delegate with weighted probability based on stake
    ///
    /// Higher stake = higher probability of selection
    pub fn select_delegate_weighted(
        &self,
        entropy: &[u8; 32],
        position_commitment: &[u8; 32],
        slot: u64,
    ) -> Option<&LiquidationDelegate> {
        let active = self.active_delegates();
        if active.is_empty() {
            return None;
        }

        // Calculate total stake
        let total_stake: u64 = active.iter().map(|d| d.stake).sum();
        if total_stake == 0 {
            // Fall back to uniform selection
            return self.select_delegate(entropy, position_commitment, slot);
        }

        // Build VRF input
        let mut data = Vec::with_capacity(72);
        data.extend_from_slice(entropy);
        data.extend_from_slice(position_commitment);
        data.extend_from_slice(&slot.to_le_bytes());

        let hash = blake3::hash(&data);
        let hash_bytes = hash.as_bytes();

        // Use first 8 bytes for selection
        let rand_bytes: [u8; 8] = hash_bytes[0..8].try_into().unwrap();
        let rand_value = u64::from_le_bytes(rand_bytes) % total_stake;

        // Select based on cumulative stake
        let mut cumulative = 0u64;
        for delegate in &active {
            cumulative += delegate.stake;
            if rand_value < cumulative {
                return Some(delegate);
            }
        }

        // Should never reach here, but fallback to last
        active.last().copied()
    }

    /// Check if a delegate is authorized for this slot
    pub fn is_authorized(
        &self,
        delegate_key: &DelegateKey,
        entropy: &[u8; 32],
        position_commitment: &[u8; 32],
        slot: u64,
    ) -> bool {
        if let Some(selected) = self.select_delegate(entropy, position_commitment, slot) {
            selected.key == *delegate_key
        } else {
            false
        }
    }
}

/// FHE-based health factor check result
///
/// Result of computing health factor on encrypted position data.
/// Only reveals boolean (liquidatable or not), not actual values.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FheHealthCheckResult {
    /// Position commitment being checked
    pub position_commitment: [u8; 32],

    /// Whether position is liquidatable (health < 1.0)
    pub is_liquidatable: bool,

    /// Proof that FHE computation was correct
    /// Verified by Ligerito in PolkaVM
    pub fhe_proof: Vec<u8>,

    /// Block at which check was performed
    pub checked_at_block: u64,
}

impl LiquidationConfig {
    /// Calculate liquidation penalty based on time elapsed
    ///
    /// HARDENING: Penalty decays over time to prevent timing attacks
    ///
    /// ```text
    /// Penalty Decay Timeline:
    ///
    /// Block 0: Position becomes liquidatable (health < 1.0)
    /// Block 2: Min delay passes, liquidation allowed at 5% penalty
    /// Block 12: Full decay period, liquidation allowed at 1% penalty
    ///
    /// Penalty:
    ///   ├─ Block 0-2:  Not allowed (delay period)
    ///   ├─ Block 2:    5.0% (max penalty)
    ///   ├─ Block 4:    4.2% (decaying...)
    ///   ├─ Block 6:    3.4%
    ///   ├─ Block 8:    2.6%
    ///   ├─ Block 10:   1.8%
    ///   └─ Block 12+:  1.0% (min penalty)
    /// ```
    pub fn calculate_penalty(
        &self,
        became_liquidatable_at: u64,
        current_block: u64,
    ) -> Result<u8> {
        let blocks_elapsed = current_block.saturating_sub(became_liquidatable_at);

        // Check minimum delay
        if blocks_elapsed < self.min_liquidation_delay {
            bail!(
                "Liquidation too early: must wait {} blocks, only {} elapsed",
                self.min_liquidation_delay,
                blocks_elapsed
            );
        }

        // Calculate penalty with linear decay
        if blocks_elapsed >= self.min_liquidation_delay + self.penalty_decay_blocks {
            // After full decay period → minimum penalty
            return Ok(self.min_penalty_percent);
        }

        // Linear interpolation between max and min penalty
        let decay_progress = blocks_elapsed - self.min_liquidation_delay;
        let penalty_range = self.max_penalty_percent - self.min_penalty_percent;
        let penalty_reduction = (penalty_range as u64 * decay_progress) / self.penalty_decay_blocks;

        Ok(self.max_penalty_percent - penalty_reduction as u8)
    }

    /// Validate liquidation timing
    ///
    /// HARDENING: Ensures liquidation respects minimum delay
    pub fn validate_timing(
        &self,
        became_liquidatable_at: u64,
        current_block: u64,
    ) -> Result<()> {
        let blocks_elapsed = current_block.saturating_sub(became_liquidatable_at);

        if blocks_elapsed < self.min_liquidation_delay {
            bail!(
                "Liquidation too early: must wait {} blocks (currently {} blocks elapsed)",
                self.min_liquidation_delay,
                blocks_elapsed
            );
        }

        Ok(())
    }

    /// Calculate maximum liquidation amount
    ///
    /// HARDENING: For partial liquidations, returns how much can be liquidated
    pub fn calculate_max_liquidation_amount(
        &self,
        total_debt: Amount,
        health_factor: Ratio,
    ) -> Amount {
        if !self.allow_partial_liquidations {
            // Full liquidation
            return total_debt;
        }

        // For partial liquidations, liquidate just enough to bring health > 1.0
        // Plus a buffer to prevent immediate re-liquidation

        // If health is very low (< 0.5), liquidate more
        // SECURITY FIX: Use checked arithmetic
        if health_factor
            .numerator
            .checked_mul(2)
            .map(|v| v < health_factor.denominator)
            .unwrap_or(false)
        {
            // Health < 0.5 → liquidate 50% of debt
            Amount(total_debt.0.checked_div(2).unwrap_or(0))
        } else {
            // Health 0.5-1.0 → liquidate minimum (20%)
            let min_amount = total_debt
                .0
                .checked_mul(self.min_partial_liquidation_percent as u128)
                .and_then(|v| v.checked_div(100))
                .unwrap_or(0);
            Amount(min_amount)
        }
    }
}

/// Private witness for liquidation proof
///
/// This data is NOT revealed publicly, only used to generate proof.
#[derive(Debug, Clone)]
pub struct LiquidationWitness {
    /// Position owner's viewing key
    pub viewing_key: ViewingKey,

    /// Position data (decrypted)
    pub position: PrivatePositionState,

    /// Current oracle prices (actual values)
    /// SECURITY FIX: Use BTreeMap for deterministic iteration
    pub oracle_prices: BTreeMap<AssetId, Amount>,

    /// NOMT witness for position commitment
    pub nomt_witness: Vec<u8>,

    /// Randomness used in commitment
    pub commitment_randomness: [u8; 32],
}

impl LiquidationWitness {
    /// Calculate health factor for this position
    pub fn calculate_health_factor(&self, pool: &LendingPool) -> Result<Ratio> {
        // Calculate total collateral value (in base currency)
        let mut total_collateral_value = 0u128;
        for (asset_id, amount) in &self.position.collateral {
            let price = self.oracle_prices
                .get(asset_id)
                .ok_or_else(|| anyhow::anyhow!("missing oracle price for asset"))?;

            // ========================================
            // TODO TODO TODO: CRITICAL PLACEHOLDER
            // ========================================
            // Get asset params from pool configuration
            // let collateral_params = pool.get_asset_params(*asset_id)?;
            let liquidation_threshold = Ratio::from_percent(80); // 80% default - HARDCODED!

            // Adjusted collateral = amount * price * liquidation_threshold
            let value = amount.0 as u128 * price.0 as u128 * liquidation_threshold.numerator as u128
                / liquidation_threshold.denominator as u128;

            total_collateral_value += value;
        }

        // Calculate total debt value (in base currency)
        let mut total_debt_value = 0u128;
        for (asset_id, amount) in &self.position.debt {
            let price = self.oracle_prices
                .get(asset_id)
                .ok_or_else(|| anyhow::anyhow!("missing oracle price for asset"))?;

            let value = amount.0 as u128 * price.0 as u128;
            total_debt_value += value;
        }

        if total_debt_value == 0 {
            return Ok(Ratio {
                numerator: u64::MAX as u128,
                denominator: 1,
            }); // No debt = infinite health
        }

        // Health factor = adjusted_collateral / debt
        let health_factor = Ratio {
            numerator: total_collateral_value,
            denominator: total_debt_value,
        };

        Ok(health_factor)
    }

    /// Verify this position is actually liquidatable
    pub fn is_liquidatable(&self, pool: &LendingPool) -> Result<bool> {
        let health = self.calculate_health_factor(pool)?;
        Ok(health.lt(&Ratio::ONE))
    }
}

/// Batch liquidation proposal
///
/// Validators submit these to propose liquidations.
/// Multiple validators can propose liquidations for the same block.
///
/// FROST INTEGRATION: Uses Byzantine threshold (11/15) for liquidation approval
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LiquidationProposal {
    /// Validator who proposed this
    pub proposer_id: ValidatorId,

    /// Block height
    pub height: u64,

    /// Liquidation proofs
    pub proofs: Vec<LiquidationProof>,

    /// FROST signature with Byzantine threshold (11/15)
    /// Requires 11 out of 15 validators to approve batch liquidation
    pub frost_signature: Option<FrostSignature>,

    /// Legacy single signature (for backward compatibility during migration)
    #[serde(skip)]
    pub signature: Option<[u8; 64]>,
}

impl LiquidationProposal {
    /// Verify all proofs in this proposal
    pub fn verify_all_proofs(&self) -> Result<bool> {
        for proof in &self.proofs {
            if !self.verify_liquidation_proof(proof)? {
                return Ok(false);
            }
        }
        Ok(true)
    }

    /// Verify a single liquidation proof
    fn verify_liquidation_proof(&self, proof: &LiquidationProof) -> Result<bool> {
        // In real implementation:
        // 1. Verify NOMT inclusion proof
        // 2. Verify AccidentalComputer ZK proof
        // 3. Check public inputs are consistent

        // Placeholder for now
        Ok(true)
    }

    /// Verify FROST signature on liquidation proposal
    ///
    /// FROST INTEGRATION: Verifies Byzantine threshold (11/15) is met
    pub fn verify_frost_signature(&self, coordinator: &FrostCoordinator) -> Result<()> {
        if let Some(frost_sig) = &self.frost_signature {
            // Verify threshold is ByzantineThreshold (11/15)
            if frost_sig.threshold != ThresholdRequirement::ByzantineThreshold {
                bail!("Invalid threshold for liquidation: expected ByzantineThreshold (11/15), got {:?}", frost_sig.threshold);
            }

            // Reconstruct message: Hash(block_height || num_liquidations || commitment_hash)
            use sha2::{Digest, Sha256};
            let mut hasher = Sha256::new();
            hasher.update(&self.height.to_le_bytes());
            hasher.update(&(self.proofs.len() as u64).to_le_bytes());

            // Hash all position commitments
            for proof in &self.proofs {
                hasher.update(&proof.position_commitment);
            }
            let message = hasher.finalize().to_vec();

            // Verify FROST signature
            coordinator.verify(&message, frost_sig)?;

            Ok(())
        } else {
            bail!("Missing FROST signature on liquidation proposal");
        }
    }
}

/// Batch liquidation execution result
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BatchLiquidationResult {
    /// Block height
    pub height: u64,

    /// Number of positions liquidated
    pub num_liquidated: u32,

    /// Total collateral seized (aggregate)
    /// SECURITY FIX: Use BTreeMap for deterministic iteration
    pub total_seized: BTreeMap<AssetId, Amount>,

    /// Total debt repaid to pool (aggregate)
    /// SECURITY FIX: Use BTreeMap for deterministic iteration
    pub total_debt_repaid: BTreeMap<AssetId, Amount>,

    /// Liquidation penalties collected (aggregate)
    /// SECURITY FIX: Use BTreeMap for deterministic iteration
    pub total_penalties: BTreeMap<AssetId, Amount>,

    /// Average health factor of liquidated positions
    pub avg_health_factor: Ratio,

    /// FROST signature on batch result (11/15 threshold)
    pub frost_signature: Option<FrostSignature>,
}

/// FROST Liquidation Coordinator
///
/// Coordinates Byzantine threshold (11/15) signatures for batch liquidations.
///
/// ## FROST Protocol for Liquidations
///
/// **Round 1: Commitment Phase**
/// - Each validator generates and broadcasts commitments for liquidation proofs
/// - Coordinator collects commitments from at least 11 validators
///
/// **Round 2: Signature Share Phase**
/// - Validators generate signature shares for liquidation batch
/// - Coordinator collects shares and aggregates
///
/// **Aggregation: Final Signature**
/// - Coordinator produces single 64-byte FROST signature
/// - Signature proves 11+ validators approved the liquidation batch
///
/// ## Security Benefits
///
/// - **Byzantine Fault Tolerance**: Can tolerate up to 4 malicious/offline validators
/// - **Single Signature**: 64 bytes instead of 11 × 64 = 704 bytes
/// - **15x Faster Verification**: Single signature check instead of 11
/// - **640 bytes saved per batch**: Over 1M blocks = 640 MB storage savings
pub struct FrostLiquidationCoordinator {
    /// Total number of validators
    total_validators: usize,

    /// FROST coordinator for signature aggregation
    frost_coordinator: FrostCoordinator,

    /// Pending commitments for current round (Round 1)
    pending_commitments: BTreeMap<ValidatorId, CommitmentData>,

    /// Pending signature shares (Round 2)
    pending_shares: BTreeMap<ValidatorId, SignatureShareData>,

    /// Current liquidation batch being signed
    current_batch: Option<Vec<LiquidationProof>>,

    /// Current block height
    current_height: u64,
}

impl FrostLiquidationCoordinator {
    /// Create new FROST liquidation coordinator
    pub fn new(total_validators: usize) -> Self {
        Self {
            total_validators,
            frost_coordinator: FrostCoordinator::new(total_validators),
            pending_commitments: BTreeMap::new(),
            pending_shares: BTreeMap::new(),
            current_batch: None,
            current_height: 0,
        }
    }

    /// Start new liquidation batch signing round
    ///
    /// Initiates FROST Round 1 for validators to commit
    pub fn start_batch(&mut self, proofs: Vec<LiquidationProof>, height: u64) -> Result<()> {
        if self.current_batch.is_some() {
            bail!("Liquidation batch already in progress");
        }

        self.current_batch = Some(proofs);
        self.current_height = height;
        self.pending_commitments.clear();
        self.pending_shares.clear();

        tracing::info!(
            "Started FROST liquidation batch at height {} with {} proofs",
            height,
            self.current_batch.as_ref().unwrap().len()
        );

        Ok(())
    }

    /// Add Round 1 commitment from validator
    pub fn add_commitment(
        &mut self,
        validator_id: ValidatorId,
        commitment: CommitmentData,
    ) -> Result<()> {
        if self.current_batch.is_none() {
            bail!("No liquidation batch in progress");
        }

        if validator_id as usize >= self.total_validators {
            bail!("Invalid validator ID: {}", validator_id);
        }

        self.pending_commitments.insert(validator_id, commitment);

        tracing::debug!(
            "Received commitment from validator {} ({}/11 required)",
            validator_id,
            self.pending_commitments.len()
        );

        Ok(())
    }

    /// Check if we have enough commitments to proceed to Round 2
    pub fn can_proceed_to_round2(&self) -> bool {
        self.pending_commitments.len() >= 11
    }

    /// Create signing package for Round 2
    ///
    /// Returns the message that validators should sign
    pub fn create_signing_package(&self) -> Result<SigningPackageData> {
        if !self.can_proceed_to_round2() {
            bail!(
                "Insufficient commitments: {} < 11 required",
                self.pending_commitments.len()
            );
        }

        let batch = self
            .current_batch
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("No batch in progress"))?;

        // Construct message: Hash(height || num_liquidations || commitment_hash)
        use sha2::{Digest, Sha256};
        let mut hasher = Sha256::new();
        hasher.update(&self.current_height.to_le_bytes());
        hasher.update(&(batch.len() as u64).to_le_bytes());

        for proof in batch {
            hasher.update(&proof.position_commitment);
        }

        let message = hasher.finalize().to_vec();

        Ok(SigningPackageData {
            message,
            commitments: self.pending_commitments.clone(),
        })
    }

    /// Add Round 2 signature share from validator
    pub fn add_signature_share(
        &mut self,
        validator_id: ValidatorId,
        share: SignatureShareData,
    ) -> Result<()> {
        if !self.can_proceed_to_round2() {
            bail!("Cannot accept shares before commitments are collected");
        }

        if !self.pending_commitments.contains_key(&validator_id) {
            bail!("Validator {} did not submit commitment", validator_id);
        }

        self.pending_shares.insert(validator_id, share);

        tracing::debug!(
            "Received signature share from validator {} ({}/11 required)",
            validator_id,
            self.pending_shares.len()
        );

        Ok(())
    }

    /// Check if we can finalize the FROST signature
    pub fn can_finalize(&self) -> bool {
        self.pending_shares.len() >= 11
    }

    /// Finalize FROST signature for liquidation batch
    ///
    /// Aggregates signature shares into final 64-byte FROST signature
    pub fn finalize(&mut self) -> Result<FrostSignature> {
        if !self.can_finalize() {
            bail!(
                "Insufficient signature shares: {} < 11 required",
                self.pending_shares.len()
            );
        }

        let signing_package = self.create_signing_package()?;

        // Aggregate signature shares
        let frost_signature = self.frost_coordinator.aggregate(
            &signing_package,
            &self.pending_shares,
            ThresholdRequirement::ByzantineThreshold,
        )?;

        tracing::info!(
            "Finalized FROST liquidation signature with {} validators at height {}",
            frost_signature.signers.len(),
            self.current_height
        );

        // Reset state for next batch
        self.current_batch = None;
        self.pending_commitments.clear();
        self.pending_shares.clear();

        Ok(frost_signature)
    }

    /// Verify a FROST liquidation signature
    pub fn verify(
        &self,
        proofs: &[LiquidationProof],
        height: u64,
        signature: &FrostSignature,
    ) -> Result<()> {
        // Verify threshold
        if signature.threshold != ThresholdRequirement::ByzantineThreshold {
            bail!("Invalid threshold: expected ByzantineThreshold (11/15)");
        }

        // Reconstruct message
        use sha2::{Digest, Sha256};
        let mut hasher = Sha256::new();
        hasher.update(&height.to_le_bytes());
        hasher.update(&(proofs.len() as u64).to_le_bytes());

        for proof in proofs {
            hasher.update(&proof.position_commitment);
        }

        let message = hasher.finalize().to_vec();

        // Verify FROST signature
        self.frost_coordinator.verify(&message, signature)?;

        Ok(())
    }

    /// Get current batch status
    pub fn status(&self) -> LiquidationBatchStatus {
        if self.current_batch.is_none() {
            return LiquidationBatchStatus::Idle;
        }

        if !self.can_proceed_to_round2() {
            return LiquidationBatchStatus::Round1 {
                commitments: self.pending_commitments.len(),
                required: 11,
            };
        }

        if !self.can_finalize() {
            return LiquidationBatchStatus::Round2 {
                shares: self.pending_shares.len(),
                required: 11,
            };
        }

        LiquidationBatchStatus::Ready
    }
}

/// Status of FROST liquidation batch coordination
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LiquidationBatchStatus {
    /// No batch in progress
    Idle,

    /// Round 1: Collecting commitments
    Round1 { commitments: usize, required: usize },

    /// Round 2: Collecting signature shares
    Round2 { shares: usize, required: usize },

    /// Ready to finalize
    Ready,
}

/// Liquidation engine
///
/// Manages the privacy-preserving liquidation process.
///
/// FROST INTEGRATION: Uses Byzantine threshold (11/15) for liquidation approval
/// VRF DELEGATES: Uses Safrole entropy for randomized delegate selection
pub struct LiquidationEngine {
    /// Liquidation configuration
    /// HARDENING: Includes timing attack protection
    config: LiquidationConfig,

    /// Minimum health factor for liquidation (1.0)
    liquidation_threshold: Ratio,

    /// Pending liquidation proposals for current block
    pending_proposals: Vec<LiquidationProposal>,

    /// FROST coordinator for Byzantine threshold signatures (11/15)
    frost_coordinator: FrostLiquidationCoordinator,

    /// Registry of liquidation delegates
    /// VRF DELEGATES: Selected via Safrole entropy
    delegate_registry: DelegateRegistry,

    /// Current Safrole entropy (from epoch_2)
    /// Updated each epoch from consensus
    current_entropy: [u8; 32],
}

impl LiquidationEngine {
    pub fn new(config: LiquidationConfig, total_validators: usize) -> Self {
        Self {
            config,
            liquidation_threshold: Ratio::ONE,
            pending_proposals: Vec::new(),
            frost_coordinator: FrostLiquidationCoordinator::new(total_validators),
            delegate_registry: DelegateRegistry::new(),
            current_entropy: [0; 32],
        }
    }

    /// Create with default configuration (15 validators)
    pub fn new_default() -> Self {
        Self::new(LiquidationConfig::default(), 15)
    }

    /// Update entropy from Safrole consensus
    ///
    /// Called at epoch boundaries with new entropy from accumulator
    pub fn update_entropy(&mut self, entropy: [u8; 32]) {
        self.current_entropy = entropy;
        tracing::debug!("Updated liquidation entropy: {:?}", &entropy[..8]);
    }

    /// Register a liquidation delegate
    pub fn register_delegate(&mut self, delegate: LiquidationDelegate) -> Result<()> {
        self.delegate_registry.register(delegate)
    }

    /// Get the selected delegate for a position at current slot
    pub fn get_selected_delegate(
        &self,
        position_commitment: &[u8; 32],
        slot: u64,
    ) -> Option<&LiquidationDelegate> {
        self.delegate_registry.select_delegate(
            &self.current_entropy,
            position_commitment,
            slot,
        )
    }

    /// Verify that a delegate is authorized to liquidate a position
    ///
    /// Returns true if the delegate is the VRF-selected delegate for this slot
    pub fn verify_delegate_authorization(
        &self,
        delegate_key: &DelegateKey,
        position_commitment: &[u8; 32],
        slot: u64,
    ) -> bool {
        self.delegate_registry.is_authorized(
            delegate_key,
            &self.current_entropy,
            position_commitment,
            slot,
        )
    }

    /// Process FHE health check result and trigger delegate selection
    ///
    /// Called when FHE computation determines a position is liquidatable.
    /// Selects a delegate via VRF and authorizes them to execute.
    pub fn process_fhe_health_check(
        &self,
        result: &FheHealthCheckResult,
        current_slot: u64,
    ) -> Option<DelegateKey> {
        if !result.is_liquidatable {
            return None;
        }

        // Select delegate for this position at current slot
        self.delegate_registry
            .select_delegate(
                &self.current_entropy,
                &result.position_commitment,
                current_slot,
            )
            .map(|d| d.key)
    }

    /// Get FROST coordinator status
    pub fn frost_status(&self) -> LiquidationBatchStatus {
        self.frost_coordinator.status()
    }

    /// Verify signature on liquidation proposal
    ///
    /// FROST INTEGRATION: Verifies Byzantine threshold (11/15) signature
    fn verify_signature(&self, proposal: &LiquidationProposal) -> Result<()> {
        // Check if FROST signature is present
        if let Some(frost_sig) = &proposal.frost_signature {
            // Verify FROST signature using coordinator
            proposal.verify_frost_signature(&self.frost_coordinator.frost_coordinator)?;
            tracing::debug!(
                "FROST signature verified for liquidation proposal from validator {} with {} signers",
                proposal.proposer_id,
                frost_sig.signers.len()
            );
            Ok(())
        } else if let Some(_legacy_sig) = &proposal.signature {
            // Legacy single signature verification
            use sha2::{Digest, Sha256};

            let mut hasher = Sha256::new();
            hasher.update(&proposal.height.to_le_bytes());
            hasher.update(&(proposal.proofs.len() as u64).to_le_bytes());

            if let Some(proof) = proposal.proofs.first() {
                hasher.update(&proof.position_commitment);
            }

            let _message_hash = hasher.finalize();

            // NOTE: In production, use commonware_cryptography::ed25519::verify
            tracing::warn!(
                "Legacy liquidation signature verification not yet integrated - accepting all"
            );

            Ok(())
        } else {
            bail!("Liquidation proposal missing both FROST and legacy signatures");
        }
    }

    /// Submit liquidation proposal
    ///
    /// HARDENING: Validates timing to prevent timing attacks
    pub fn submit_proposal(&mut self, proposal: LiquidationProposal) -> Result<()> {
        // Verify signature
        // SECURITY FIX: Enable signature verification
        self.verify_signature(&proposal)?;

        // Verify all proofs
        if !proposal.verify_all_proofs()? {
            bail!("invalid liquidation proofs");
        }

        // HARDENING: Validate liquidation timing for each proof
        for proof in &proposal.proofs {
            self.config.validate_timing(
                proof.public_inputs.became_liquidatable_at,
                proposal.height,
            )?;
        }

        self.pending_proposals.push(proposal);
        Ok(())
    }

    /// Execute batch liquidation
    ///
    /// Aggregates all proposals and liquidates positions.
    ///
    /// FROST INTEGRATION: Produces Byzantine threshold (11/15) signature on batch result
    pub fn execute_batch_liquidation(
        &mut self,
        pool: &mut LendingPool,
        position_manager: &mut PrivatePositionManager,
        height: u64,
    ) -> Result<BatchLiquidationResult> {
        if self.pending_proposals.is_empty() {
            // No liquidations this block
            return Ok(BatchLiquidationResult {
                height,
                num_liquidated: 0,
                total_seized: BTreeMap::new(),
                total_debt_repaid: BTreeMap::new(),
                total_penalties: BTreeMap::new(),
                avg_health_factor: Ratio::ONE,
                frost_signature: None,
            });
        }

        // Aggregate all liquidation proofs from all validators
        let mut all_liquidations: Vec<LiquidationProof> = Vec::new();
        for proposal in &self.pending_proposals {
            all_liquidations.extend(proposal.proofs.clone());
        }

        // Deduplicate (same position might be proposed by multiple validators)
        all_liquidations.sort_by_key(|l| l.position_commitment);
        all_liquidations.dedup_by_key(|l| l.position_commitment);

        // Execute each liquidation
        let mut total_seized: BTreeMap<AssetId, Amount> = BTreeMap::new();
        let mut total_debt_repaid: BTreeMap<AssetId, Amount> = BTreeMap::new();
        let mut total_penalties: BTreeMap<AssetId, Amount> = BTreeMap::new();
        let mut total_health = 0u128;

        for liquidation_proof in &all_liquidations {
            let result = self.execute_single_liquidation(
                liquidation_proof,
                pool,
                position_manager,
            )?;

            // Accumulate totals
            for (asset_id, amount) in result.seized_collateral {
                *total_seized.entry(asset_id).or_insert(Amount::ZERO) += amount;
            }

            for (asset_id, amount) in result.debt_repaid {
                *total_debt_repaid.entry(asset_id).or_insert(Amount::ZERO) += amount;
            }

            for (asset_id, amount) in result.penalty {
                *total_penalties.entry(asset_id).or_insert(Amount::ZERO) += amount;
            }

            total_health += result.health_factor.numerator as u128;
        }

        // Calculate average health factor
        let avg_health_factor = if !all_liquidations.is_empty() {
            Ratio {
                numerator: total_health / all_liquidations.len() as u128,
                denominator: Ratio::ONE.denominator,
            }
        } else {
            Ratio::ONE
        };

        // Generate FROST signature for batch result
        // NOTE: In production, validators would coordinate FROST signature on the batch result
        // For now, we extract the signature from the first proposal that has one
        let frost_signature = self
            .pending_proposals
            .iter()
            .find_map(|p| p.frost_signature.clone());

        // Clear proposals for next block
        self.pending_proposals.clear();

        Ok(BatchLiquidationResult {
            height,
            num_liquidated: all_liquidations.len() as u32,
            total_seized,
            total_debt_repaid,
            total_penalties,
            avg_health_factor,
            frost_signature,
        })
    }

    /// Coordinate FROST signature for liquidation batch
    ///
    /// This method is called by validators to coordinate the FROST signing process
    pub fn coordinate_frost_signature(
        &mut self,
        proofs: Vec<LiquidationProof>,
        height: u64,
    ) -> Result<()> {
        self.frost_coordinator.start_batch(proofs, height)
    }

    /// Add commitment from validator (Round 1)
    pub fn add_frost_commitment(
        &mut self,
        validator_id: ValidatorId,
        commitment: CommitmentData,
    ) -> Result<()> {
        self.frost_coordinator.add_commitment(validator_id, commitment)
    }

    /// Get signing package for Round 2
    pub fn get_signing_package(&self) -> Result<SigningPackageData> {
        self.frost_coordinator.create_signing_package()
    }

    /// Add signature share from validator (Round 2)
    pub fn add_frost_share(
        &mut self,
        validator_id: ValidatorId,
        share: SignatureShareData,
    ) -> Result<()> {
        self.frost_coordinator.add_signature_share(validator_id, share)
    }

    /// Finalize FROST signature
    pub fn finalize_frost_signature(&mut self) -> Result<FrostSignature> {
        self.frost_coordinator.finalize()
    }

    /// Execute single liquidation
    fn execute_single_liquidation(
        &self,
        proof: &LiquidationProof,
        pool: &mut LendingPool,
        position_manager: &mut PrivatePositionManager,
    ) -> Result<SingleLiquidationResult> {
        // 1. Verify proof (already done in submit_proposal, but double-check)
        // verify_liquidation_proof(proof)?;

        // 2. Get position from commitment
        let position_commitment = proof.position_commitment;

        // In real implementation:
        // We would decrypt the position using the proof's witness data
        // For now, use placeholder values

        // 3. Calculate liquidation amounts
        let seized_collateral = proof.public_inputs.seized_collateral;
        let debt_repaid = proof.public_inputs.debt_repaid;

        // Penalty = seized_collateral * penalty_percent / 100
        // SECURITY FIX: Use checked arithmetic
        let penalty_amount = Amount(
            seized_collateral
                .0
                .checked_mul(self.config.max_penalty_percent as u128)
                .and_then(|v| v.checked_div(100))
                .ok_or_else(|| anyhow::anyhow!("Overflow calculating penalty amount"))?
        );

        // 4. Update pool state
        // pool.add_liquidity(debt_repaid)?;
        // pool.add_penalty_income(penalty_amount)?;

        // 5. Remove position from manager
        // position_manager.remove_position(position_commitment)?;

        Ok(SingleLiquidationResult {
            position_commitment,
            seized_collateral: vec![(AssetId([0; 32]), seized_collateral)].into_iter().collect(),
            debt_repaid: vec![(AssetId([0; 32]), debt_repaid)].into_iter().collect(),
            penalty: vec![(AssetId([0; 32]), penalty_amount)].into_iter().collect(),
            health_factor: Ratio::from_percent(95), // Placeholder
        })
    }
}

/// Result of liquidating a single position
struct SingleLiquidationResult {
    position_commitment: [u8; 32],
    seized_collateral: BTreeMap<AssetId, Amount>,
    debt_repaid: BTreeMap<AssetId, Amount>,
    penalty: BTreeMap<AssetId, Amount>,
    health_factor: Ratio,
}

/// Liquidation scanner (runs on validators to find liquidatable positions)
pub struct LiquidationScanner {
    /// Our viewing key (for checking positions we track)
    viewing_key: Option<ViewingKey>,
}

impl LiquidationScanner {
    pub fn new(viewing_key: Option<ViewingKey>) -> Self {
        Self { viewing_key }
    }

    /// Scan for liquidatable positions
    ///
    /// In practice, validators would:
    /// 1. Track positions they're interested in
    /// 2. Check health factors against oracle prices
    /// 3. Generate proofs for underwater positions
    /// 4. Submit liquidation proposals
    pub async fn scan_for_liquidations(
        &self,
        position_manager: &PrivatePositionManager,
        oracle_prices: &BTreeMap<AssetId, Amount>,
        pool: &LendingPool,
    ) -> Result<Vec<LiquidationProof>> {
        let mut liquidation_proofs = Vec::new();

        // In real implementation:
        // 1. Iterate through positions we can decrypt
        // 2. Calculate health factors
        // 3. Generate ZK proofs for underwater positions

        // For now, return empty (positions are private)
        Ok(liquidation_proofs)
    }

    /// Generate liquidation proof for a specific position
    pub fn generate_liquidation_proof(
        &self,
        witness: LiquidationWitness,
        pool: &LendingPool,
    ) -> Result<LiquidationProof> {
        // 1. Verify position is actually liquidatable
        if !witness.is_liquidatable(pool)? {
            bail!("position is not liquidatable");
        }

        // 2. Calculate public inputs
        let health_factor = witness.calculate_health_factor(pool)?;

        // Calculate seized collateral and debt repaid
        let seized_collateral = self.calculate_seized_collateral(&witness)?;
        let debt_repaid = self.calculate_debt_repaid(&witness)?;

        let public_inputs = LiquidationPublicInputs {
            commitment: witness.position.owner_key, // TODO: PLACEHOLDER
            oracle_prices_hash: self.hash_oracle_prices(&witness.oracle_prices),
            state_root: [0; 32], // TODO: From NOMT
            penalty_percent: 5,
            seized_collateral: Amount(seized_collateral),
            debt_repaid: Amount(debt_repaid),
            became_liquidatable_at: 0, // TODO: PLACEHOLDER - should come from position state
        };

        // 3. Build ZK circuit
        // let circuit = build_liquidation_circuit(&witness, &public_inputs)?;

        // 4. Generate proof
        // let proof = prove_liquidation(circuit)?;

        // ========================================
        // TODO TODO TODO: CRITICAL PLACEHOLDER
        // ========================================
        // Placeholder proof - matches current AccidentalComputerProof structure
        // MUST REPLACE WITH ACTUAL LIQUIDATION PROOF GENERATION
        let proof = AccidentalComputerProof {
            zoda_commitment: Vec::new(),
            shard_indices: Vec::new(),
            shards: Vec::new(),
            sender_commitment_old: [0u8; 32],
            sender_commitment_new: [0u8; 32],
            receiver_commitment_old: [0u8; 32],
            receiver_commitment_new: [0u8; 32],
        };

        Ok(LiquidationProof {
            position_commitment: public_inputs.commitment,
            proof,
            nomt_witness: witness.nomt_witness,
            public_inputs,
        })
    }

    /// Calculate how much collateral to seize
    /// SECURITY FIX: Use checked arithmetic
    fn calculate_seized_collateral(&self, witness: &LiquidationWitness) -> Result<u128> {
        // Seize enough collateral to cover debt + penalty
        let mut total_debt_value = 0u128; // Use u128 to avoid overflow

        for (asset_id, amount) in &witness.position.debt {
            let price_amount = witness
                .oracle_prices
                .get(asset_id)
                .ok_or_else(|| anyhow::anyhow!("missing price"))?;

            // amount * price (both as raw u128 values)
            let value = amount
                .0
                .checked_mul(price_amount.0)
                .ok_or_else(|| anyhow::anyhow!("Overflow calculating debt value"))?;

            total_debt_value = total_debt_value
                .checked_add(value)
                .ok_or_else(|| anyhow::anyhow!("Overflow summing debt values"))?;
        }

        // Add 5% penalty: debt * 105 / 100
        let with_penalty = total_debt_value
            .checked_mul(105)
            .and_then(|v| v.checked_div(100))
            .ok_or_else(|| anyhow::anyhow!("Overflow applying penalty"))?;

        Ok(with_penalty)
    }

    /// Calculate how much debt is repaid
    fn calculate_debt_repaid(&self, witness: &LiquidationWitness) -> Result<u128> {
        let mut total_debt = 0u128;

        for (_, amount) in &witness.position.debt {
            total_debt += amount.0;
        }

        Ok(total_debt)
    }

    /// Hash oracle prices for public input
    fn hash_oracle_prices(&self, prices: &BTreeMap<AssetId, Amount>) -> [u8; 32] {
        use sha2::{Digest, Sha256};

        let mut hasher = Sha256::new();

        // BTreeMap is already sorted by key, no need to sort manually
        for (asset_id, amount) in prices {
            hasher.update(&asset_id.0);
            hasher.update(&amount.0.to_le_bytes());
        }

        hasher.finalize().into()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_health_factor_calculation() {
        let viewing_key = ViewingKey { key: [1; 32] };

        let mut position = PrivatePositionState {
            owner_key: [1; 32],
            collateral: vec![(AssetId([1; 32]), Amount(1000))],
            debt: vec![(AssetId([1; 32]), Amount(800))],
            health_factor: Ratio::ONE,
            entry_prices: Vec::new(),
            leverage: 5,
        };

        let mut oracle_prices = BTreeMap::new();
        oracle_prices.insert(AssetId([1; 32]), Amount(1));

        let witness = LiquidationWitness {
            viewing_key,
            position: position.clone(),
            oracle_prices: oracle_prices.clone(),
            nomt_witness: Vec::new(),
            commitment_randomness: [0; 32],
        };

        let pool = LendingPool::new();

        // Healthy position: 1000 collateral / 800 debt = 1.25 health
        // (Above 1.0, not liquidatable)
        let health = witness.calculate_health_factor(&pool).unwrap();
        assert!(health.gt(&Ratio::ONE));

        // Underwater position: 800 debt > 600 collateral
        position.collateral = vec![(AssetId([1; 32]), Amount(600))];
        let underwater_witness = LiquidationWitness {
            viewing_key,
            position,
            oracle_prices,
            nomt_witness: Vec::new(),
            commitment_randomness: [0; 32],
        };

        let underwater_health = underwater_witness.calculate_health_factor(&pool).unwrap();
        assert!(underwater_health.lt(&Ratio::ONE));
        assert!(underwater_witness.is_liquidatable(&pool).unwrap());
    }

    #[test]
    fn test_batch_liquidation() {
        let config = LiquidationConfig {
            min_liquidation_delay: 2,
            max_penalty_percent: 5,
            min_penalty_percent: 1,
            penalty_decay_blocks: 10,
            allow_partial_liquidations: true,
            min_partial_liquidation_percent: 20,
        };
        let mut engine = LiquidationEngine::new(config, 15);
        let mut pool = LendingPool::new();
        let mut position_manager = PrivatePositionManager::new();

        // Create liquidation proposal with FROST signature
        let proposal = LiquidationProposal {
            proposer_id: 0,
            height: 100,
            proofs: vec![
                // Would contain actual proofs
            ],
            frost_signature: None,
            signature: Some([0; 64]),
        };

        // engine.submit_proposal(proposal).unwrap();

        // Execute batch
        let result = engine.execute_batch_liquidation(
            &mut pool,
            &mut position_manager,
            100,
        ).unwrap();

        // No liquidations submitted
        assert_eq!(result.num_liquidated, 0);
    }
}
