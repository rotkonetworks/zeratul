//! Zeratul State - Service Registry and Accumulation State
//!
//! Zeratul is a minimal verification/accumulation layer inspired by JAM.
//!
//! Key differences from traditional blockchains:
//! - No accounts, balances, or transfers (that's for services to handle)
//! - Only tracks registered services and their accumulated state roots
//! - Workload-agnostic: doesn't interpret what services compute
//!
//! Browser clients do "Refine" (computation) and submit work results.
//! Zeratul does "Accumulate" (verify proofs, update state roots).

use crate::types::*;
use sha2::{Sha256, Digest};
use std::collections::HashMap;

/// Zeratul chain state - minimal verification layer
#[derive(Clone, Debug)]
pub struct State {
    /// Current block height
    height: Height,
    /// Global state root (commitment to all service states)
    state_root: Hash,
    /// Registered services
    services: HashMap<ServiceId, Service>,
    /// Per-service accumulated state
    service_states: HashMap<ServiceId, ServiceState>,
    /// Active validators (for consensus)
    validators: Vec<Validator>,
    /// Pending work results (waiting for dependencies)
    pending_queue: Vec<PendingWork>,
}

/// Per-service accumulated state
#[derive(Clone, Debug, Default)]
pub struct ServiceState {
    /// Merkle root of service's accumulated data
    pub state_root: Hash,
    /// Total work results accumulated
    pub accumulated_count: u64,
    /// Last block this service was updated
    pub last_update: Height,
}

/// Work waiting for dependencies (like JAM's prerequisite system)
#[derive(Clone, Debug)]
pub struct PendingWork {
    pub result: WorkResult,
    /// Package hashes this depends on (prerequisites)
    pub prerequisites: Vec<Hash>,
    pub submitted_at: Height,
}

impl State {
    /// Create genesis state with validators
    pub fn genesis(validators: Vec<Validator>) -> Self {
        // Register service 0 as "null" service for testing
        let mut services = HashMap::new();
        services.insert(0, Service {
            id: 0,
            verifier_hash: ZERO_HASH,
            state_root: ZERO_HASH,
            active: true,
        });

        Self {
            height: 0,
            state_root: ZERO_HASH,
            services,
            service_states: HashMap::new(),
            validators,
            pending_queue: Vec::new(),
        }
    }

    /// Current height
    pub fn height(&self) -> Height {
        self.height
    }

    /// Current state root
    pub fn state_root(&self) -> Hash {
        self.state_root
    }

    /// Number of active validators
    pub fn active_validator_count(&self) -> usize {
        self.validators.iter().filter(|v| v.active).count()
    }

    /// Get validators
    pub fn validators(&self) -> &[Validator] {
        &self.validators
    }

    /// Register a new service
    ///
    /// Services define their own verification logic - Zeratul doesn't care
    /// what computation they do, just that proofs verify.
    pub fn register_service(&mut self, service: Service) -> Result<(), StateError> {
        if self.services.contains_key(&service.id) {
            return Err(StateError::ServiceAlreadyExists(service.id));
        }
        self.services.insert(service.id, service);
        Ok(())
    }

    /// Get service by ID
    pub fn get_service(&self, id: ServiceId) -> Option<&Service> {
        self.services.get(&id)
    }

    /// Check if service exists and is active
    pub fn is_service_active(&self, id: ServiceId) -> bool {
        self.services.get(&id).map_or(false, |s| s.active)
    }

    /// Get service state
    pub fn get_service_state(&self, id: ServiceId) -> Option<&ServiceState> {
        self.service_states.get(&id)
    }

    /// Accumulate a verified work result into service state
    ///
    /// This is the "Accumulate" phase from JAM - but simplified.
    /// In JAM, accumulate runs service-specific code.
    /// In Zeratul MVP, we just update the state root.
    pub fn accumulate(&mut self, result: &WorkResult) -> Result<(), StateError> {
        if !self.is_service_active(result.service) {
            return Err(StateError::ServiceNotFound(result.service));
        }

        let service_state = self.service_states
            .entry(result.service)
            .or_default();

        // Update state root by hashing in the new result
        // This creates a chain of accumulated results
        let mut hasher = Sha256::new();
        hasher.update(&service_state.state_root);
        hasher.update(&result.hash());
        service_state.state_root = hasher.finalize().into();
        service_state.accumulated_count += 1;
        service_state.last_update = self.height;

        Ok(())
    }

