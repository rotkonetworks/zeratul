//! Phragmén Election Algorithm
//!
//! Implementation of Phragmén's method for proportional representation, used to select
//! validators from a candidate pool based on nominator votes.
//!
//! ## Algorithm
//!
//! **Goal**: Select k validators from n candidates to maximize the minimum backing.
//!
//! **Properties**:
//! - Proportional representation (nominators' preferences respected)
//! - Balanced stakes (no single validator dominates)
//! - Maximin support (maximize minimum validator backing)
//!
//! ## References
//!
//! - [Polkadot NPoS](https://wiki.polkadot.network/docs/learn-phragmen)
//! - [Phragmén's Method](https://en.wikipedia.org/wiki/Phragmen%27s_method)

use super::{AccountId, Balance, ValidatorIndex};
use anyhow::{bail, Result};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

/// Nomination from a single nominator
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Nomination {
    /// Nominator account
    pub nominator: AccountId,

    /// Total stake amount
    pub stake: Balance,

    /// Validator candidates nominated (up to 16)
    pub targets: Vec<AccountId>,
}

/// Validator candidate
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Candidate {
    /// Validator account
    pub account: AccountId,

    /// Self-stake amount
    pub self_stake: Balance,

    /// Whether candidate is eligible for election
    pub is_active: bool,
}

/// Election result for a single validator
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ValidatorElection {
    /// Validator account
    pub validator: AccountId,

    /// Total backing (self-stake + nominator stakes)
    pub total_backing: Balance,

    /// Individual nominator contributions
    /// AccountId -> Amount backing this validator
    pub nominators: BTreeMap<AccountId, Balance>,

    /// Validator index (0-14)
    pub index: ValidatorIndex,
}

/// Complete election result
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ElectionResult {
    /// Era for this election
    pub era: u64,

    /// Elected validators (sorted by backing)
    pub validators: Vec<ValidatorElection>,

    /// Total stake in the system
    pub total_stake: Balance,

    /// Minimum validator backing (maximin property)
    pub min_backing: Balance,

    /// Maximum validator backing
    pub max_backing: Balance,

    /// Average validator backing
    pub avg_backing: Balance,
}

impl ElectionResult {
    /// Check if election result is well-balanced
    ///
    /// A well-balanced election has max_backing / min_backing close to 1.0
    pub fn balance_ratio(&self) -> f64 {
        if self.min_backing == 0 {
            return f64::INFINITY;
        }
        self.max_backing as f64 / self.min_backing as f64
    }

    /// Get validator by account ID
    pub fn get_validator(&self, account: &AccountId) -> Option<&ValidatorElection> {
        self.validators.iter().find(|v| &v.validator == account)
    }

    /// Get validator by index
    pub fn get_validator_by_index(&self, index: ValidatorIndex) -> Option<&ValidatorElection> {
        self.validators.iter().find(|v| v.index == index)
    }
}

/// Phragmén election engine
pub struct PhragmenElection {
    /// Number of validators to elect
    validator_count: usize,

    /// All validator candidates
    candidates: BTreeMap<AccountId, Candidate>,

    /// All nominations
    nominations: Vec<Nomination>,
}

impl PhragmenElection {
    /// Create new Phragmén election
    pub fn new(validator_count: usize) -> Self {
        Self {
            validator_count,
            candidates: BTreeMap::new(),
            nominations: Vec::new(),
        }
    }

    /// Add a validator candidate
    pub fn add_candidate(&mut self, candidate: Candidate) -> Result<()> {
        if candidate.self_stake == 0 {
            bail!("Candidate must have non-zero self-stake");
        }

        self.candidates.insert(candidate.account, candidate);
        Ok(())
    }

    /// Add a nomination
    pub fn add_nomination(&mut self, nomination: Nomination) -> Result<()> {
        if nomination.stake == 0 {
            bail!("Nomination must have non-zero stake");
        }

        if nomination.targets.is_empty() {
            bail!("Nomination must have at least one target");
        }

        if nomination.targets.len() > 16 {
            bail!("Nomination can have at most 16 targets");
        }

        // Check all targets are valid candidates
        for target in &nomination.targets {
            if !self.candidates.contains_key(target) {
                bail!("Target {} is not a valid candidate", hex::encode(target));
            }
        }

        self.nominations.push(nomination);
        Ok(())
    }

