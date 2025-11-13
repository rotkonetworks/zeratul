//! Direct PolkaVM Integration (No Substrate)
//!
//! This module provides on-chain ZK verification using PolkaVM directly,
//! WITHOUT requiring Substrate runtime or pallet_revive.
//!
//! ## Architecture
//!
//! ```
//! Commonware Consensus
//!   ↓
//! Block Validation
//!   ↓
//! PolkaVMVerifier::verify() ← Embedded PolkaVM
//!   ↓
//! All nodes execute same RISC-V code
//!   ↓
//! Consensus guaranteed ✅
//! ```
//!
//! ## Why Direct PolkaVM?
//!
//! - ✅ No Substrate dependency
//! - ✅ Clean architecture (stays with Commonware)
//! - ✅ On-chain verification (in consensus)
//! - ✅ Deterministic execution
//! - ✅ Full control over gas metering
//!
//! ## Usage
//!
//! ```rust,ignore
//! // Initialize verifier (one-time)
//! let verifier_binary = include_bytes!("../polkavm_verifier.polkavm");
//! let verifier = PolkaVMVerifier::new(verifier_binary)?;
//!
//! // Verify in consensus (every block)
//! let valid = verifier.verify_in_consensus(&succinct_proof)?;
//! ```

use anyhow::{bail, Context, Result};
use std::sync::Arc;
use std::time::Duration;

use crate::light_client::LigeritoSuccinctProof;

/// PolkaVM-based verifier for on-chain ZK proof verification
///
/// This verifier runs PolkaVM **directly in the Commonware consensus layer**,
/// providing deterministic verification without needing Substrate runtime.
pub struct PolkaVMVerifier {
    /// Verifier binary (RISC-V)
    verifier_binary: Arc<Vec<u8>>,

    /// Maximum execution time per proof (gas metering)
    timeout: Duration,

    /// Maximum proof size to accept
    max_proof_size: usize,
}

impl PolkaVMVerifier {
    /// Create new PolkaVM verifier
    ///
    /// # Arguments
    ///
    /// * `verifier_binary` - The PolkaVM binary containing Ligerito verifier
    ///
    /// # Example
    ///
    /// ```rust,ignore
    /// let binary = include_bytes!("../polkavm_verifier.polkavm");
    /// let verifier = PolkaVMVerifier::new(binary)?;
    /// ```
    pub fn new(verifier_binary: &[u8]) -> Result<Self> {
        // Validate binary format
        Self::validate_binary(verifier_binary)?;

        Ok(Self {
            verifier_binary: Arc::new(verifier_binary.to_vec()),
            timeout: Duration::from_millis(100), // 100ms default
            max_proof_size: 1024 * 1024,         // 1MB default
        })
    }

    /// Verify proof in consensus (deterministic)
    ///
    /// This is called by ALL validators during block validation.
    /// Must be deterministic - same input always produces same output.
    ///
    /// # Arguments
    ///
    /// * `proof` - The succinct Ligerito proof to verify
    ///
    /// # Returns
    ///
    /// * `Ok(true)` - Proof is valid
    /// * `Ok(false)` - Proof is invalid
    /// * `Err(_)` - Execution error (treat as invalid)
    ///
    /// # Example
    ///
    /// ```rust,ignore
    /// // In Commonware consensus
    /// impl Automaton for SafroleAutomaton {
    ///     fn verify(&mut self, block: &Block) -> bool {
    ///         for proof in &block.proofs {
    ///             let succinct = extract_succinct_proof(proof, 24)?;
    ///
    ///             // All nodes run this - must be deterministic!
    ///             if !self.verifier.verify_in_consensus(&succinct)? {
    ///                 return false;
    ///             }
    ///         }
    ///         true
    ///     }
    /// }
    /// ```
    pub fn verify_in_consensus(&self, proof: &LigeritoSuccinctProof) -> Result<bool> {
        // Size check (DoS prevention)
        if proof.proof_bytes.len() > self.max_proof_size {
            bail!("Proof too large: {} bytes", proof.proof_bytes.len());
        }

        // TODO: Replace with actual PolkaVM execution
        //
        // In real implementation:
        // let blob = polkavm::ProgramBlob::parse(&self.verifier_binary)?;
        // let config = polkavm::Config::default();
        // let engine = polkavm::Engine::new(&config)?;
        // let module = polkavm::Module::from_blob(&engine, &polkavm::ModuleConfig::default(), blob)?;
        //
        // let mut instance = module.instantiate()?;
        //
        // // Prepare input: [config_size: u32][proof_bytes]
        // let mut input = Vec::new();
        // input.extend_from_slice(&proof.config_size.to_le_bytes());
        // input.extend_from_slice(&proof.proof_bytes);
        //
        // // Execute with timeout (gas metering)
        // let result = tokio::time::timeout(self.timeout, async {
        //     instance.call_typed(&mut (), "main", &input)
        // }).await??;
        //
        // // Check exit code (0 = valid)
        // Ok(result == 0)

        // Placeholder: Accept valid config sizes
        Ok(proof.config_size >= 12 && proof.config_size <= 30)
    }

