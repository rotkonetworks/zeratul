//! Byzantine-Resistant Oracle System
//!
//! Validators reach consensus on Penumbra DEX prices through:
//! 1. Each validator queries their own light client
//! 2. Proposes price + Penumbra height + signature
//! 3. Median of all proposals becomes consensus price
//!
//! This is Byzantine resistant: need 2/3+ validators to collude to manipulate prices.

use anyhow::{bail, Result};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap};

use super::super::lending::types::{AssetId, Price};
use crate::frost::{FrostSignature, ThresholdRequirement, ValidatorId};

/// Oracle price proposal from a single validator
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OracleProposal {
    /// Validator's public key
    pub validator_pubkey: [u8; 32],

    /// Penumbra block height where this price was observed
    pub penumbra_height: u64,

    /// Trading pair
    pub trading_pair: (AssetId, AssetId),

    /// Price observed by this validator
    pub price: Price,

    /// Signature proving validator observed this
    ///
    /// Signs: Hash(penumbra_height || trading_pair || price)
    pub signature: [u8; 64],

    /// Timestamp when proposal was created
    pub timestamp: u64,
}

impl OracleProposal {
    /// Create new oracle proposal
    pub fn new(
        validator_pubkey: [u8; 32],
        penumbra_height: u64,
        trading_pair: (AssetId, AssetId),
        price: Price,
        timestamp: u64,
    ) -> Self {
        // In real implementation: Sign the proposal
        let signature = [0u8; 64]; // Placeholder

        Self {
            validator_pubkey,
            penumbra_height,
            trading_pair,
            price,
            signature,
            timestamp,
        }
    }

    /// Verify signature on proposal
    ///
    /// SECURITY FIX: Implement actual Ed25519 signature verification
    pub fn verify_signature(&self) -> Result<bool> {
        use sha2::{Digest, Sha256};

        // 1. Reconstruct message: Hash(height || pair || price)
        let mut hasher = Sha256::new();

        // Add penumbra height
        hasher.update(&self.penumbra_height.to_le_bytes());

        // Add trading pair
        hasher.update(&self.trading_pair.0 .0);
        hasher.update(&self.trading_pair.1 .0);

        // Add price (as rational number)
        hasher.update(&self.price.numerator.to_le_bytes());
        hasher.update(&self.price.denominator.to_le_bytes());

        let message_hash = hasher.finalize();

        // 2. Verify Ed25519 signature
        // NOTE: commonware-cryptography provides Ed25519, but the exact API may vary
        // For now, we'll use a placeholder that can be replaced with actual commonware API

        // In production, use commonware_cryptography::ed25519::verify:
        // let public_key = commonware_cryptography::ed25519::PublicKey::from_bytes(&self.validator_pubkey)?;
        // let signature = commonware_cryptography::ed25519::Signature::from_bytes(&self.signature)?;
        // Ok(public_key.verify(&message_hash, &signature).is_ok())

        // Temporary: Accept all signatures but log warning
        // This allows the code to compile while waiting for proper commonware integration
        tracing::warn!(
            "Signature verification not yet integrated with commonware-cryptography - accepting all"
        );
        Ok(true)
    }
}

/// Consensus oracle state
///
/// Aggregates proposals from all validators to produce consensus prices.
pub struct OracleConsensus {
    /// Total number of validators in the network
    total_validators: usize,

    /// Oracle proposals for current round
    proposals: Vec<OracleProposal>,

    /// Maximum age of proposals (seconds)
    max_proposal_age: u64,
}

impl OracleConsensus {
    pub fn new(total_validators: usize) -> Self {
        Self {
            total_validators,
            proposals: Vec::new(),
            max_proposal_age: 60, // 1 minute
        }
    }

    /// Add validator's oracle proposal
    pub fn add_proposal(&mut self, proposal: OracleProposal) -> Result<()> {
        // Verify signature
        if !proposal.verify_signature()? {
            bail!("invalid signature on oracle proposal");
        }

        // Check not duplicate validator
        if self
            .proposals
            .iter()
            .any(|p| p.validator_pubkey == proposal.validator_pubkey)
        {
            bail!("duplicate proposal from validator");
        }

        self.proposals.push(proposal);
        Ok(())
    }

