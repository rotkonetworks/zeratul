//! Validator Candidate Registry and Selection
//!
//! Manages the pool of validator candidates and coordinates elections.

use super::phragmen::{Candidate, ElectionResult, Nomination, PhragmenElection};
use super::{AccountId, Balance, EraIndex, ValidatorIndex};
use anyhow::{bail, Result};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::net::SocketAddr;

/// Validator candidate registration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ValidatorCandidate {
    /// Validator account
    pub account: AccountId,

    /// Ed25519 consensus public key
    pub consensus_key: [u8; 32],

    /// Decaf377 FROST public key
    pub frost_key: [u8; 32],

    /// Network endpoint for P2P
    pub endpoint: SocketAddr,

    /// Self-stake amount
    pub self_stake: Balance,

    /// Commission rate (0-100, e.g., 10 = 10%)
    pub commission: u8,

    /// On-chain identity (optional)
    pub identity: Option<ValidatorIdentity>,

    /// Registration era
    pub registered_era: EraIndex,

    /// Whether candidate is active
    pub is_active: bool,
}

impl ValidatorCandidate {
    /// Validate candidate registration
    pub fn validate(&self, min_stake: Balance) -> Result<()> {
        if self.self_stake < min_stake {
            bail!(
                "Insufficient self-stake: {} < {} required",
                self.self_stake,
                min_stake
            );
        }

        if self.commission > 100 {
            bail!("Invalid commission: {} > 100%", self.commission);
        }

        Ok(())
    }

    /// Convert to Phragmén candidate
    pub fn to_phragmen_candidate(&self) -> Candidate {
        Candidate {
            account: self.account,
            self_stake: self.self_stake,
            is_active: self.is_active,
        }
    }
}

/// Validator on-chain identity
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ValidatorIdentity {
    /// Display name
    pub name: String,

    /// Website URL
    pub website: Option<String>,

    /// Twitter handle
    pub twitter: Option<String>,

    /// Email
    pub email: Option<String>,
}

/// Active validator set for current era
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ValidatorSet {
    /// Current era
    pub era: EraIndex,

    /// Elected validators (sorted by index)
    pub validators: Vec<ActiveValidator>,

    /// Total stake backing this set
    pub total_stake: Balance,

    /// FROST public key for this set (from DKG)
    pub frost_public_key: Option<[u8; 32]>,
}

impl ValidatorSet {
    /// Get validator by account
    pub fn get_by_account(&self, account: &AccountId) -> Option<&ActiveValidator> {
        self.validators.iter().find(|v| &v.account == account)
    }

    /// Get validator by index
    pub fn get_by_index(&self, index: ValidatorIndex) -> Option<&ActiveValidator> {
        self.validators.iter().find(|v| v.index == index)
    }

    /// Check if account is in validator set
    pub fn contains(&self, account: &AccountId) -> bool {
        self.validators.iter().any(|v| &v.account == account)
    }

    /// Get total number of validators
    pub fn len(&self) -> usize {
        self.validators.len()
    }

    /// Check if validator set is empty
    pub fn is_empty(&self) -> bool {
        self.validators.is_empty()
    }

    /// Get minimum backing
    pub fn min_backing(&self) -> Balance {
        self.validators
            .iter()
            .map(|v| v.total_backing)
            .min()
            .unwrap_or(0)
    }

    /// Get average backing
    pub fn avg_backing(&self) -> Balance {
        if self.validators.is_empty() {
            return 0;
        }
        self.total_stake / self.validators.len() as u128
    }
}

/// Active validator (elected via Phragmén)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ActiveValidator {
    /// Validator account
    pub account: AccountId,

    /// Validator index (0-14)
    pub index: ValidatorIndex,

    /// Consensus public key
    pub consensus_key: [u8; 32],

    /// FROST public key
    pub frost_key: [u8; 32],

    /// Network endpoint
    pub endpoint: SocketAddr,

    /// Total backing (self + nominators)
    pub total_backing: Balance,

    /// Self-stake amount
    pub self_stake: Balance,

    /// Commission rate
    pub commission: u8,

    /// Nominators backing this validator
    pub nominators: BTreeMap<AccountId, Balance>,
}

/// Candidate registry
pub struct CandidateRegistry {
    /// All registered candidates
    candidates: BTreeMap<AccountId, ValidatorCandidate>,

    /// Active nominations
    nominations: BTreeMap<AccountId, Nomination>,

    /// Current validator set
    current_set: Option<ValidatorSet>,

    /// Election history
    election_history: Vec<ElectionResult>,

    /// Configuration
    config: RegistryConfig,
}

#[derive(Debug, Clone)]
pub struct RegistryConfig {
    /// Number of validators to elect
    pub validator_count: usize,

    /// Minimum validator self-stake
    pub min_validator_stake: Balance,

    /// Minimum nominator stake
    pub min_nominator_stake: Balance,

    /// Maximum nominations per nominator
    pub max_nominations: usize,
}

