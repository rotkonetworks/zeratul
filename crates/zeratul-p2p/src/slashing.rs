//! Slashing with superlinear penalties (like Polkadot)
//!
//! ## Why superlinear slashing?
//!
//! Single validator offline: Likely accident → small penalty (1%)
//! 30% of validators offline together: Likely coordinated attack → large penalty (30%+)
//!
//! ## Polkadot's formula:
//!
//! ```
//! penalty = min(3 * (k / n)^2, 1)
//! ```
//!
//! where:
//! - k = number of validators slashed in same period
//! - n = total number of active validators
//! - Result is capped at 100%
//!
//! ## Examples:
//!
//! Total validators: 100
//!
//! 1 validator slashed:
//!   k/n = 1/100 = 0.01
//!   penalty = 3 * 0.01^2 = 0.0003 = 0.03%
//!
//! 10 validators slashed:
//!   k/n = 10/100 = 0.1
//!   penalty = 3 * 0.1^2 = 0.03 = 3%
//!
//! 30 validators slashed (coordinated attack):
//!   k/n = 30/100 = 0.3
//!   penalty = 3 * 0.3^2 = 0.27 = 27%
//!
//! 50 validators slashed:
//!   k/n = 50/100 = 0.5
//!   penalty = 3 * 0.5^2 = 0.75 = 75%
//!
//! 60+ validators slashed:
//!   k/n = 60/100 = 0.6
//!   penalty = 3 * 0.6^2 = 1.08 → capped at 100%

use serde::{Deserialize, Serialize};

/// Slashing penalty calculator (Polkadot-style)
#[derive(Debug, Clone)]
pub struct SlashingCalculator {
    /// Base penalty for offense type (before superlinear scaling)
    base_penalty_bps: u64,
}

impl SlashingCalculator {
    /// Create for offense type
    pub fn new(base_penalty_bps: u64) -> Self {
        Self { base_penalty_bps }
    }

    /// Calculate actual penalty with superlinear scaling
    ///
    /// Uses Polkadot formula: penalty = min(3 * (k/n)^2, 1)
    ///
    /// Then applies base penalty as minimum
    pub fn calculate_penalty(
        &self,
        num_slashed: u64,
        total_validators: u64,
    ) -> u64 {
        if total_validators == 0 {
            return self.base_penalty_bps;
        }

        // k/n ratio
        let ratio = num_slashed as f64 / total_validators as f64;

        // Polkadot formula: 3 * (k/n)^2
        let superlinear = 3.0 * ratio * ratio;

        // Cap at 100%
        let capped = superlinear.min(1.0);

        // Convert to basis points
        let penalty_bps = (capped * 10000.0) as u64;

        // Use max of base penalty and superlinear penalty
        penalty_bps.max(self.base_penalty_bps)
    }

    /// Get base penalty (single offender)
    pub fn base_penalty(&self) -> u64 {
        self.base_penalty_bps
    }
}

/// Slashing event with calculated penalty
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SlashingEvent {
    /// Validators being slashed
    pub validators: Vec<[u8; 32]>,

    /// Offense type
    pub offense: SlashingOffense,

    /// Calculated penalty (basis points)
    /// This accounts for superlinear scaling
    pub penalty_bps: u64,

    /// Epoch when slashing occurred
    pub epoch: u64,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub enum SlashingOffense {
    /// Invalid batch proof
    InvalidProof,
    /// Double signing
    DoubleSigning,
    /// Offline for too long
    LivenessFailure,
}

impl SlashingOffense {
    /// Base penalty before superlinear scaling
    pub fn base_penalty_bps(&self) -> u64 {
        match self {
            SlashingOffense::InvalidProof => 1000,      // 10%
            SlashingOffense::DoubleSigning => 2000,     // 20%
            SlashingOffense::LivenessFailure => 100,    // 1%
        }
    }

    /// Calculate penalty with superlinear scaling
    pub fn calculate_penalty(&self, num_slashed: u64, total_validators: u64) -> u64 {
        let calc = SlashingCalculator::new(self.base_penalty_bps());
        calc.calculate_penalty(num_slashed, total_validators)
    }
}

impl SlashingEvent {
    /// Create slashing event
    pub fn new(
        validators: Vec<[u8; 32]>,
        offense: SlashingOffense,
        total_validators: u64,
        epoch: u64,
    ) -> Self {
        let num_slashed = validators.len() as u64;
        let penalty_bps = offense.calculate_penalty(num_slashed, total_validators);

        Self {
            validators,
            offense,
            penalty_bps,
            epoch,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_single_validator_low_penalty() {
        let calc = SlashingCalculator::new(1000); // 10% base

        // 1 out of 100 validators
        let penalty = calc.calculate_penalty(1, 100);

        // Should be close to base penalty (superlinear is tiny)
        assert!(penalty >= 1000 && penalty < 1100);

        println!("1/100 validators: {}% penalty", penalty as f64 / 100.0);
    }

    #[test]
    fn test_coordinated_attack_high_penalty() {
        let calc = SlashingCalculator::new(1000); // 10% base

        // 30 out of 100 validators (coordinated attack)
        let penalty = calc.calculate_penalty(30, 100);

        // Superlinear should kick in: 3 * 0.3^2 = 0.27 = 27%
        assert!(penalty > 2500 && penalty < 2900);

        println!("30/100 validators: {}% penalty", penalty as f64 / 100.0);
    }

    #[test]
    fn test_majority_attack_max_penalty() {
        let calc = SlashingCalculator::new(1000); // 10% base

        // 60 out of 100 validators
        let penalty = calc.calculate_penalty(60, 100);

        // Should be capped at 100%
        assert_eq!(penalty, 10000); // 100%

        println!("60/100 validators: {}% penalty (capped)", penalty as f64 / 100.0);
    }

    #[test]
    fn test_slashing_event_double_sign() {
        let validators = vec![[1u8; 32], [2u8; 32], [3u8; 32]]; // 3 validators

        let event = SlashingEvent::new(
            validators.clone(),
            SlashingOffense::DoubleSigning,
            100, // out of 100 total
            1,
        );

        // Base penalty for double-signing is 20%
        // But 3/100 = 0.03, superlinear = 3 * 0.03^2 = 0.0027 = 0.27%
        // So we use base penalty (20%)
        assert!(event.penalty_bps >= 2000);

        println!("3 double-signers out of 100: {}% penalty", event.penalty_bps as f64 / 100.0);
    }

    #[test]
    fn test_liveness_failure_scales() {
        // 1 offline (accident)
        let event1 = SlashingEvent::new(
            vec![[1u8; 32]],
            SlashingOffense::LivenessFailure,
            100,
            1,
        );

        // 20 offline (coordinated?)
        let event2 = SlashingEvent::new(
            vec![[1u8; 32]; 20],
            SlashingOffense::LivenessFailure,
            100,
            1,
        );

        println!("1 offline: {}% penalty", event1.penalty_bps as f64 / 100.0);
        println!("20 offline: {}% penalty", event2.penalty_bps as f64 / 100.0);

        // Many offline should be much worse
        assert!(event2.penalty_bps > event1.penalty_bps * 5);
    }
}