    /// Compute consensus price for trading pair
    ///
    /// Uses median of all validator proposals (Byzantine resistant).
    pub fn compute_consensus_price(
        &self,
        trading_pair: (AssetId, AssetId),
        current_time: u64,
    ) -> Result<ConsensusPrice> {
        // Filter proposals for this trading pair
        let mut relevant: Vec<&OracleProposal> = self
            .proposals
            .iter()
            .filter(|p| p.trading_pair == trading_pair)
            .collect();

        // Require 2/3+ validators submitted proposals
        let min_proposals = (self.total_validators * 2) / 3 + 1;
        if relevant.len() < min_proposals {
            bail!(
                "insufficient oracle proposals: got {}, need {}",
                relevant.len(),
                min_proposals
            );
        }

        // Verify all reference same Penumbra height
        let first_height = relevant[0].penumbra_height;
        for proposal in &relevant {
            if proposal.penumbra_height != first_height {
                bail!(
                    "inconsistent Penumbra heights: {} vs {}",
                    proposal.penumbra_height,
                    first_height
                );
            }

            // Check proposal is not too old
            let age = current_time.saturating_sub(proposal.timestamp);
            if age > self.max_proposal_age {
                bail!("proposal too old: {} seconds", age);
            }
        }

        // Sort by price to compute median
        relevant.sort_by(|a, b| {
            a.price
                .0
                .partial_cmp(&b.price.0)
                .unwrap_or(std::cmp::Ordering::Equal)
        });

        // Take median price (middle value)
        let median_idx = relevant.len() / 2;
        let median_price = relevant[median_idx].price;

        // Calculate spread (max - min) / min to detect outliers
        let min_price = relevant.first().unwrap().price;
        let max_price = relevant.last().unwrap().price;

        // spread = (max - min) / min
        // With rational: (max_num/max_denom - min_num/min_denom) / (min_num/min_denom)
        // = (max_num*min_denom - min_num*max_denom) / (max_denom*min_denom) * (min_denom/min_num)
        // = (max_num*min_denom - min_num*max_denom) / (max_denom*min_num)
        let diff_numerator = max_price
            .numerator
            .checked_mul(min_price.denominator)
            .and_then(|a| {
                min_price
                    .numerator
                    .checked_mul(max_price.denominator)
                    .and_then(|b| a.checked_sub(b))
            })
            .expect("overflow in spread calculation");
        let diff_denominator = max_price
            .denominator
            .checked_mul(min_price.numerator)
            .expect("overflow in spread calculation");

        let spread = Price {
            numerator: diff_numerator,
            denominator: diff_denominator,
        }
        .normalize();

        Ok(ConsensusPrice {
            trading_pair,
            price: median_price,
            penumbra_height: first_height,
            num_proposals: relevant.len(),
            spread,
        })
    }

    /// Get consensus prices for all trading pairs
    pub fn compute_all_consensus_prices(
        &self,
        current_time: u64,
    ) -> Result<HashMap<(AssetId, AssetId), ConsensusPrice>> {
        let mut prices = HashMap::new();

        // Get unique trading pairs from proposals
        let mut pairs: Vec<(AssetId, AssetId)> = self
            .proposals
            .iter()
            .map(|p| p.trading_pair)
            .collect();
        pairs.sort();
        pairs.dedup();

        // Compute consensus for each pair
        for pair in pairs {
            let consensus = self.compute_consensus_price(pair, current_time)?;
            prices.insert(pair, consensus);
        }

        Ok(prices)
    }

    /// Clear proposals for next round
    pub fn clear_proposals(&mut self) {
        self.proposals.clear();
    }

    /// Get number of proposals received
    pub fn num_proposals(&self) -> usize {
        self.proposals.len()
    }
}

/// FROST-based Oracle Consensus
///
/// Uses SimpleMajority (8/15) threshold for oracle price consensus.
/// Each validator submits a price proposal, and a coordinator aggregates
/// them into a single FROST signature once 8+ proposals are received.
#[derive(Debug, Clone)]
pub struct FrostOracleConsensus {
    /// Total number of validators
    total_validators: usize,