impl Default for RegistryConfig {
    fn default() -> Self {
        Self {
            validator_count: 15,
            min_validator_stake: 10_000 * 10u128.pow(18),
            min_nominator_stake: 100 * 10u128.pow(18),
            max_nominations: 16,
        }
    }
}

impl CandidateRegistry {
    /// Create new registry
    pub fn new(config: RegistryConfig) -> Self {
        Self {
            candidates: BTreeMap::new(),
            nominations: BTreeMap::new(),
            current_set: None,
            election_history: Vec::new(),
            config,
        }
    }

    /// Register a new validator candidate
    pub fn register_candidate(&mut self, candidate: ValidatorCandidate) -> Result<()> {
        // Validate candidate
        candidate.validate(self.config.min_validator_stake)?;

        // Check not already registered
        if self.candidates.contains_key(&candidate.account) {
            bail!("Candidate already registered");
        }

        tracing::info!(
            "Registered validator candidate {} with self-stake {}",
            hex::encode(candidate.account),
            candidate.self_stake
        );

        self.candidates.insert(candidate.account, candidate);
        Ok(())
    }

    /// Deactivate a candidate
    pub fn deactivate_candidate(&mut self, account: &AccountId) -> Result<()> {
        let candidate = self
            .candidates
            .get_mut(account)
            .ok_or_else(|| anyhow::anyhow!("Candidate not found"))?;

        candidate.is_active = false;

        tracing::info!("Deactivated validator candidate {}", hex::encode(account));
        Ok(())
    }

    /// Reactivate a candidate
    pub fn reactivate_candidate(&mut self, account: &AccountId) -> Result<()> {
        let candidate = self
            .candidates
            .get_mut(account)
            .ok_or_else(|| anyhow::anyhow!("Candidate not found"))?;

        candidate.is_active = true;

        tracing::info!("Reactivated validator candidate {}", hex::encode(account));
        Ok(())
    }

    /// Submit a nomination
    pub fn nominate(
        &mut self,
        nominator: AccountId,
        stake: Balance,
        targets: Vec<AccountId>,
    ) -> Result<()> {
        // Validate stake
        if stake < self.config.min_nominator_stake {
            bail!(
                "Insufficient stake: {} < {} required",
                stake,
                self.config.min_nominator_stake
            );
        }

        // Validate target count
        if targets.is_empty() {
            bail!("Must nominate at least one validator");
        }

        if targets.len() > self.config.max_nominations {
            bail!(
                "Too many nominations: {} > {} allowed",
                targets.len(),
                self.config.max_nominations
            );
        }

        // Validate all targets are registered
        for target in &targets {
            if !self.candidates.contains_key(target) {
                bail!("Target {} is not a registered candidate", hex::encode(target));
            }
        }

        let nomination = Nomination {
            nominator,
            stake,
            targets,
        };

        tracing::info!(
            "Nominator {} staked {} for {} validators",
            hex::encode(nominator),
            stake,
            nomination.targets.len()
        );

        self.nominations.insert(nominator, nomination);
        Ok(())
    }

    /// Update an existing nomination
    pub fn update_nomination(&mut self, nominator: &AccountId, targets: Vec<AccountId>) -> Result<()> {
        let nomination = self
            .nominations
            .get_mut(nominator)
            .ok_or_else(|| anyhow::anyhow!("Nominator not found"))?;

        // Validate new targets
        if targets.is_empty() {
            bail!("Must nominate at least one validator");
        }

        if targets.len() > self.config.max_nominations {
            bail!("Too many nominations: {} > {} allowed", targets.len(), self.config.max_nominations);
        }

        for target in &targets {
            if !self.candidates.contains_key(target) {
                bail!("Target {} is not a registered candidate", hex::encode(target));
            }
        }

        nomination.targets = targets;

        tracing::info!("Updated nominations for {}", hex::encode(nominator));
        Ok(())
    }

    /// Withdraw nomination
    pub fn withdraw_nomination(&mut self, nominator: &AccountId) -> Result<Balance> {
        let nomination = self
            .nominations
            .remove(nominator)
            .ok_or_else(|| anyhow::anyhow!("Nominator not found"))?;

        tracing::info!(
            "Nominator {} withdrew stake {}",
            hex::encode(nominator),
            nomination.stake
        );

        Ok(nomination.stake)
    }

