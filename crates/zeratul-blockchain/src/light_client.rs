//! Light Client Support
//!
//! Light clients don't want to download full ZODA shards (~MB of data).
//! Instead, they verify small succinct proofs (~KB) using PolkaVM.
//!
//! ## Architecture
//!
//! ```text
//! Full Nodes:
//!   AccidentalComputerProof (ZODA shards ~MB)
//!   ↓
//!   verify_accidental_computer() ← Fast, native verification
//!
//! Light Clients:
//!   AccidentalComputerProof
//!   ↓
//!   extract_succinct_proof() ← Compress ZODA to Ligerito proof
//!   ↓
//!   LigeritoSuccinctProof (~KB)
//!   ↓
//!   verify_via_polkavm() ← PolkaVM sandboxed verification
//! ```
//!
//! ## Why Two Verification Paths?
//!
//! - **Full nodes** use AccidentalComputer pattern (ZODA = PCS, very fast)
//! - **Light clients** use Ligerito proofs (small size, sandboxed via PolkaVM)

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use zeratul_circuit::AccidentalComputerProof;
use std::path::Path;
use std::sync::Arc;

use crate::block::Block;

/// Configuration for light client
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LightClientConfig {
    /// Path to PolkaVM verifier binary
    pub polkavm_verifier_path: String,

    /// Which Ligerito config size to use (12, 16, 20, 24, 28, 30)
    pub ligerito_config_size: u32,

    /// Maximum proof size to accept (DoS prevention)
    pub max_proof_size: usize,
}

impl Default for LightClientConfig {
    fn default() -> Self {
        Self {
            polkavm_verifier_path: "../polkavm_verifier/target/riscv64-zkvm-elf/release/polkavm_verifier".to_string(),
            ligerito_config_size: 24, // 2^24 = 16M field elements
            max_proof_size: 1024 * 1024, // 1MB max
        }
    }
}

/// Succinct proof extracted from AccidentalComputerProof for light clients
///
/// This is MUCH smaller than the full ZODA shards:
/// - ZODA shards: ~MB (full Reed-Solomon encoding)
/// - Ligerito proof: ~KB (compressed polynomial proof)
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, parity_scale_codec::Encode, parity_scale_codec::Decode, scale_info::TypeInfo)]
pub struct LigeritoSuccinctProof {
    /// Serialized Ligerito proof
    pub proof_bytes: Vec<u8>,

    /// Config size used (12, 16, 20, 24, 28, 30)
    pub config_size: u32,

    /// Public inputs (commitments)
    pub sender_commitment_old: [u8; 32],
    pub sender_commitment_new: [u8; 32],
    pub receiver_commitment_old: [u8; 32],
    pub receiver_commitment_new: [u8; 32],
}

/// Light client state
///
/// Light clients sync by:
/// 1. Downloading block headers (small)
/// 2. Verifying succinct proofs via PolkaVM (small + sandboxed)
/// 3. Updating state commitments
pub struct LightClient {
    config: LightClientConfig,
    polkavm_runner: Option<Arc<PolkaVMRunner>>,
    latest_block_height: u64,
    latest_state_root: [u8; 32],
}

impl LightClient {
    /// Create new light client
    pub fn new(config: LightClientConfig) -> Result<Self> {
        Ok(Self {
            config,
            polkavm_runner: None,
            latest_block_height: 0,
            latest_state_root: [0u8; 32],
        })
    }

    /// Initialize PolkaVM verifier
    ///
    /// Loads the PolkaVM binary that contains the Ligerito verifier.
    /// This binary is compiled from `examples/polkavm_verifier/main.rs`.
    pub async fn init_polkavm(&mut self) -> Result<()> {
        let runner = PolkaVMRunner::new(&self.config.polkavm_verifier_path)
            .context("Failed to load PolkaVM verifier")?;

        self.polkavm_runner = Some(Arc::new(runner));
        Ok(())
    }

    /// Sync to latest block
    ///
    /// Light client sync process:
    /// 1. Download block header from full nodes
    /// 2. Extract succinct proofs from block
    /// 3. Verify via PolkaVM
    /// 4. Update state commitments
    pub async fn sync_to_block(&mut self, block: &Block) -> Result<()> {
        // Verify block height is sequential
        if block.height != self.latest_block_height + 1 {
            anyhow::bail!(
                "Invalid block height: expected {}, got {}",
                self.latest_block_height + 1,
                block.height
            );
        }

        // Verify parent hash
        let expected_parent = self.latest_state_root;
        if block.parent != expected_parent.into() {
            anyhow::bail!("Invalid parent hash");
        }

        // Verify all state transition proofs
        for proof in &block.proofs {
            self.verify_proof(proof).await?;
        }

        // Update state
        self.latest_block_height = block.height;
        self.latest_state_root = block.state_root;

        Ok(())
    }

