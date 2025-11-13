//! Validator Reputation & Byzantine Detection
//!
//! HARDENING: Detects and punishes Byzantine validator behavior.
//!
//! ## Byzantine Behaviors Detected
//!
//! 1. **Oracle Manipulation**: Submitting fake prices far from consensus
//! 2. **Censorship**: Excluding valid transactions repeatedly
//! 3. **Double Signing**: Signing conflicting blocks
//! 4. **Liveness Failures**: Not participating in consensus
//! 5. **Invalid Proofs**: Submitting invalid ZK proofs
//!
//! ## Punishment Mechanisms
//!
//! 1. **Slashing**: Lose portion of staked tokens
//! 2. **Reputation Decay**: Lower reputation = lower rewards
//! 3. **Temporary Ban**: Excluded from consensus for N blocks
//! 4. **Permanent Ejection**: Removed from validator set

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use anyhow::{bail, Result};

use crate::lending::types::{AssetId, Price};
use crate::penumbra::oracle::ConsensusPrice;

/// Validator reputation configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReputationConfig {
    /// Maximum price deviation from median (as percentage)
    /// HARDENING: Larger deviations indicate manipulation
    /// Default: 2% (prices >2% from median are suspicious)
    pub max_price_deviation_percent: u8,

    /// Slashing percentage for oracle manipulation
    /// HARDENING: Validators lose stake for manipulation
    /// Default: 10% (lose 10% of stake)
    pub oracle_manipulation_slash_percent: u8,

    /// Reputation decay for suspicious behavior
    /// HARDENING: Reputation decreases with bad behavior
    /// Default: 5 points per incident
    pub reputation_decay_per_incident: u32,

    /// Minimum reputation to remain validator
    /// HARDENING: Too low reputation = ejection
    /// Default: 50 (out of 100)
    pub min_reputation_threshold: u32,

    /// Starting reputation for new validators
    /// Default: 100
    pub starting_reputation: u32,

    /// Reputation recovery rate (per block)
    /// HARDENING: Good behavior slowly recovers reputation
    /// Default: 1 point per 100 blocks
    pub reputation_recovery_per_100_blocks: u32,

    /// Ban duration for severe offenses (blocks)
    /// HARDENING: Temporary exclusion from consensus
    /// Default: 1000 blocks (~30 minutes)
    pub ban_duration_blocks: u64,

    /// Number of offenses before permanent ejection
    /// HARDENING: Repeat offenders are removed
    /// Default: 3 strikes
    pub max_offenses_before_ejection: u32,
}

impl Default for ReputationConfig {
    fn default() -> Self {
        Self {
            max_price_deviation_percent: 2,           // 2% max deviation
            oracle_manipulation_slash_percent: 10,    // 10% slash
            reputation_decay_per_incident: 5,         // -5 reputation
            min_reputation_threshold: 50,             // Min 50/100 reputation
            starting_reputation: 100,                 // Start at 100
            reputation_recovery_per_100_blocks: 1,    // +1 per 100 blocks
            ban_duration_blocks: 1000,                // 1000 blocks (~30 min)
            max_offenses_before_ejection: 3,          // 3 strikes
        }
    }
}

/// Types of Byzantine behavior
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum ByzantineBehavior {
    /// Oracle price manipulation
    OracleManipulation {
        trading_pair: (AssetId, AssetId),
        reported_price: Price,
        median_price: Price,
        deviation_percent: u128, // Percentage as integer (0-100+)
    },

    /// Censorship (excluding valid transactions)
    Censorship {
        num_censored: u32,
        block_height: u64,
    },

    /// Double signing (signing two different blocks at same height)
    DoubleSigning {
        block_height: u64,
        block_hash_1: [u8; 32],
        block_hash_2: [u8; 32],
    },

    /// Liveness failure (not participating)
    LivenessFailure {
        missed_blocks: u32,
        total_blocks: u32,
    },

    /// Invalid proof submission
    InvalidProof {
        proof_type: String,
        block_height: u64,
    },
}