    /// Apply a block to state
    pub fn apply_block(&mut self, block: &Block) -> Result<(), StateError> {
        if block.header.height != self.height + 1 {
            return Err(StateError::InvalidHeight {
                expected: self.height + 1,
                got: block.header.height,
            });
        }

        // Accumulate all work results
        for result in &block.work_results {
            self.accumulate(result)?;
        }

        // Update global state
        self.height = block.header.height;
        self.state_root = self.compute_state_root();

        Ok(())
    }

    /// Compute global state root from all service states
    pub fn compute_state_root(&self) -> Hash {
        if self.service_states.is_empty() {
            return ZERO_HASH;
        }

        let mut hasher = Sha256::new();

        // Sort by service ID for determinism
        let mut service_ids: Vec<_> = self.service_states.keys().collect();
        service_ids.sort();

        for id in service_ids {
            let state = &self.service_states[id];
            hasher.update(&id.to_le_bytes());
            hasher.update(&state.state_root);
            hasher.update(&state.accumulated_count.to_le_bytes());
        }

        hasher.finalize().into()
    }

    /// Add work to pending queue (has unmet dependencies)
    pub fn queue_pending(&mut self, result: WorkResult, prerequisites: Vec<Hash>) {
        self.pending_queue.push(PendingWork {
            result,
            prerequisites,
            submitted_at: self.height,
        });
    }

    /// Get pending work ready to process (all dependencies met)
    pub fn drain_ready_work(&mut self, completed: &[Hash]) -> Vec<WorkResult> {
        let (ready, still_pending): (Vec<_>, Vec<_>) = self.pending_queue
            .drain(..)
            .partition(|pw| pw.prerequisites.iter().all(|p| completed.contains(p)));

        self.pending_queue = still_pending;
        ready.into_iter().map(|pw| pw.result).collect()
    }

    /// Advance height (for empty blocks)
    pub fn advance_height(&mut self) {
        self.height += 1;
    }
}

/// State errors
#[derive(Debug, Clone)]
pub enum StateError {
    ServiceNotFound(ServiceId),
    ServiceAlreadyExists(ServiceId),
    InvalidHeight { expected: Height, got: Height },
    InvalidStateRoot,
}

impl std::fmt::Display for StateError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            StateError::ServiceNotFound(id) => write!(f, "Service {} not found", id),
            StateError::ServiceAlreadyExists(id) => write!(f, "Service {} already exists", id),
            StateError::InvalidHeight { expected, got } => {
                write!(f, "Invalid height: expected {}, got {}", expected, got)
            }
            StateError::InvalidStateRoot => write!(f, "Invalid state root"),
        }
    }
}

impl std::error::Error for StateError {}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_validators() -> Vec<Validator> {
        vec![
            Validator { pubkey: [1u8; 32], stake: 1000, active: true },
            Validator { pubkey: [2u8; 32], stake: 1000, active: true },
            Validator { pubkey: [3u8; 32], stake: 1000, active: true },
        ]
    }

    #[test]
    fn test_genesis_state() {
        let state = State::genesis(test_validators());
        assert_eq!(state.height(), 0);
        assert_eq!(state.active_validator_count(), 3);
        assert!(state.is_service_active(0)); // Null service
    }

    #[test]
    fn test_register_service() {
        let mut state = State::genesis(vec![]);

        let service = Service {
            id: 1,
            verifier_hash: [0xAB; 32],
            state_root: ZERO_HASH,
            active: true,
        };

        assert!(state.register_service(service.clone()).is_ok());
        assert!(state.is_service_active(1));
        assert!(state.register_service(service).is_err()); // Duplicate
    }

    #[test]
    fn test_accumulate() {
        let mut state = State::genesis(vec![]);

        let result = WorkResult {
            package_hash: [1u8; 32],
            service: 0,
            output_hash: [2u8; 32],
            gas_used: 100,
            success: true,
        };

        assert!(state.accumulate(&result).is_ok());

        let ss = state.get_service_state(0).unwrap();
        assert_eq!(ss.accumulated_count, 1);
        assert_ne!(ss.state_root, ZERO_HASH);
    }

    #[test]
    fn test_accumulate_chain() {
        let mut state = State::genesis(vec![]);

        // Accumulate multiple results - state root should change each time
        let mut prev_root = ZERO_HASH;
        for i in 0..3 {
            let result = WorkResult {
                package_hash: [i as u8; 32],
                service: 0,
                output_hash: [(i + 10) as u8; 32],
                gas_used: 100,
                success: true,
            };
            state.accumulate(&result).unwrap();

            let ss = state.get_service_state(0).unwrap();
            assert_ne!(ss.state_root, prev_root);
            prev_root = ss.state_root;
        }

        let ss = state.get_service_state(0).unwrap();
        assert_eq!(ss.accumulated_count, 3);
    }
}