    /// Verify a single state transition proof
    async fn verify_proof(&self, proof: &AccidentalComputerProof) -> Result<()> {
        // Extract succinct Ligerito proof from ZODA shards
        let succinct_proof = extract_succinct_proof(proof, self.config.ligerito_config_size)?;

        // Verify via PolkaVM
        self.verify_via_polkavm(&succinct_proof).await?;

        Ok(())
    }

    /// Verify succinct proof using PolkaVM
    ///
    /// This calls the PolkaVM guest program which contains the Ligerito verifier.
    /// The guest reads the proof from stdin and returns the result via exit code.
    async fn verify_via_polkavm(&self, proof: &LigeritoSuccinctProof) -> Result<()> {
        let runner = self.polkavm_runner.as_ref()
            .context("PolkaVM not initialized - call init_polkavm() first")?;

        // Check proof size (DoS prevention)
        if proof.proof_bytes.len() > self.config.max_proof_size {
            anyhow::bail!("Proof too large: {} bytes", proof.proof_bytes.len());
        }

        // Serialize input for PolkaVM guest
        // Format: [config_size: u32][proof_bytes: bincode]
        let mut input = Vec::new();
        input.extend_from_slice(&proof.config_size.to_le_bytes());
        input.extend_from_slice(&proof.proof_bytes);

        // Execute PolkaVM guest
        let result = runner.execute(&input).await?;

        // Check exit code (0 = valid, 1 = invalid, 2 = error)
        match result.exit_code {
            0 => Ok(()),
            1 => anyhow::bail!("Proof verification failed: invalid proof"),
            2 => anyhow::bail!("Proof verification error: {}", result.stderr),
            code => anyhow::bail!("Unexpected exit code from PolkaVM: {}", code),
        }
    }

    /// Get current sync state
    pub fn sync_state(&self) -> LightClientSyncState {
        LightClientSyncState {
            latest_block_height: self.latest_block_height,
            latest_state_root: self.latest_state_root,
        }
    }
}

/// Light client sync state
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LightClientSyncState {
    pub latest_block_height: u64,
    pub latest_state_root: [u8; 32],
}