    /// Run Phragmén election for new era
    pub fn run_election(&mut self, era: EraIndex) -> Result<ValidatorSet> {
        tracing::info!(
            "Running Phragmén election for era {} with {} candidates and {} nominators",
            era,
            self.candidates.len(),
            self.nominations.len()
        );

        // Create Phragmén election
        let mut election = PhragmenElection::new(self.config.validator_count);

        // Add all active candidates
        for candidate in self.candidates.values() {
            if candidate.is_active {
                election.add_candidate(candidate.to_phragmen_candidate())?;
            }
        }

        // Add all nominations
        for nomination in self.nominations.values() {
            election.add_nomination(nomination.clone())?;
        }

        // Run election
        let result = election.run_election(era)?;

        // Convert to validator set
        let mut validators = Vec::new();
        for elected in &result.validators {
            let candidate = self
                .candidates
                .get(&elected.validator)
                .ok_or_else(|| anyhow::anyhow!("Elected validator not in registry"))?;

            validators.push(ActiveValidator {
                account: elected.validator,
                index: elected.index,
                consensus_key: candidate.consensus_key,
                frost_key: candidate.frost_key,
                endpoint: candidate.endpoint,
                total_backing: elected.total_backing,
                self_stake: candidate.self_stake,
                commission: candidate.commission,
                nominators: elected.nominators.clone(),
            });
        }

        // Sort by index
        validators.sort_by_key(|v| v.index);

        let validator_set = ValidatorSet {
            era,
            validators,
            total_stake: result.total_stake,
            frost_public_key: None, // Will be set after DKG
        };

        // Store election result
        self.election_history.push(result);
        self.current_set = Some(validator_set.clone());

        tracing::info!(
            "Elected {} validators for era {} with total stake {}",
            validator_set.validators.len(),
            era,
            validator_set.total_stake
        );

        Ok(validator_set)
    }

    /// Get current validator set
    pub fn current_validator_set(&self) -> Option<&ValidatorSet> {
        self.current_set.as_ref()
    }

    /// Get candidate by account
    pub fn get_candidate(&self, account: &AccountId) -> Option<&ValidatorCandidate> {
        self.candidates.get(account)
    }

    /// Get nomination by nominator
    pub fn get_nomination(&self, nominator: &AccountId) -> Option<&Nomination> {
        self.nominations.get(nominator)
    }

    /// Get all active candidates
    pub fn active_candidates(&self) -> Vec<&ValidatorCandidate> {
        self.candidates
            .values()
            .filter(|c| c.is_active)
            .collect()
    }

    /// Get total number of candidates
    pub fn candidate_count(&self) -> usize {
        self.candidates.len()
    }

    /// Get total number of nominators
    pub fn nominator_count(&self) -> usize {
        self.nominations.len()
    }

    /// Get total staked amount
    pub fn total_staked(&self) -> Balance {
        let candidate_stake: Balance = self.candidates.values().map(|c| c.self_stake).sum();
        let nominator_stake: Balance = self.nominations.values().map(|n| n.stake).sum();
        candidate_stake + nominator_stake
    }

    /// Get election history
    pub fn election_history(&self) -> &[ElectionResult] {
        &self.election_history
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

    fn create_test_endpoint() -> SocketAddr {
        "127.0.0.1:8000".parse().unwrap()
    }

    #[test]
    fn test_candidate_registration() {
        let mut registry = CandidateRegistry::new(RegistryConfig::default());

        let candidate = ValidatorCandidate {
            account: create_test_account(1),
            consensus_key: [1u8; 32],
            frost_key: [1u8; 32],
            endpoint: create_test_endpoint(),
            self_stake: 10_000 * 10u128.pow(18),
            commission: 10,
            identity: None,
            registered_era: 1,
            is_active: true,
        };

        assert!(registry.register_candidate(candidate).is_ok());
        assert_eq!(registry.candidate_count(), 1);
    }

    #[test]
    fn test_nomination() {
        let mut registry = CandidateRegistry::new(RegistryConfig::default());

        // Register candidate
        registry
            .register_candidate(ValidatorCandidate {
                account: create_test_account(1),
                consensus_key: [1u8; 32],
                frost_key: [1u8; 32],
                endpoint: create_test_endpoint(),
                self_stake: 10_000 * 10u128.pow(18),
                commission: 10,
                identity: None,
                registered_era: 1,
                is_active: true,
            })
            .unwrap();

        // Nominate
        let result = registry.nominate(
            create_test_account(100),
            1000 * 10u128.pow(18),
            vec![create_test_account(1)],
        );

        assert!(result.is_ok());
        assert_eq!(registry.nominator_count(), 1);
    }

    #[test]
    fn test_election() {
        let mut registry = CandidateRegistry::new(RegistryConfig {
            validator_count: 3,
            ..Default::default()
        });

        // Register 5 candidates
        for i in 1..=5 {
            registry
                .register_candidate(ValidatorCandidate {
                    account: create_test_account(i),
                    consensus_key: [i; 32],
                    frost_key: [i; 32],
                    endpoint: create_test_endpoint(),
                    self_stake: 10_000 * 10u128.pow(18),
                    commission: 10,
                    identity: None,
                    registered_era: 1,
                    is_active: true,
                })
                .unwrap();
        }

        // Add nominations
        registry
            .nominate(
                create_test_account(100),
                50_000 * 10u128.pow(18),
                vec![
                    create_test_account(1),
                    create_test_account(2),
                    create_test_account(3),
                ],
            )
            .unwrap();

        // Run election
        let validator_set = registry.run_election(1).unwrap();

        assert_eq!(validator_set.len(), 3);
        assert!(validator_set.total_stake > 0);
        assert!(validator_set.min_backing() > 0);
    }
}