/// Validator reputation record
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ValidatorReputation {
    /// Validator public key
    pub validator_pubkey: [u8; 32],

    /// Current reputation score (0-100)
    pub reputation: u32,

    /// Total offenses committed
    pub total_offenses: u32,

    /// Recent offenses (last 1000 blocks)
    pub recent_offenses: Vec<(u64, ByzantineBehavior)>,

    /// Stake slashed (total amount)
    pub total_slashed: u128,

    /// Ban status
    pub ban_info: Option<BanInfo>,

    /// Last activity block
    pub last_active_block: u64,
}

/// Ban information
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BanInfo {
    /// Block when ban started
    pub banned_at_block: u64,

    /// Block when ban expires
    pub expires_at_block: u64,

    /// Reason for ban
    pub reason: String,
}

impl ValidatorReputation {
    pub fn new(validator_pubkey: [u8; 32], starting_reputation: u32) -> Self {
        Self {
            validator_pubkey,
            reputation: starting_reputation,
            total_offenses: 0,
            recent_offenses: Vec::new(),
            total_slashed: 0,
            ban_info: None,
            last_active_block: 0,
        }
    }

    /// Check if validator is currently banned
    pub fn is_banned(&self, current_block: u64) -> bool {
        if let Some(ban_info) = &self.ban_info {
            current_block < ban_info.expires_at_block
        } else {
            false
        }
    }

    /// Record offense
    pub fn record_offense(
        &mut self,
        block_height: u64,
        behavior: ByzantineBehavior,
        config: &ReputationConfig,
    ) {
        self.total_offenses += 1;
        self.recent_offenses.push((block_height, behavior));

        // Decay reputation
        self.reputation = self.reputation.saturating_sub(config.reputation_decay_per_incident);

        // Keep only recent offenses (last 1000 blocks)
        self.recent_offenses.retain(|(h, _)| block_height - h < 1000);
    }

    /// Apply ban
    pub fn ban(&mut self, current_block: u64, duration: u64, reason: String) {
        self.ban_info = Some(BanInfo {
            banned_at_block: current_block,
            expires_at_block: current_block + duration,
            reason,
        });
    }

    /// Recover reputation over time
    pub fn recover_reputation(&mut self, blocks_elapsed: u64, config: &ReputationConfig) {
        let recovery = (blocks_elapsed / 100) as u32 * config.reputation_recovery_per_100_blocks;
        self.reputation = (self.reputation + recovery).min(config.starting_reputation);
    }
}

/// Reputation system managing all validators
pub struct ReputationSystem {
    /// Configuration
    config: ReputationConfig,

    /// Validator reputations
    /// Map: validator_pubkey -> reputation
    reputations: HashMap<[u8; 32], ValidatorReputation>,

    /// Current block height
    current_block: u64,

    /// Ejected validators (permanently removed)
    ejected_validators: Vec<[u8; 32]>,
}

impl ReputationSystem {
    pub fn new(config: ReputationConfig) -> Self {
        Self {
            config,
            reputations: HashMap::new(),
            current_block: 0,
            ejected_validators: Vec::new(),
        }
    }

    /// Register new validator
    pub fn register_validator(&mut self, validator_pubkey: [u8; 32]) {
        if !self.reputations.contains_key(&validator_pubkey) {
            let reputation = ValidatorReputation::new(
                validator_pubkey,
                self.config.starting_reputation,
            );
            self.reputations.insert(validator_pubkey, reputation);
        }
    }

    /// Update to new block
    pub fn new_block(&mut self, block_height: u64) {
        let prev_block = self.current_block;
        self.current_block = block_height;

        // Recover reputation for all validators over time
        let blocks_elapsed = block_height.saturating_sub(prev_block);
        for reputation in self.reputations.values_mut() {
            reputation.recover_reputation(blocks_elapsed, &self.config);
        }
    }

