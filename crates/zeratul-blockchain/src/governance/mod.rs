//! Governance Module
//!
//! Implements Nominated Proof-of-Stake (NPoS) with Phragmén election for validator selection.
//!
//! ## Architecture
//!
//! - **Nominators**: ZT token holders who vote for validator candidates
//! - **Validators**: 15 operators selected via Phragmén algorithm
//! - **FROST Integration**: Byzantine threshold signatures (11/15)
//! - **Economic Security**: Token-weighted voting + slashing

pub mod validator_selection;
pub mod phragmen;
pub mod staking;
pub mod rewards;
pub mod liquid_staking;
pub mod note_staking;
pub mod zoda_integration;
pub mod sassafras_staking;
pub mod dkg_integration;

pub use validator_selection::{ValidatorCandidate, ValidatorSet, CandidateRegistry};
pub use phragmen::{PhragmenElection, ElectionResult};
pub use staking::{NominatorState, StakingLedger};
pub use rewards::{RewardPool, PayoutInfo};
pub use liquid_staking::{LiquidStakingPool, FrostCustodyPool};
pub use note_staking::{StakeNote, NoteTreeState, EraTransition, StakingAction};
pub use zoda_integration::{ZodaEraTransition, ZodaHeader, LigeritoProof};
pub use sassafras_staking::{StakingTicket, TicketPool, SassafrasEraTransition};
pub use dkg_integration::{DKGGovernanceManager, ValidatorRegistry, SlashingEvent, SlashingReason};

use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// Account identifier
pub type AccountId = [u8; 32];

/// ZT token amount (base units)
pub type Balance = u128;

/// Validator index (0-14 for 15 validators)
pub type ValidatorIndex = u16;

/// Era number (24-hour period)
pub type EraIndex = u64;

/// Epoch number (4-hour period, 6 epochs per era)
pub type EpochIndex = u64;

/// Governance configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GovernanceConfig {
    /// Number of validators to select
    pub validator_set_size: usize,

    /// Minimum self-stake for validator candidates (in ZT)
    pub min_validator_stake: Balance,

    /// Minimum stake for nominators (in ZT)
    pub min_nominator_stake: Balance,

    /// Maximum nominations per nominator
    pub max_nominations: usize,

    /// Era duration in blocks (24 hours at 2-second blocks = 43,200)
    pub era_duration: u64,

    /// Unbonding period in days
    pub unbonding_period_days: u32,

    /// Block reward in ZT
    pub block_reward: Balance,
}

impl Default for GovernanceConfig {
    fn default() -> Self {
        Self {
            validator_set_size: 15,
            min_validator_stake: 10_000 * 10u128.pow(18), // 10K ZT
            min_nominator_stake: 100 * 10u128.pow(18),     // 100 ZT
            max_nominations: 16,
            era_duration: 43_200,                          // 24 hours
            unbonding_period_days: 7,
            block_reward: 10 * 10u128.pow(18),            // 10 ZT
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_governance_config_defaults() {
        let config = GovernanceConfig::default();
        assert_eq!(config.validator_set_size, 15);
        assert_eq!(config.max_nominations, 16);
        assert_eq!(config.era_duration, 43_200);
    }
}
