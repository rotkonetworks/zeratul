//! DKG Integration with Governance
//!
//! Coordinates Golden DKG execution at epoch boundaries with validator selection from Phragmén.
//!
//! ## Flow
//!
//! 1. **Epoch Boundary**: Every 4 hours
//! 2. **Election**: Phragmén selects 15 validators based on stake
//! 3. **DKG**: Selected validators run Golden DKG protocol
//! 4. **Threshold**: t = 2f + 1 = 11 (Byzantine fault tolerance)
//! 5. **Slashing**: Non-participating validators lose stake
//!
//! ## Slashing Rules
//!
//! - **Miss DKG broadcast**: 1% stake slashed
//! - **Invalid DKG broadcast**: 5% stake slashed
//! - **Byzantine behavior**: 100% stake slashed (ejection)

use anyhow::{bail, Result};
use commonware_cryptography::bls12381::primitives::group::Scalar;
use commonware_cryptography::bls12381::PublicKey;
use rand_core::CryptoRngCore;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};
use tracing::{debug, error, info, warn};

use super::{AccountId, Balance, ElectionResult, EpochIndex, ValidatorIndex};
use crate::dkg_coordinator::{DKGCoordinator, EpochDKG};
use golden_rs::dkg::broadcast::BroadcastMsg;

/// Mapping between AccountId and BLS PublicKey
#[derive(Clone, Debug)]
pub struct ValidatorRegistry {
    /// AccountId -> BLS PublicKey
    account_to_bls: HashMap<AccountId, PublicKey>,

    /// BLS PublicKey -> AccountId
    bls_to_account: HashMap<PublicKey, AccountId>,
}

impl ValidatorRegistry {
    pub fn new() -> Self {
        Self {
            account_to_bls: HashMap::new(),
            bls_to_account: HashMap::new(),
        }
    }

    /// Register a validator's BLS public key
    pub fn register(&mut self, account: AccountId, bls_pubkey: PublicKey) {
        self.account_to_bls.insert(account, bls_pubkey.clone());
        self.bls_to_account.insert(bls_pubkey, account);
    }

    /// Get BLS public key for an account
    pub fn get_bls(&self, account: &AccountId) -> Option<PublicKey> {
        self.account_to_bls.get(account).cloned()
    }

    /// Get account for a BLS public key
    pub fn get_account(&self, bls_pubkey: &PublicKey) -> Option<AccountId> {
        self.bls_to_account.get(bls_pubkey).cloned()
    }
}