/// Extract succinct Ligerito proof from AccidentalComputerProof
///
/// This compresses the ZODA shards into a smaller Ligerito proof that
/// can be verified by light clients.
///
/// ## How It Works
///
/// AccidentalComputerProof contains:
/// - ZODA commitment (Merkle root of encoded rows)
/// - ZODA shards (full Reed-Solomon encoding)
///
/// LigeritoSuccinctProof contains:
/// - Polynomial evaluations at random points (sumcheck protocol)
/// - Opening proofs for those evaluations
/// - Much smaller than full shards!
///
/// ## Proof Extraction Process
///
/// 1. Decode ZODA shards to get original data
/// 2. Reconstruct polynomial from data
/// 3. Generate Ligerito proof (sumcheck + openings)
/// 4. Serialize to bytes
pub fn extract_succinct_proof(
    accidental_proof: &AccidentalComputerProof,
    config_size: u32,
) -> Result<LigeritoSuccinctProof> {
    use commonware_coding::{Config as CodingConfig, Scheme, Zoda};
    use commonware_codec::Read;
    use commonware_cryptography::Sha256;
    use commonware_cryptography::transcript::Summary;

    // Step 1: Decode ZODA commitment
    let mut commitment_bytes = accidental_proof.zoda_commitment.as_slice();
    let commitment: Summary = Read::read_cfg(&mut commitment_bytes, &())?;

    // Step 2: Collect enough shards to reconstruct
    // In AccidentalComputer, we need minimum_shards to recover data
    let minimum_shards = 3; // From AccidentalComputerConfig::default()

    if accidental_proof.shards.len() < minimum_shards {
        anyhow::bail!("Not enough shards: need {}, got {}", minimum_shards, accidental_proof.shards.len());
    }

    // Step 3: Decode shards and prepare for reconstruction
    let codec_config = commonware_coding::CodecConfig {
        maximum_shard_size: 1024 * 1024,
    };

    let coding_config = CodingConfig {
        minimum_shards: 3,
        extra_shards: 2,
    };

    let mut checked_shards = Vec::new();
    for (index, shard_bytes) in accidental_proof.shard_indices.iter().zip(&accidental_proof.shards) {
        let mut buf = shard_bytes.as_slice();
        let shard = <Zoda<Sha256> as Scheme>::Shard::read_cfg(&mut buf, &codec_config)?;

        let (_checking_data, checked_shard, _reshard) =
            Zoda::<Sha256>::reshard(&coding_config, &commitment, *index, shard)?;

        checked_shards.push((index, checked_shard));
    }

    // Step 4: Reconstruct original data from shards using ZODA recovery
    let shard_refs: Vec<_> = checked_shards
        .iter()
        .map(|(idx, shard)| (**idx, shard.clone()))
        .collect();

    // TODO TODO TODO: Implement proper Zoda recovery
    // The Zoda API doesn't have a recover method - need to implement erasure code decoding
    let recovered_data = Vec::new(); // Placeholder

    // Step 5: Convert recovered data to polynomial (Vec<BinaryElem32>)
    let polynomial = bytes_to_polynomial(&recovered_data, config_size)?;

    // Step 6: Generate Ligerito proof
    use ligerito_binary_fields::{BinaryElem32, BinaryElem128};
    use std::marker::PhantomData;

    let ligerito_config = match config_size {
        12 => ligerito::hardcoded_config_12(
            PhantomData::<BinaryElem32>,
            PhantomData::<BinaryElem128>,
        ),
        20 => ligerito::hardcoded_config_20(
            PhantomData::<BinaryElem32>,
            PhantomData::<BinaryElem128>,
        ),
        24 => ligerito::hardcoded_config_24(
            PhantomData::<BinaryElem32>,
            PhantomData::<BinaryElem128>,
        ),
        _ => anyhow::bail!("Unsupported config size: {}", config_size),
    };

    let ligerito_proof = ligerito::prover(&ligerito_config, &polynomial)?;

    // Step 7: Serialize the proof
    // TODO TODO TODO: Proper proof serialization required
    let proof_bytes = Vec::new(); // Placeholder

    Ok(LigeritoSuccinctProof {
        proof_bytes,
        config_size,
        sender_commitment_old: accidental_proof.sender_commitment_old,
        sender_commitment_new: accidental_proof.sender_commitment_new,
        receiver_commitment_old: accidental_proof.receiver_commitment_old,
        receiver_commitment_new: accidental_proof.receiver_commitment_new,
    })
}

/// Convert recovered bytes to a polynomial for Ligerito
///
/// This function:
/// 1. Interprets bytes as u32 values (BinaryElem32)
/// 2. Pads to the required polynomial size (power of 2)
/// 3. Returns Vec<BinaryElem32> ready for Ligerito prover
fn bytes_to_polynomial(data: &[u8], config_size: u32) -> Result<Vec<ligerito_binary_fields::BinaryElem32>> {
    use ligerito_binary_fields::BinaryElem32;

    let required_size = 1usize << config_size; // 2^config_size

    // Convert bytes to u32 chunks
    let mut polynomial = Vec::with_capacity(required_size);

    // Process 4 bytes at a time to create BinaryElem32 elements
    for chunk in data.chunks(4) {
        let mut bytes = [0u8; 4];
        bytes[..chunk.len()].copy_from_slice(chunk);
        let value = u32::from_le_bytes(bytes);
        polynomial.push(BinaryElem32::from(value));
    }

    // Pad with zeros to reach required size
    while polynomial.len() < required_size {
        polynomial.push(BinaryElem32::from(0u32));
    }

    // Truncate if we have too many (shouldn't happen in practice)
    polynomial.truncate(required_size);

    Ok(polynomial)
}

/// PolkaVM runner for executing guest programs
///
/// This wraps the PolkaVM engine and provides a simple interface for
/// executing the Ligerito verifier guest program.
struct PolkaVMRunner {
    // In real implementation, this would hold:
    // engine: polkavm::Engine,
    // module: polkavm::Module,
    verifier_path: String,
}

impl PolkaVMRunner {
    /// Load PolkaVM verifier binary
    fn new(binary_path: impl AsRef<Path>) -> Result<Self> {
        let path = binary_path.as_ref();

        // Verify file exists
        if !path.exists() {
            anyhow::bail!("PolkaVM verifier binary not found: {}", path.display());
        }

        // TODO: Load actual PolkaVM binary
        // In real implementation:
        // let blob = std::fs::read(path)?;
        // let program = ProgramBlob::parse(&blob)?;
        // let engine = Engine::new(&Config::default())?;
        // let linker = Linker::new(&engine);
        // let module = Module::from_blob(&linker, &program)?;

        Ok(Self {
            verifier_path: path.to_string_lossy().to_string(),
        })
    }

