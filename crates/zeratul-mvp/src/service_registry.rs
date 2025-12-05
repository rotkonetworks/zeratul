//! Service Registry - Maps ServiceId to program commitments
//!
//! Services are PolkaVM programs that define computation logic.
//! The registry tracks which programs are registered and their metadata.

use crate::types::*;
use std::collections::HashMap;
use serde::{Serialize, Deserialize};

/// Service metadata
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServiceMetadata {
    /// Unique service identifier
    pub id: ServiceId,
    /// Hash of the PolkaVM program blob
    pub program_hash: Hash,
    /// Human-readable name
    pub name: String,
    /// Service version
    pub version: u32,
    /// Minimum gas required per invocation
    pub min_gas: Gas,
    /// Maximum gas allowed per invocation
    pub max_gas: Gas,
    /// Block when service was registered
    pub registered_at: Height,
    /// Owner who can update the service
    pub owner: PublicKey,
    /// Whether service is active
    pub active: bool,
}

/// Service registry
#[derive(Debug, Clone, Default)]
pub struct ServiceRegistry {
    /// Registered services by ID
    services: HashMap<ServiceId, ServiceMetadata>,
    /// Lookup by program hash
    by_program_hash: HashMap<Hash, ServiceId>,
    /// Next available service ID
    next_id: ServiceId,
}

impl ServiceRegistry {
    /// Create new empty registry
    pub fn new() -> Self {
        let mut registry = Self {
            services: HashMap::new(),
            by_program_hash: HashMap::new(),
            next_id: 1, // 0 is reserved for null/test service
        };

        // Register null service (for testing)
        registry.services.insert(0, ServiceMetadata {
            id: 0,
            program_hash: ZERO_HASH,
            name: "null".to_string(),
            version: 1,
            min_gas: 0,
            max_gas: u64::MAX,
            registered_at: 0,
            owner: [0u8; 32],
            active: true,
        });

        // Register validator service (reserved, controls validator set)
        // This will be implemented when epoch-based validator changes are added

        registry
    }

    /// Register a new service
    pub fn register(
        &mut self,
        program_hash: Hash,
        name: String,
        min_gas: Gas,
        max_gas: Gas,
        owner: PublicKey,
        current_height: Height,
    ) -> Result<ServiceId, RegistryError> {
        // Check program hash not already registered
        if self.by_program_hash.contains_key(&program_hash) {
            return Err(RegistryError::ProgramAlreadyRegistered);
        }

        let id = self.next_id;
        self.next_id += 1;

        let metadata = ServiceMetadata {
            id,
            program_hash,
            name,
            version: 1,
            min_gas,
            max_gas,
            registered_at: current_height,
            owner,
            active: true,
        };

        self.services.insert(id, metadata);
        self.by_program_hash.insert(program_hash, id);

        Ok(id)
    }

    /// Get service by ID
    pub fn get(&self, id: ServiceId) -> Option<&ServiceMetadata> {
        self.services.get(&id)
    }

    /// Get service by program hash
    pub fn get_by_program_hash(&self, hash: &Hash) -> Option<&ServiceMetadata> {
        self.by_program_hash.get(hash).and_then(|id| self.services.get(id))
    }

    /// Check if service is active
    pub fn is_active(&self, id: ServiceId) -> bool {
        self.services.get(&id).map(|s| s.active).unwrap_or(false)
    }

    /// Get program hash for service
    pub fn program_hash(&self, id: ServiceId) -> Option<Hash> {
        self.services.get(&id).map(|s| s.program_hash)
    }

    /// Deactivate a service (owner only)
    pub fn deactivate(&mut self, id: ServiceId, caller: &PublicKey) -> Result<(), RegistryError> {
        let service = self.services.get_mut(&id).ok_or(RegistryError::ServiceNotFound)?;

        if service.owner != *caller {
            return Err(RegistryError::NotOwner);
        }

        service.active = false;
        Ok(())
    }

    /// Update service program (owner only, creates new version)
    pub fn update_program(
        &mut self,
        id: ServiceId,
        new_program_hash: Hash,
        caller: &PublicKey,
    ) -> Result<(), RegistryError> {
        // Check new program hash not already registered
        if self.by_program_hash.contains_key(&new_program_hash) {
            return Err(RegistryError::ProgramAlreadyRegistered);
        }

        let service = self.services.get_mut(&id).ok_or(RegistryError::ServiceNotFound)?;

        if service.owner != *caller {
            return Err(RegistryError::NotOwner);
        }

        // Remove old program hash mapping
        self.by_program_hash.remove(&service.program_hash);

        // Update service
        service.program_hash = new_program_hash;
        service.version += 1;

        // Add new program hash mapping
        self.by_program_hash.insert(new_program_hash, id);

        Ok(())
    }

    /// List all active services
    pub fn list_active(&self) -> Vec<&ServiceMetadata> {
        self.services.values().filter(|s| s.active).collect()
    }

    /// Total registered services
    pub fn count(&self) -> usize {
        self.services.len()
    }
}

/// Registry errors
#[derive(Debug, Clone)]
pub enum RegistryError {
    ServiceNotFound,
    ProgramAlreadyRegistered,
    NotOwner,
    InvalidProgramHash,
}

impl std::fmt::Display for RegistryError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            RegistryError::ServiceNotFound => write!(f, "Service not found"),
            RegistryError::ProgramAlreadyRegistered => write!(f, "Program already registered"),
            RegistryError::NotOwner => write!(f, "Not the service owner"),
            RegistryError::InvalidProgramHash => write!(f, "Invalid program hash"),
        }
    }
}

impl std::error::Error for RegistryError {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_register_service() {
        let mut registry = ServiceRegistry::new();

        let program_hash = [1u8; 32];
        let owner = [2u8; 32];

        let id = registry.register(
            program_hash,
            "test-service".to_string(),
            1000,
            100_000,
            owner,
            1,
        ).unwrap();

        assert_eq!(id, 1); // 0 is null service
        assert!(registry.is_active(id));
        assert_eq!(registry.program_hash(id), Some(program_hash));
    }

    #[test]
    fn test_duplicate_program_rejected() {
        let mut registry = ServiceRegistry::new();

        let program_hash = [1u8; 32];
        let owner = [2u8; 32];

        registry.register(program_hash, "first".to_string(), 1000, 100_000, owner, 1).unwrap();

        // Same program hash should fail
        let result = registry.register(program_hash, "second".to_string(), 1000, 100_000, owner, 2);
        assert!(matches!(result, Err(RegistryError::ProgramAlreadyRegistered)));
    }

    #[test]
    fn test_deactivate_service() {
        let mut registry = ServiceRegistry::new();

        let program_hash = [1u8; 32];
        let owner = [2u8; 32];
        let other = [3u8; 32];

        let id = registry.register(program_hash, "test".to_string(), 1000, 100_000, owner, 1).unwrap();

        // Non-owner cannot deactivate
        assert!(registry.deactivate(id, &other).is_err());

        // Owner can deactivate
        registry.deactivate(id, &owner).unwrap();
        assert!(!registry.is_active(id));
    }

    #[test]
    fn test_null_service_exists() {
        let registry = ServiceRegistry::new();
        assert!(registry.is_active(0));
        assert_eq!(registry.get(0).unwrap().name, "null");
    }
}