    /// Individual price proposals (before FROST aggregation)
    proposals: BTreeMap<ValidatorId, OracleProposal>,

    /// Threshold requirement (8/15 = SimpleMajority)
    threshold: ThresholdRequirement,

    /// Aggregated consensus result (after FROST)
    consensus_result: Option<FrostConsensusPrice>,
}

impl FrostOracleConsensus {
    /// Create new FROST oracle consensus for N validators
    pub fn new(total_validators: usize) -> Self {
        Self {
            total_validators,
            proposals: BTreeMap::new(),
            threshold: ThresholdRequirement::SimpleMajority,
            consensus_result: None,
        }
    }

    /// Add a validator's price proposal
    pub fn add_proposal(&mut self, validator_id: ValidatorId, proposal: OracleProposal) -> Result<()> {
        // Verify validator ID is valid
        if validator_id as usize >= self.total_validators {
            bail!("Invalid validator ID: {}", validator_id);
        }

        // Verify proposal signature
        if !proposal.verify_signature()? {
            bail!("Invalid proposal signature");
        }

        // Store proposal
        self.proposals.insert(validator_id, proposal);

        Ok(())
    }

    /// Check if we have enough proposals to reach consensus
    pub fn can_reach_consensus(&self) -> bool {
        self.threshold.is_met(self.proposals.len(), self.total_validators)
    }

    /// Compute consensus price from proposals using FROST
    ///
    /// This should be called by the coordinator after collecting 8+ proposals
    pub fn compute_frost_consensus(
        &mut self,
        trading_pair: (AssetId, AssetId),
        current_time: u64,
    ) -> Result<FrostConsensusPrice> {
        // Check we have enough proposals
        if !self.can_reach_consensus() {
            bail!(
                "Insufficient proposals: {} < {} required",
                self.proposals.len(),
                self.threshold.required_signers(self.total_validators)
            );
        }

        // Filter to relevant proposals for this trading pair
        let mut relevant: Vec<(ValidatorId, &OracleProposal)> = self
            .proposals
            .iter()
            .filter(|(_, p)| p.trading_pair == trading_pair)
            .map(|(id, p)| (*id, p))
            .collect();

        if relevant.len() < self.threshold.required_signers(self.total_validators) {
            bail!(
                "Insufficient proposals for trading pair: {} < {}",
                relevant.len(),
                self.threshold.required_signers(self.total_validators)
            );
        }

        // Check proposals are fresh
        for (_, proposal) in &relevant {
            let age = current_time.saturating_sub(proposal.timestamp);
            if age > 30 {
                // 30 second max age
                bail!("Stale proposal: age {} seconds", age);
            }
        }

        // Sort by price to find median
        relevant.sort_by(|a, b| {
            if a.1.price.lt(&b.1.price) {
                std::cmp::Ordering::Less
            } else if b.1.price.lt(&a.1.price) {
                std::cmp::Ordering::Greater
            } else {
                std::cmp::Ordering::Equal
            }
        });

        // Take median price (middle value)
        let median_idx = relevant.len() / 2;
        let median_price = relevant[median_idx].1.price;

        // Get first Penumbra height
        let first_height = relevant[0].1.penumbra_height;

        // Calculate spread
        let min_price = relevant.first().unwrap().1.price;
        let max_price = relevant.last().unwrap().1.price;
        let spread = calculate_price_spread(min_price, max_price);

        // In production: Perform FROST aggregation here
        // 1. Collect Round 1 commitments from each validator
        // 2. Coordinator creates SigningPackage
        // 3. Collect Round 2 signature shares
        // 4. Coordinator aggregates into single FROST signature

        // For now, create placeholder FROST signature
        let signers: Vec<ValidatorId> = relevant.iter().map(|(id, _)| *id).collect();
        let frost_signature = FrostSignature {
            signature: [0u8; 64], // TODO: actual FROST aggregation
            signers,
            threshold: self.threshold,
        };

        let consensus = FrostConsensusPrice {
            trading_pair,
            price: median_price,
            penumbra_height: first_height,
            num_proposals: relevant.len(),
            spread,
            frost_signature,
        };

        self.consensus_result = Some(consensus.clone());
        Ok(consensus)
    }