/// Slashing event for DKG non-participation
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SlashingEvent {
    /// Epoch where slashing occurred
    pub epoch: EpochIndex,

    /// Validator account that was slashed
    pub validator: AccountId,

    /// Amount slashed (in ZT base units)
    pub amount: Balance,

    /// Reason for slashing
    pub reason: SlashingReason,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum SlashingReason {
    /// Validator didn't broadcast DKG message
    MissedBroadcast,

    /// Validator broadcast invalid DKG message
    InvalidBroadcast,

    /// Byzantine behavior detected
    Byzantine,
}

impl SlashingReason {
    /// Get the slash percentage for this reason
    pub fn slash_percentage(&self) -> f64 {
        match self {
            SlashingReason::MissedBroadcast => 0.01,  // 1%
            SlashingReason::InvalidBroadcast => 0.05, // 5%
            SlashingReason::Byzantine => 1.0,         // 100%
        }
    }
}

/// DKG + Governance integration manager
pub struct DKGGovernanceManager {
    /// DKG coordinator
    dkg: DKGCoordinator,

    /// Validator registry (AccountId <-> BLS PublicKey)
    registry: Arc<Mutex<ValidatorRegistry>>,

    /// Current epoch
    current_epoch: EpochIndex,

    /// Election results per epoch
    elections: HashMap<EpochIndex, ElectionResult>,

    /// Slashing events
    slashing_events: Vec<SlashingEvent>,

    /// DKG timeout (in seconds)
    dkg_timeout: u64,
}

impl DKGGovernanceManager {
    /// Create a new DKG governance manager
    pub fn new<R: CryptoRngCore>(
        rng: &mut R,
        our_pubkey: PublicKey,
        beta: Scalar,
        dkg_timeout: u64,
    ) -> Self {
        Self {
            dkg: DKGCoordinator::new(rng, our_pubkey, beta),
            registry: Arc::new(Mutex::new(ValidatorRegistry::new())),
            current_epoch: 0,
            elections: HashMap::new(),
            slashing_events: Vec::new(),
            dkg_timeout,
        }
    }

    /// Register a validator's BLS public key
    pub fn register_validator(&self, account: AccountId, bls_pubkey: PublicKey) {
        let mut registry = self.registry.lock().unwrap();
        registry.register(account, bls_pubkey.clone());
        info!(
            account = ?account,
            bls_pubkey = ?bls_pubkey,
            "Registered validator BLS key"
        );
    }

    /// Start a new epoch with validator election results
    pub fn start_epoch<R: CryptoRngCore>(
        &mut self,
        rng: &mut R,
        epoch: EpochIndex,
        election_result: ElectionResult,
    ) -> Result<Option<BroadcastMsg>> {
        info!(
            epoch,
            num_validators = election_result.validators.len(),
            "Starting new epoch with DKG"
        );

        self.current_epoch = epoch;
        self.elections.insert(epoch, election_result.clone());

        // Start DKG for this epoch
        let bmsg = self.dkg.start_epoch(rng, epoch, election_result)?;

        Ok(bmsg)
    }

    /// Process incoming DKG broadcast
    pub fn on_dkg_broadcast(
        &mut self,
        epoch: EpochIndex,
        sender: PublicKey,
        bmsg: BroadcastMsg,
    ) -> Result<()> {
        self.dkg.on_broadcast(epoch, sender, bmsg)
    }

    /// Check DKG completion and slash non-participants
    ///
    /// Should be called after DKG timeout expires
    pub fn finalize_dkg(&mut self, epoch: EpochIndex) -> Result<Vec<SlashingEvent>> {
        let missing = self.dkg.missing_validators(epoch);

        if missing.is_empty() {
            info!(epoch, "DKG completed successfully, all validators participated");
            return Ok(Vec::new());
        }

        // Get election result to calculate slashing amounts
        let election = self.elections.get(&epoch)
            .ok_or_else(|| anyhow::anyhow!("No election result for epoch {}", epoch))?;

        let registry = self.registry.lock().unwrap();
        let mut events = Vec::new();

        for bls_pubkey in missing {
            // Map BLS key back to AccountId
            let Some(account) = registry.get_account(&bls_pubkey) else {
                warn!(bls_pubkey = ?bls_pubkey, "Missing validator not in registry");
                continue;
            };

            // Find validator's stake
            let validator_stake = election
                .validators
                .iter()
                .find(|v| v.validator == account)
                .map(|v| v.total_backing)
                .unwrap_or(0);

            // Calculate slash amount (1% of stake)
            let slash_amount = (validator_stake as f64 * 0.01) as Balance;

            let event = SlashingEvent {
                epoch,
                validator: account,
                amount: slash_amount,
                reason: SlashingReason::MissedBroadcast,
            };

            warn!(
                epoch,
                validator = ?account,
                slash_amount,
                "Slashing validator for missing DKG broadcast"
            );

            events.push(event.clone());
            self.slashing_events.push(event);
        }

        Ok(events)
    }

    /// Get group public key for an epoch
    pub fn group_pubkey(&self, epoch: EpochIndex) -> Option<PublicKey> {
        self.dkg.group_pubkey(epoch)
    }

    /// Get our secret share for an epoch
    pub fn secret_share(&self, epoch: EpochIndex) -> Option<Scalar> {
        self.dkg.secret_share(epoch)
    }

    /// Check if DKG is complete for an epoch
    pub fn is_dkg_complete(&self, epoch: EpochIndex) -> bool {
        self.dkg.is_complete(epoch)
    }

    /// Get all slashing events
    pub fn slashing_events(&self) -> &[SlashingEvent] {
        &self.slashing_events
    }

    /// Get slashing events for a specific epoch
    pub fn epoch_slashing_events(&self, epoch: EpochIndex) -> Vec<&SlashingEvent> {
        self.slashing_events
            .iter()
            .filter(|e| e.epoch == epoch)
            .collect()
    }

    /// Partial sign a message using our secret share
    pub fn partial_sign(
        &self,
        epoch: EpochIndex,
        message: &[u8],
    ) -> Option<commonware_cryptography::bls12381::primitives::poly::Eval<commonware_cryptography::bls12381::primitives::group::G2>> {
        self.dkg.partial_sign(epoch, message)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use commonware_cryptography::bls12381::primitives::group::G1;
    use rand::thread_rng;

    #[test]
    fn test_validator_registry() {
        let mut registry = ValidatorRegistry::new();

        let account = [1u8; 32];
        let bls_key = PublicKey::from(G1::one());

        registry.register(account, bls_key);

        assert_eq!(registry.get_bls(&account), Some(bls_key));
        assert_eq!(registry.get_account(&bls_key), Some(account));
    }

    #[test]
    fn test_slashing_percentages() {
        assert_eq!(SlashingReason::MissedBroadcast.slash_percentage(), 0.01);
        assert_eq!(SlashingReason::InvalidBroadcast.slash_percentage(), 0.05);
        assert_eq!(SlashingReason::Byzantine.slash_percentage(), 1.0);
    }

    #[test]
    fn test_dkg_governance_manager() {
        let mut rng = thread_rng();
        let beta = Scalar::one();
        let our_key = PublicKey::from(G1::one());

        let manager = DKGGovernanceManager::new(
            &mut rng,
            our_key,
            beta,
            60, // 60 second timeout
        );

        // Register a validator
        let account = [2u8; 32];
        manager.register_validator(account, our_key);

        let registry = manager.registry.lock().unwrap();
        assert_eq!(registry.get_bls(&account), Some(our_key));
    }
}