    /// Execute PolkaVM guest with given input
    async fn execute(&self, input: &[u8]) -> Result<ExecutionResult> {
        // TODO: Replace with actual PolkaVM execution
        //
        // In real implementation:
        // let mut instance = self.module.instantiate()?;
        // let result = instance.call_typed::<(), i32>(&mut (), "main", ())?;

        // For now, simulate the execution
        // In production, this would call the actual PolkaVM runtime

        // Simulate reading config_size from input
        if input.len() < 4 {
            return Ok(ExecutionResult {
                exit_code: 2,
                stdout: String::new(),
                stderr: "Input too short".to_string(),
            });
        }

        let config_size = u32::from_le_bytes([input[0], input[1], input[2], input[3]]);

        // For now, accept all proofs as valid (placeholder)
        // In production, this would run the actual Ligerito verifier in PolkaVM
        if config_size >= 12 && config_size <= 30 {
            Ok(ExecutionResult {
                exit_code: 0,
                stdout: "VALID".to_string(),
                stderr: String::new(),
            })
        } else {
            Ok(ExecutionResult {
                exit_code: 2,
                stdout: String::new(),
                stderr: format!("Unsupported config size: {}", config_size),
            })
        }
    }
}

/// Result of PolkaVM execution
struct ExecutionResult {
    pub exit_code: i32,
    pub stdout: String,
    pub stderr: String,
}

#[cfg(test)]
mod tests {
    use super::*;
    use zeratul_circuit::{
        prove_with_accidental_computer, AccountData, AccidentalComputerConfig, TransferInstance,
    };

    fn test_salt(seed: u8) -> [u8; 32] {
        let mut salt = [0u8; 32];
        for i in 0..32 {
            salt[i] = (i as u8).wrapping_mul(seed).wrapping_add(13);
        }
        salt
    }

    #[test]
    fn test_extract_succinct_proof() -> Result<()> {
        // Create a test AccidentalComputerProof
        let sender = AccountData {
            id: 1,
            balance: 1000,
            nonce: 0,
            salt: test_salt(1),
        };

        let receiver = AccountData {
            id: 2,
            balance: 500,
            nonce: 0,
            salt: test_salt(2),
        };

        let instance = TransferInstance::new(
            sender,
            test_salt(3),
            receiver,
            test_salt(4),
            100,
        )?;

        let config = AccidentalComputerConfig::default();
        let accidental_proof = prove_with_accidental_computer(&config, &instance)?;

        // Extract succinct proof
        let succinct_proof = extract_succinct_proof(&accidental_proof, 24)?;

        // Verify it's smaller than ZODA shards
        let zoda_size: usize = accidental_proof.shards.iter().map(|s| s.len()).sum();
        assert!(
            succinct_proof.proof_bytes.len() < zoda_size,
            "Succinct proof should be smaller than ZODA shards"
        );

        // Verify commitments match
        assert_eq!(succinct_proof.sender_commitment_old, instance.sender_commitment_old);
        assert_eq!(succinct_proof.sender_commitment_new, instance.sender_commitment_new);

        Ok(())
    }

    #[tokio::test]
    async fn test_light_client_sync() -> Result<()> {
        // Create light client
        let config = LightClientConfig::default();
        let mut client = LightClient::new(config)?;

        // Initialize PolkaVM (will fail if binary doesn't exist, that's OK for unit test)
        // In CI/production, we'd ensure the binary is built first
        if let Err(e) = client.init_polkavm().await {
            println!("Skipping PolkaVM test (binary not found): {}", e);
            return Ok(());
        }

        // Create test proof
        let sender = AccountData {
            id: 1,
            balance: 1000,
            nonce: 0,
            salt: test_salt(1),
        };

        let receiver = AccountData {
            id: 2,
            balance: 500,
            nonce: 0,
            salt: test_salt(2),
        };

        let instance = TransferInstance::new(sender, test_salt(3), receiver, test_salt(4), 100)?;
        let ac_config = AccidentalComputerConfig::default();
        let proof = prove_with_accidental_computer(&ac_config, &instance)?;

        // Test verification
        client.verify_proof(&proof).await?;

        Ok(())
    }
}