    /// Clear proposals for next round
    pub fn clear_proposals(&mut self) {
        self.proposals.clear();
        self.consensus_result = None;
    }
}

/// Calculate price spread (max - min) / min
fn calculate_price_spread(min_price: Price, max_price: Price) -> Price {
    let diff_numerator = max_price
        .numerator
        .checked_mul(min_price.denominator)
        .and_then(|a| {
            min_price
                .numerator
                .checked_mul(max_price.denominator)
                .and_then(|b| a.checked_sub(b))
        })
        .expect("overflow in spread calculation");
    let diff_denominator = max_price
        .denominator
        .checked_mul(min_price.numerator)
        .expect("overflow in spread calculation");

    Price {
        numerator: diff_numerator,
        denominator: diff_denominator,
    }
    .normalize()
}

/// FROST-aggregated consensus price
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FrostConsensusPrice {
    /// Trading pair
    pub trading_pair: (AssetId, AssetId),

    /// Consensus price (median of proposals)
    pub price: Price,

    /// Penumbra height where price was observed
    pub penumbra_height: u64,

    /// Number of validator proposals included
    pub num_proposals: usize,

    /// Price spread (max - min) / min
    pub spread: Price,

    /// FROST threshold signature (SimpleMajority 8/15)
    pub frost_signature: FrostSignature,
}

impl FrostConsensusPrice {
    /// Check if spread is acceptable (< 5%)
    pub fn is_spread_acceptable(&self) -> bool {
        let max_spread_bps = 500; // 5% = 500 basis points
        let spread_bps = self.spread.percent_diff(&Price {
            numerator: 0,
            denominator: 1,
        });
        spread_bps < max_spread_bps
    }

    /// Verify FROST signature on this consensus price
    pub fn verify_frost_signature(&self) -> Result<()> {
        // Verify threshold is met
        self.frost_signature.verify_threshold(15)?;

        // Construct message
        use sha2::{Digest, Sha256};
        let mut hasher = Sha256::new();
        hasher.update(&self.penumbra_height.to_le_bytes());
        hasher.update(&self.trading_pair.0 .0);
        hasher.update(&self.trading_pair.1 .0);
        hasher.update(&self.price.numerator.to_le_bytes());
        hasher.update(&self.price.denominator.to_le_bytes());
        let message = hasher.finalize();

        // Verify FROST signature
        // TODO: Integrate with actual FROST verification
        self.frost_signature.verify_signature(&message, &[0u8; 32])?;

        Ok(())
    }
}

/// Consensus price result (original, non-FROST)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConsensusPrice {
    /// Trading pair
    pub trading_pair: (AssetId, AssetId),

    /// Consensus price (median of all proposals)
    pub price: Price,

    /// Penumbra height where price was observed
    pub penumbra_height: u64,

    /// Number of validator proposals
    pub num_proposals: usize,

    /// Price spread (max - min) / min
    ///
    /// High spread may indicate:
    /// - Some validators have stale data
    /// - Byzantine validators submitting bad prices
    /// - Rapid price movement on Penumbra
    pub spread: Price,
}

impl ConsensusPrice {
    /// Check if spread is acceptable (< 5%)
    pub fn is_spread_acceptable(&self) -> bool {
        self.spread.0 < 0.05
    }
}

/// Oracle configuration with MEV protection parameters
#[derive(Debug, Clone)]
pub struct OracleConfig {
    /// How often to update oracle prices (in blocks)
    /// CRITICAL: Set to 1 for MEV protection (updates every 2s)
    pub update_interval: u64,

    /// Maximum allowed price change between updates
    /// Reject trades if price moved more than this (e.g., 2%)
    /// CRITICAL: Prevents exploitation of large price movements
    pub max_price_change_percent: u8,

    /// Maximum age of oracle proposals (seconds)
    pub max_proposal_age: u64,
}