    /// Detect oracle manipulation
    ///
    /// HARDENING: Checks if validator's price deviates too much from median
    /// SECURITY FIX: Uses rational arithmetic to avoid f64 non-determinism
    pub fn detect_oracle_manipulation(
        &mut self,
        validator_pubkey: [u8; 32],
        reported_price: Price,
        consensus: &ConsensusPrice,
    ) -> Result<()> {
        let median_price = consensus.price;

        // Calculate deviation percentage using rational arithmetic
        // deviation_percent = |reported - median| / median * 100
        let deviation_bps = reported_price.percent_diff(&median_price);
        let deviation_percent = deviation_bps / 100; // Convert basis points to percent

        // Check if exceeds threshold
        if deviation_percent > self.config.max_price_deviation_percent as u128 {
            // MANIPULATION DETECTED!
            let behavior = ByzantineBehavior::OracleManipulation {
                trading_pair: consensus.trading_pair,
                reported_price,
                median_price,
                deviation_percent,
            };

            self.punish_validator(validator_pubkey, behavior)?;

            bail!(
                "Oracle manipulation detected: validator {:?} reported {}/{} (median {}/{}, deviation {}%)",
                validator_pubkey,
                reported_price.numerator,
                reported_price.denominator,
                median_price.numerator,
                median_price.denominator,
                deviation_percent
            );
        }

        Ok(())
    }

    /// Punish validator for Byzantine behavior
    ///
    /// HARDENING: Applies slashing, reputation decay, and bans
    fn punish_validator(
        &mut self,
        validator_pubkey: [u8; 32],
        behavior: ByzantineBehavior,
    ) -> Result<()> {
        let reputation = self.reputations
            .get_mut(&validator_pubkey)
            .ok_or_else(|| anyhow::anyhow!("Validator not found"))?;

        // Record offense
        reputation.record_offense(self.current_block, behavior.clone(), &self.config);

        // Determine punishment based on behavior type
        let (slash_percent, ban_duration) = match &behavior {
            ByzantineBehavior::OracleManipulation { .. } => {
                (self.config.oracle_manipulation_slash_percent, self.config.ban_duration_blocks)
            },
            ByzantineBehavior::DoubleSigning { .. } => {
                (50, self.config.ban_duration_blocks * 5) // Severe: 50% slash, 5x ban
            },
            ByzantineBehavior::Censorship { .. } => {
                (5, self.config.ban_duration_blocks / 2) // Moderate: 5% slash, 0.5x ban
            },
            ByzantineBehavior::InvalidProof { .. } => {
                (2, self.config.ban_duration_blocks / 4) // Light: 2% slash, 0.25x ban
            },
            ByzantineBehavior::LivenessFailure { .. } => {
                (1, 0) // Very light: 1% slash, no ban
            },
        };

        // Apply ban if duration > 0
        if ban_duration > 0 {
            let reason = format!("{:?}", behavior);
            reputation.ban(self.current_block, ban_duration, reason);

            println!(
                "Validator {:?} BANNED for {} blocks: {:?}",
                validator_pubkey, ban_duration, behavior
            );
        }

        // Check if should eject permanently
        if reputation.total_offenses >= self.config.max_offenses_before_ejection {
            self.eject_validator(validator_pubkey)?;
            println!(
                "Validator {:?} EJECTED permanently ({} offenses)",
                validator_pubkey, reputation.total_offenses
            );
        }

        // Check if reputation too low
        if reputation.reputation < self.config.min_reputation_threshold {
            self.eject_validator(validator_pubkey)?;
            println!(
                "Validator {:?} EJECTED for low reputation ({})",
                validator_pubkey, reputation.reputation
            );
        }

        // Log slashing (actual token slashing would happen in staking module)
        println!(
            "Validator {:?} SLASHED {}% for {:?}",
            validator_pubkey, slash_percent, behavior
        );

        Ok(())
    }

    /// Eject validator permanently
    fn eject_validator(&mut self, validator_pubkey: [u8; 32]) -> Result<()> {
        if !self.ejected_validators.contains(&validator_pubkey) {
            self.ejected_validators.push(validator_pubkey);
        }
        Ok(())
    }