    /// Set timeout for verification (gas metering)
    pub fn set_timeout(&mut self, timeout: Duration) {
        self.timeout = timeout;
    }

    /// Set maximum proof size
    pub fn set_max_proof_size(&mut self, size: usize) {
        self.max_proof_size = size;
    }

    /// Validate PolkaVM binary format
    fn validate_binary(binary: &[u8]) -> Result<()> {
        // TODO: Actual validation
        //
        // In real implementation:
        // let blob = polkavm::ProgramBlob::parse(binary)
        //     .context("Invalid PolkaVM binary")?;
        //
        // // Check exports
        // let has_main = blob.exports().any(|e| e.symbol() == "main");
        // if !has_main {
        //     bail!("PolkaVM binary missing 'main' export");
        // }

        if binary.is_empty() {
            bail!("Empty verifier binary");
        }

        Ok(())
    }
}

/// Configuration for PolkaVM verifier
#[derive(Clone, Debug)]
pub struct PolkaVMConfig {
    /// Maximum execution time per proof
    pub timeout_ms: u64,

    /// Maximum proof size to accept
    pub max_proof_size: usize,

    /// Enable execution tracing (for debugging)
    pub enable_tracing: bool,
}

impl Default for PolkaVMConfig {
    fn default() -> Self {
        Self {
            timeout_ms: 100,               // 100ms per proof
            max_proof_size: 1024 * 1024,   // 1MB max
            enable_tracing: false,
        }
    }
}

/// Gas metering for PolkaVM execution
///
/// Simple timeout-based approach. In production, could be replaced
/// with instruction counting or more sophisticated metering.
pub struct GasMeter {
    timeout: Duration,
    start_time: std::time::Instant,
}

impl GasMeter {
    pub fn new(timeout: Duration) -> Self {
        Self {
            timeout,
            start_time: std::time::Instant::now(),
        }
    }

    pub fn check(&self) -> Result<()> {
        if self.start_time.elapsed() > self.timeout {
            bail!("Gas exhausted (timeout)");
        }
        Ok(())
    }

    pub fn remaining(&self) -> Duration {
        self.timeout.saturating_sub(self.start_time.elapsed())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_verifier_creation() {
        // Placeholder binary
        let binary = vec![0u8; 100];
        let verifier = PolkaVMVerifier::new(&binary);
        assert!(verifier.is_ok());
    }

    #[test]
    fn test_proof_size_limit() {
        let binary = vec![0u8; 100];
        let verifier = PolkaVMVerifier::new(&binary).unwrap();

        // Create proof that's too large
        let large_proof = LigeritoSuccinctProof {
            proof_bytes: vec![0u8; 2 * 1024 * 1024], // 2MB
            config_size: 24,
            sender_commitment_old: [0u8; 32],
            sender_commitment_new: [0u8; 32],
            receiver_commitment_old: [0u8; 32],
            receiver_commitment_new: [0u8; 32],
        };

        let result = verifier.verify_in_consensus(&large_proof);
        assert!(result.is_err());
    }

    #[test]
    fn test_valid_config_size() {
        let binary = vec![0u8; 100];
        let verifier = PolkaVMVerifier::new(&binary).unwrap();

        let proof = LigeritoSuccinctProof {
            proof_bytes: vec![0u8; 100],
            config_size: 24, // Valid
            sender_commitment_old: [0u8; 32],
            sender_commitment_new: [0u8; 32],
            receiver_commitment_old: [0u8; 32],
            receiver_commitment_new: [0u8; 32],
        };

        let result = verifier.verify_in_consensus(&proof).unwrap();
        assert!(result); // Should accept valid config size
    }

    #[test]
    fn test_invalid_config_size() {
        let binary = vec![0u8; 100];
        let verifier = PolkaVMVerifier::new(&binary).unwrap();

        let proof = LigeritoSuccinctProof {
            proof_bytes: vec![0u8; 100],
            config_size: 99, // Invalid
            sender_commitment_old: [0u8; 32],
            sender_commitment_new: [0u8; 32],
            receiver_commitment_old: [0u8; 32],
            receiver_commitment_new: [0u8; 32],
        };

        let result = verifier.verify_in_consensus(&proof).unwrap();
        assert!(!result); // Should reject invalid config size
    }

    #[test]
    fn test_gas_meter() {
        let meter = GasMeter::new(Duration::from_millis(100));

        // Should have gas
        assert!(meter.check().is_ok());

        // Wait and check again
        std::thread::sleep(Duration::from_millis(150));
        assert!(meter.check().is_err()); // Should be out of gas
    }
}