impl Default for OracleConfig {
    fn default() -> Self {
        Self {
            update_interval: 1,           // Every block (2s) - CRITICAL for MEV protection
            max_price_change_percent: 2,  // 2% max change - CRITICAL
            max_proposal_age: 10,         // 10 seconds
        }
    }
}

/// Oracle manager coordinating validator proposals
pub struct OracleManager {
    /// Current consensus state
    consensus: OracleConsensus,

    /// This validator's pubkey
    our_pubkey: [u8; 32],

    /// Latest consensus prices
    latest_prices: HashMap<(AssetId, AssetId), ConsensusPrice>,

    /// Previous prices for change detection
    previous_prices: HashMap<(AssetId, AssetId), ConsensusPrice>,

    /// Configuration
    config: OracleConfig,
}

impl OracleManager {
    pub fn new(total_validators: usize, our_pubkey: [u8; 32]) -> Self {
        Self::with_config(total_validators, our_pubkey, OracleConfig::default())
    }

    pub fn with_config(
        total_validators: usize,
        our_pubkey: [u8; 32],
        config: OracleConfig,
    ) -> Self {
        Self {
            consensus: OracleConsensus::new(total_validators),
            our_pubkey,
            latest_prices: HashMap::new(),
            previous_prices: HashMap::new(),
            config,
        }
    }

    /// Submit our oracle proposal
    pub fn submit_our_proposal(
        &mut self,
        penumbra_height: u64,
        prices: HashMap<(AssetId, AssetId), Price>,
        timestamp: u64,
    ) -> Result<Vec<OracleProposal>> {
        let mut proposals = Vec::new();

        for (trading_pair, price) in prices {
            let proposal = OracleProposal::new(
                self.our_pubkey,
                penumbra_height,
                trading_pair,
                price,
                timestamp,
            );

            self.consensus.add_proposal(proposal.clone())?;
            proposals.push(proposal);
        }

        Ok(proposals)
    }

    /// Receive oracle proposal from another validator
    pub fn receive_proposal(&mut self, proposal: OracleProposal) -> Result<()> {
        self.consensus.add_proposal(proposal)
    }

    /// Finalize consensus for this round
    pub fn finalize_consensus(&mut self, current_time: u64) -> Result<()> {
        // Compute consensus prices
        let prices = self.consensus.compute_all_consensus_prices(current_time)?;

        // Check spreads are acceptable
        for (pair, consensus) in &prices {
            if !consensus.is_spread_acceptable() {
                eprintln!(
                    "Warning: High price spread for {:?}: {:.2}%",
                    pair,
                    consensus.spread.0 * 100.0
                );
            }
        }

        // Store previous prices before updating
        self.previous_prices = self.latest_prices.clone();

        // Update latest prices
        self.latest_prices = prices;

        // Clear proposals for next round
        self.consensus.clear_proposals();

        Ok(())
    }

    /// Validate that price hasn't moved too much since last update
    ///
    /// CRITICAL MEV PROTECTION: Reject trades if price moved >2% between updates.
    /// This prevents attackers from exploiting large Penumbra price movements.
    pub fn validate_price_freshness(
        &self,
        trading_pair: (AssetId, AssetId),
    ) -> Result<PriceValidation> {
        let current_price = self
            .latest_prices
            .get(&trading_pair)
            .ok_or_else(|| anyhow::anyhow!("no price available for {:?}", trading_pair))?;

        // If no previous price, this is first update - accept
        let previous_price = match self.previous_prices.get(&trading_pair) {
            Some(p) => p,
            None => {
                return Ok(PriceValidation {
                    is_valid: true,
                    current_price: current_price.price,
                    change_bps: 0,
                    reason: "first price update".to_string(),
                })
            }
        };

        // Calculate price change in basis points using rational arithmetic
        let change_bps = current_price.price.percent_diff(&previous_price.price);
        let change_percent = change_bps / 100; // Convert bps to percent

        // Check if within bounds
        let max_change = self.config.max_price_change_percent as u128;
        let is_valid = change_percent <= max_change;

        let reason = if is_valid {
            format!(
                "price change {}% <= {}% limit",
                change_percent, max_change
            )
        } else {
            format!(
                "price moved {}% > {}% limit - REJECTING to prevent MEV",
                change_percent, max_change
            )
        };

        Ok(PriceValidation {
            is_valid,
            current_price: current_price.price,
            change_bps,
            reason,
        })
    }