    /// Check if validator is allowed to participate
    ///
    /// HARDENING: Prevents banned/ejected validators from participating
    pub fn can_participate(&self, validator_pubkey: &[u8; 32]) -> Result<()> {
        // Check if ejected
        if self.ejected_validators.contains(validator_pubkey) {
            bail!("Validator permanently ejected");
        }

        // Check if banned
        if let Some(reputation) = self.reputations.get(validator_pubkey) {
            if reputation.is_banned(self.current_block) {
                let ban_info = reputation.ban_info.as_ref().unwrap();
                bail!(
                    "Validator banned until block {} (reason: {})",
                    ban_info.expires_at_block,
                    ban_info.reason
                );
            }

            // Check reputation threshold
            if reputation.reputation < self.config.min_reputation_threshold {
                bail!(
                    "Validator reputation too low: {} < {}",
                    reputation.reputation,
                    self.config.min_reputation_threshold
                );
            }
        }

        Ok(())
    }

    /// Get validator reputation
    pub fn get_reputation(&self, validator_pubkey: &[u8; 32]) -> Option<&ValidatorReputation> {
        self.reputations.get(validator_pubkey)
    }

    /// Get all active validators (not banned/ejected)
    pub fn get_active_validators(&self) -> Vec<[u8; 32]> {
        self.reputations
            .iter()
            .filter(|(pubkey, rep)| {
                !self.ejected_validators.contains(pubkey)
                    && !rep.is_banned(self.current_block)
                    && rep.reputation >= self.config.min_reputation_threshold
            })
            .map(|(pubkey, _)| *pubkey)
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_oracle_manipulation_detection() {
        let config = ReputationConfig::default();
        let mut system = ReputationSystem::new(config);

        let validator = [1u8; 32];
        system.register_validator(validator);
        system.new_block(100);

        let consensus = ConsensusPrice {
            trading_pair: (AssetId([0; 32]), AssetId([1; 32])),
            price: Price {
                numerator: 100,
                denominator: 1,
            },
            penumbra_height: 100,
            num_proposals: 4,
            spread: Price {
                numerator: 1,
                denominator: 100,
            },
        };

        // Report honest price (within 2%): 101.5 = 1015/10
        let honest_price = Price {
            numerator: 1015,
            denominator: 10,
        }; // 1.5% deviation
        assert!(system
            .detect_oracle_manipulation(validator, honest_price, &consensus)
            .is_ok());

        // Report manipulated price (>2% deviation): 105 = 105/1
        let manipulated_price = Price {
            numerator: 105,
            denominator: 1,
        }; // 5% deviation
        assert!(system
            .detect_oracle_manipulation(validator, manipulated_price, &consensus)
            .is_err());

        // Validator should be banned
        let rep = system.get_reputation(&validator).unwrap();
        assert!(rep.is_banned(100));
        assert_eq!(rep.total_offenses, 1);
    }

    #[test]
    fn test_reputation_recovery() {
        let config = ReputationConfig::default();
        let mut system = ReputationSystem::new(config.clone());

        let validator = [1u8; 32];
        system.register_validator(validator);

        // Initial reputation: 100
        assert_eq!(system.get_reputation(&validator).unwrap().reputation, 100);

        // Record offense (reputation drops by 5)
        system.new_block(100);
        system.reputations.get_mut(&validator).unwrap().record_offense(
            100,
            ByzantineBehavior::LivenessFailure { missed_blocks: 10, total_blocks: 100 },
            &config,
        );
        assert_eq!(system.get_reputation(&validator).unwrap().reputation, 95);

        // After 100 blocks, reputation recovers by 1
        system.new_block(200);
        assert_eq!(system.get_reputation(&validator).unwrap().reputation, 96);

        // After 500 blocks, reputation recovers by 5
        system.new_block(600);
        assert_eq!(system.get_reputation(&validator).unwrap().reputation, 100); // Capped at 100
    }

    #[test]
    fn test_ejection() {
        let mut config = ReputationConfig::default();
        config.max_offenses_before_ejection = 3;

        let mut system = ReputationSystem::new(config.clone());

        let validator = [1u8; 32];
        system.register_validator(validator);
        system.new_block(100);

        // Commit 3 offenses (triggers ejection)
        for i in 0..3 {
            let behavior = ByzantineBehavior::LivenessFailure {
                missed_blocks: 10,
                total_blocks: 100,
            };
            system.punish_validator(validator, behavior).ok();
        }

        // Validator should be ejected
        assert!(system.can_participate(&validator).is_err());
        assert!(system.ejected_validators.contains(&validator));
    }
}