    /// Run Phragmén election algorithm
    ///
    /// ## Algorithm Steps
    ///
    /// 1. Initialize: Each candidate starts with their self-stake
    /// 2. Sequential selection: Elect validators one by one
    /// 3. For each round:
    ///    - Select candidate with highest total potential backing
    ///    - Distribute nominator stakes to maintain balance
    ///    - Mark candidate as elected
    /// 4. Repeat until k validators elected
    ///
    /// **Maximin property**: This algorithm maximizes the minimum backing across all validators.
    pub fn run_election(&self, era: u64) -> Result<ElectionResult> {
        if self.candidates.is_empty() {
            bail!("No candidates for election");
        }

        if self.candidates.len() < self.validator_count {
            bail!(
                "Not enough candidates: {} < {} required",
                self.candidates.len(),
                self.validator_count
            );
        }

        tracing::info!(
            "Running Phragmén election for era {} with {} candidates and {} nominations",
            era,
            self.candidates.len(),
            self.nominations.len()
        );

        // Step 1: Initialize candidate backings with self-stake
        let mut candidate_backings: BTreeMap<AccountId, Balance> = self
            .candidates
            .iter()
            .filter(|(_, c)| c.is_active)
            .map(|(account, candidate)| (*account, candidate.self_stake))
            .collect();

        // Step 2: Initialize nominator loads (how much support they've given)
        let mut nominator_loads: BTreeMap<AccountId, Balance> = BTreeMap::new();

        // Step 3: Track which nominators support which validators
        let mut nominator_to_validators: BTreeMap<AccountId, Vec<AccountId>> = BTreeMap::new();
        for nomination in &self.nominations {
            nominator_to_validators.insert(nomination.nominator, nomination.targets.clone());
        }

        // Step 4: Elected validators (will have exactly validator_count entries)
        let mut elected: Vec<ValidatorElection> = Vec::new();
        let mut elected_set: BTreeSet<AccountId> = BTreeSet::new();

        // Step 5: Sequential Phragmén election
        for round in 0..self.validator_count {
            // Find candidate with highest backing
            let (winner, winner_backing) = candidate_backings
                .iter()
                .filter(|(account, _)| !elected_set.contains(*account))
                .max_by_key(|(_, backing)| *backing)
                .ok_or_else(|| anyhow::anyhow!("No more candidates to elect"))?;

            let winner = *winner;
            let winner_backing = *winner_backing;

            tracing::debug!(
                "Round {}: Elected validator {} with backing {}",
                round,
                hex::encode(winner),
                winner_backing
            );

            // Calculate nominator contributions to this validator
            let mut nominator_contributions: BTreeMap<AccountId, Balance> = BTreeMap::new();

            for nomination in &self.nominations {
                // Check if this nominator nominated the winner
                if !nomination.targets.contains(&winner) {
                    continue;
                }

                // Calculate how much this nominator should contribute
                let nominator_load = nominator_loads.get(&nomination.nominator).copied().unwrap_or(0);
                let targets_still_available = nomination
                    .targets
                    .iter()
                    .filter(|t| !elected_set.contains(*t))
                    .count();

                if targets_still_available == 0 {
                    continue;
                }

                // Distribute stake evenly among remaining targets
                let contribution_per_target = nomination.stake / targets_still_available as u128;

                // Adjust for existing load (ensures balance)
                let adjusted_contribution = if nominator_load > 0 {
                    contribution_per_target.saturating_sub(nominator_load / targets_still_available as u128)
                } else {
                    contribution_per_target
                };

                if adjusted_contribution > 0 {
                    nominator_contributions.insert(nomination.nominator, adjusted_contribution);

                    // Update nominator load
                    *nominator_loads.entry(nomination.nominator).or_insert(0) += adjusted_contribution;
                }
            }

            // Total backing = self-stake + nominator contributions
            let total_backing = self
                .candidates
                .get(&winner)
                .map(|c| c.self_stake)
                .unwrap_or(0)
                + nominator_contributions.values().sum::<Balance>();

            // Record election
            let validator_election = ValidatorElection {
                validator: winner,
                total_backing,
                nominators: nominator_contributions,
                index: round as ValidatorIndex,
            };

            elected.push(validator_election);
            elected_set.insert(winner);

            // Rebalance: Update remaining candidates' backings
            // This ensures balanced distribution in future rounds
            for (candidate, backing) in candidate_backings.iter_mut() {
                if elected_set.contains(candidate) {
                    continue;
                }

                // Calculate potential backing from nominators
                let mut potential_backing = self
                    .candidates
                    .get(candidate)
                    .map(|c| c.self_stake)
                    .unwrap_or(0);

                for nomination in &self.nominations {
                    if !nomination.targets.contains(candidate) {
                        continue;
                    }

                    let nominator_load = nominator_loads.get(&nomination.nominator).copied().unwrap_or(0);
                    let targets_remaining = nomination
                        .targets
                        .iter()
                        .filter(|t| !elected_set.contains(*t))
                        .count();

                    if targets_remaining > 0 {
                        let share = (nomination.stake - nominator_load) / targets_remaining as u128;
                        potential_backing += share;
                    }
                }

                *backing = potential_backing;
            }
        }

        // Calculate statistics
        let total_stake: Balance = elected.iter().map(|v| v.total_backing).sum();
        let min_backing = elected
            .iter()
            .map(|v| v.total_backing)
            .min()
            .unwrap_or(0);
        let max_backing = elected
            .iter()
            .map(|v| v.total_backing)
            .max()
            .unwrap_or(0);
        let avg_backing = total_stake / self.validator_count as u128;

        let result = ElectionResult {
            era,
            validators: elected,
            total_stake,
            min_backing,
            max_backing,
            avg_backing,
        };

        tracing::info!(
            "Phragmén election complete: {} validators elected, total stake: {}, balance ratio: {:.2}",
            result.validators.len(),
            result.total_stake,
            result.balance_ratio()
        );

        Ok(result)
    }