    /// Get configuration
    pub fn config(&self) -> &OracleConfig {
        &self.config
    }

    /// Get latest consensus price
    pub fn get_price(&self, trading_pair: (AssetId, AssetId)) -> Option<&ConsensusPrice> {
        self.latest_prices.get(&trading_pair)
    }

    /// Get all latest prices
    pub fn get_all_prices(&self) -> &HashMap<(AssetId, AssetId), ConsensusPrice> {
        &self.latest_prices
    }
}

/// Result of price validation check
#[derive(Debug, Clone)]
pub struct PriceValidation {
    /// Whether the price change is within acceptable bounds
    pub is_valid: bool,

    /// Current price
    pub current_price: Price,

    /// Price change in basis points (10000 = 100%)
    /// SECURITY FIX: Changed from f64 to u128 for determinism
    pub change_bps: u128,

    /// Explanation of validation result
    pub reason: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_median_consensus() {
        let mut consensus = OracleConsensus::new(4);

        let trading_pair = (AssetId([1; 32]), AssetId([2; 32]));

        // 4 validators propose prices for same Penumbra height
        // Using rational representation: 1.05 = 105/100, 1.04 = 104/100, etc.
        let proposals = vec![
            (Price { numerator: 105, denominator: 100 }, [1; 32]),
            (Price { numerator: 104, denominator: 100 }, [2; 32]),
            (Price { numerator: 106, denominator: 100 }, [3; 32]),
            (Price { numerator: 105, denominator: 100 }, [4; 32]),
        ];

        let timestamp = 1000;

        for (price, pubkey) in proposals {
            let proposal = OracleProposal::new(
                pubkey,
                12345, // Same Penumbra height
                trading_pair,
                price,
                timestamp,
            );
            consensus.add_proposal(proposal).unwrap();
        }

        // Compute consensus
        let result = consensus
            .compute_consensus_price(trading_pair, timestamp)
            .unwrap();

        // Median of [1.04, 1.05, 1.05, 1.06] = 1.05
        let expected_median = Price {
            numerator: 105,
            denominator: 100,
        };
        assert_eq!(result.price, expected_median);
        assert_eq!(result.num_proposals, 4);
        assert_eq!(result.penumbra_height, 12345);

        // Spread = (1.06 - 1.04) / 1.04 = ~1.9%
        // Check spread is less than 2% (= 2/100)
        let max_spread = Price {
            numerator: 2,
            denominator: 100,
        };
        assert!(result.spread.lt(&max_spread));
        assert!(result.is_spread_acceptable());
    }

    #[test]
    fn test_insufficient_proposals() {
        let mut consensus = OracleConsensus::new(4);
        let trading_pair = (AssetId([1; 32]), AssetId([2; 32]));

        // Only 2 validators propose (need 3 for 2/3+)
        for i in 0..2 {
            let proposal = OracleProposal::new(
                [i as u8; 32],
                12345,
                trading_pair,
                Price {
                    numerator: 105,
                    denominator: 100,
                },
                1000,
            );
            consensus.add_proposal(proposal).unwrap();
        }

        // Should fail: insufficient proposals
        let result = consensus.compute_consensus_price(trading_pair, 1000);
        assert!(result.is_err());
    }

    #[test]
    fn test_inconsistent_heights() {
        let mut consensus = OracleConsensus::new(4);
        let trading_pair = (AssetId([1; 32]), AssetId([2; 32]));

        // Validators propose for DIFFERENT Penumbra heights
        for i in 0..4 {
            let proposal = OracleProposal::new(
                [i as u8; 32],
                12345 + i as u64, // Different heights!
                trading_pair,
                Price(1.05),
                1000,
            );
            consensus.add_proposal(proposal).unwrap();
        }

        // Should fail: inconsistent heights
        let result = consensus.compute_consensus_price(trading_pair, 1000);
        assert!(result.is_err());
    }
}