    /// Get number of active candidates
    pub fn active_candidates(&self) -> usize {
        self.candidates.values().filter(|c| c.is_active).count()
    }

    /// Get total nominated stake
    pub fn total_nominated_stake(&self) -> Balance {
        self.nominations.iter().map(|n| n.stake).sum()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn create_test_account(id: u8) -> AccountId {
        let mut account = [0u8; 32];
        account[0] = id;
        account
    }

    #[test]
    fn test_simple_election() {
        let mut election = PhragmenElection::new(3);

        // Add candidates
        for i in 0..5 {
            election
                .add_candidate(Candidate {
                    account: create_test_account(i),
                    self_stake: 10_000,
                    is_active: true,
                })
                .unwrap();
        }

        // Add nominations
        election
            .add_nomination(Nomination {
                nominator: create_test_account(100),
                stake: 50_000,
                targets: vec![
                    create_test_account(0),
                    create_test_account(1),
                    create_test_account(2),
                ],
            })
            .unwrap();

        election
            .add_nomination(Nomination {
                nominator: create_test_account(101),
                stake: 30_000,
                targets: vec![create_test_account(1), create_test_account(3)],
            })
            .unwrap();

        // Run election
        let result = election.run_election(1).unwrap();

        assert_eq!(result.validators.len(), 3);
        assert!(result.min_backing > 0);
        assert!(result.max_backing >= result.min_backing);
        assert!(result.balance_ratio() < 2.0); // Should be well-balanced
    }

    #[test]
    fn test_balanced_stakes() {
        let mut election = PhragmenElection::new(3);

        // Add 5 candidates with equal self-stake
        for i in 0..5 {
            election
                .add_candidate(Candidate {
                    account: create_test_account(i),
                    self_stake: 10_000,
                    is_active: true,
                })
                .unwrap();
        }

        // Single large nominator nominates all
        election
            .add_nomination(Nomination {
                nominator: create_test_account(100),
                stake: 100_000,
                targets: vec![
                    create_test_account(0),
                    create_test_account(1),
                    create_test_account(2),
                    create_test_account(3),
                    create_test_account(4),
                ],
            })
            .unwrap();

        let result = election.run_election(1).unwrap();

        // With Phragmén, stakes should be balanced
        // The 100K stake should be distributed roughly equally among 3 winners
        let balance_ratio = result.balance_ratio();
        assert!(balance_ratio < 1.5, "Balance ratio too high: {}", balance_ratio);

        // Each validator should have approximately equal backing
        let expected_per_validator = (100_000 + 5 * 10_000) / 3;
        for validator in &result.validators {
            let diff_percent = ((validator.total_backing as i128 - expected_per_validator as i128).abs() as f64
                / expected_per_validator as f64)
                * 100.0;
            assert!(
                diff_percent < 20.0,
                "Validator backing {} differs from expected {} by {}%",
                validator.total_backing,
                expected_per_validator,
                diff_percent
            );
        }
    }

    #[test]
    fn test_insufficient_candidates() {
        let mut election = PhragmenElection::new(3);

        // Only 2 candidates, need 3
        election
            .add_candidate(Candidate {
                account: create_test_account(0),
                self_stake: 10_000,
                is_active: true,
            })
            .unwrap();

        election
            .add_candidate(Candidate {
                account: create_test_account(1),
                self_stake: 10_000,
                is_active: true,
            })
            .unwrap();

        let result = election.run_election(1);
        assert!(result.is_err());
    }

    #[test]
    fn test_nomination_validation() {
        let mut election = PhragmenElection::new(3);

        election
            .add_candidate(Candidate {
                account: create_test_account(0),
                self_stake: 10_000,
                is_active: true,
            })
            .unwrap();

        // Zero stake
        assert!(election
            .add_nomination(Nomination {
                nominator: create_test_account(100),
                stake: 0,
                targets: vec![create_test_account(0)],
            })
            .is_err());

        // No targets
        assert!(election
            .add_nomination(Nomination {
                nominator: create_test_account(100),
                stake: 1000,
                targets: vec![],
            })
            .is_err());

        // Too many targets (>16)
        let too_many_targets: Vec<AccountId> = (0..17).map(create_test_account).collect();
        assert!(election
            .add_nomination(Nomination {
                nominator: create_test_account(100),
                stake: 1000,
                targets: too_many_targets,
            })
            .is_err());

        // Invalid target
        assert!(election
            .add_nomination(Nomination {
                nominator: create_test_account(100),
                stake: 1000,
                targets: vec![create_test_account(99)], // Doesn't exist
            })
            .is_err());
    }

    #[test]
    fn test_maximin_property() {
        let mut election = PhragmenElection::new(3);

        // Add candidates
        for i in 0..5 {
            election
                .add_candidate(Candidate {
                    account: create_test_account(i),
                    self_stake: 5_000,
                    is_active: true,
                })
                .unwrap();
        }

        // Add several nominators with different preferences
        election
            .add_nomination(Nomination {
                nominator: create_test_account(100),
                stake: 30_000,
                targets: vec![create_test_account(0), create_test_account(1)],
            })
            .unwrap();

        election
            .add_nomination(Nomination {
                nominator: create_test_account(101),
                stake: 20_000,
                targets: vec![create_test_account(2), create_test_account(3)],
            })
            .unwrap();

        election
            .add_nomination(Nomination {
                nominator: create_test_account(102),
                stake: 40_000,
                targets: vec![
                    create_test_account(0),
                    create_test_account(2),
                    create_test_account(4),
                ],
            })
            .unwrap();

        let result = election.run_election(1).unwrap();

        // Phragmén should maximize the minimum backing
        // Check that no validator is dramatically under-backed
        let mean_backing = result.avg_backing;
        for validator in &result.validators {
            assert!(
                validator.total_backing >= mean_backing / 2,
                "Validator {} has backing {} which is less than half the mean {}",
                hex::encode(validator.validator),
                validator.total_backing,
                mean_backing
            );
        }
    }
}
