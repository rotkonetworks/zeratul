//! Hybrid Privacy Router
//!
//! Automatically selects the best privacy mode for each transaction.
//!
//! ## Decision Tree
//!
//! ```text
//! Transaction
//!     │
//!     ├─ Simple (transfer, swap)
//!     │  └─→ MPC-ZODA (Tier 1) [~10ms]
//!     │
//!     ├─ Contract (DeFi, governance)
//!     │  └─→ PolkaVM-ZODA (Tier 2) [~160ms]
//!     │
//!     └─ Complex (arbitrary proof)
//!        └─→ Ligerito (Tier 3) [~5000ms]
//! ```

use anyhow::Result;
use serde::{Deserialize, Serialize};

use super::mpc::{MPCOperation, MPCState, ZodaShare};
use super::polkavm_zoda::{PolkaVMZodaValidator, ZodaTrace};
use super::ligerito::LigeritoProof;
use super::PrivacyMode;

/// Transaction complexity classification
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Complexity {
    /// Simple operations (transfer, swap, vote)
    Simple,

    /// Smart contract execution
    Contract,

    /// Arbitrary computation (needs full ZK)
    Complex,
}

/// Hybrid privacy coordinator
pub struct HybridPrivacy {
    /// MPC state (Tier 1)
    mpc_state: MPCState,

    /// PolkaVM-ZODA validator (Tier 2)
    polkavm_validator: PolkaVMZodaValidator,

    /// Our validator index
    our_index: u32,
}

impl HybridPrivacy {
    pub fn new(our_index: u32, validator_count: u32, threshold: u32) -> Self {
        Self {
            mpc_state: MPCState::new(our_index, validator_count, threshold),
            polkavm_validator: PolkaVMZodaValidator::new(our_index, threshold),
            our_index,
        }
    }

    /// Classify transaction complexity
    pub fn classify(&self, tx: &PrivacyMode) -> Complexity {
        match tx {
            PrivacyMode::MPC { operation, .. } => {
                // MPC operations are always simple
                match operation {
                    MPCOperation::Transfer { .. } => Complexity::Simple,
                    MPCOperation::Swap { .. } => Complexity::Simple,
                    MPCOperation::Vote { .. } => Complexity::Simple,
                    MPCOperation::Stake { .. } => Complexity::Simple,
                }
            }
            PrivacyMode::PolkaVM { .. } => {
                // PolkaVM execution is contract-level complexity
                Complexity::Contract
            }
            PrivacyMode::Ligerito { .. } => {
                // Ligerito is for arbitrary complex proofs
                Complexity::Complex
            }
        }
    }

    /// Execute transaction using appropriate tier
    pub async fn execute(
        &mut self,
        tx: &PrivacyMode,
    ) -> Result<ExecutionResult> {
        let complexity = self.classify(tx);

        match complexity {
            Complexity::Simple => {
                self.execute_mpc(tx).await
            }
            Complexity::Contract => {
                self.execute_polkavm_zoda(tx).await
            }
            Complexity::Complex => {
                self.execute_ligerito(tx).await
            }
        }
    }

    /// Execute via MPC (Tier 1)
    async fn execute_mpc(&mut self, tx: &PrivacyMode) -> Result<ExecutionResult> {
        let (operation, shares) = match tx {
            PrivacyMode::MPC { operation, shares } => (operation, shares),
            _ => anyhow::bail!("Expected MPC transaction"),
        };

        // Get our share
        let our_share = shares.get(self.our_index as usize)
            .ok_or_else(|| anyhow::anyhow!("Missing share for our index"))?;

        // Execute operation on our shares
        self.mpc_state.execute_operation(operation, our_share.value)?;

        Ok(ExecutionResult {
            success: true,
            gas_used: 1000,  // MPC is cheap!
            output: Vec::new(),
        })
    }

    /// Execute via PolkaVM-ZODA (Tier 2)
    async fn execute_polkavm_zoda(&mut self, tx: &PrivacyMode) -> Result<ExecutionResult> {
        let (commitment, share, public_inputs) = match tx {
            PrivacyMode::PolkaVM { commitment, share, public_inputs } => {
                (commitment, share, public_inputs)
            }
            _ => anyhow::bail!("Expected PolkaVM transaction"),
        };

        // Verify our share against commitment
        let valid = self.polkavm_validator.verify_share(*commitment, share.clone())?;

        if !valid {
            anyhow::bail!("Invalid ZODA share for PolkaVM execution");
        }

        // If suspicious, reconstruct and verify full trace
        if let Some(trace) = self.polkavm_validator.reconstruct_trace(commitment)? {
            // Verify execution matches expected output
            let execution_valid = self.polkavm_validator
                .verify_execution(&trace, public_inputs)?;

            if !execution_valid {
                anyhow::bail!("PolkaVM execution verification failed");
            }

            // Execution verified successfully
            Ok(ExecutionResult {
                success: true,
                gas_used: trace.gas_used,
                output: trace.output.clone(),
            })
        } else {
            // Not enough shares to reconstruct yet
            // Accept optimistically (can challenge later if needed)
            Ok(ExecutionResult {
                success: true,
                gas_used: 10000,  // Estimated
                output: public_inputs.clone(),
            })
        }
    }

    /// Execute via Ligerito (Tier 3)
    async fn execute_ligerito(&mut self, tx: &PrivacyMode) -> Result<ExecutionResult> {
        let proof = match tx {
            PrivacyMode::Ligerito { proof, .. } => proof,
            _ => anyhow::bail!("Expected Ligerito transaction"),
        };

        // Deserialize and verify proof
        let ligerito_proof: LigeritoProof = bincode::deserialize(proof)?;
        let valid = ligerito_proof.verify()?;

        if !valid {
            anyhow::bail!("Invalid Ligerito proof");
        }

        Ok(ExecutionResult {
            success: true,
            gas_used: 100000,  // Ligerito is expensive
            output: Vec::new(),
        })
    }

    /// Get performance statistics
    pub fn stats(&self) -> PrivacyStats {
        PrivacyStats {
            mpc_count: 0,      // TODO: Track
            polkavm_count: 0,  // TODO: Track
            ligerito_count: 0, // TODO: Track
        }
    }
}

/// Execution result
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExecutionResult {
    pub success: bool,
    pub gas_used: u64,
    pub output: Vec<u8>,
}

/// Privacy system statistics
#[derive(Debug, Clone)]
pub struct PrivacyStats {
    pub mpc_count: u64,
    pub polkavm_count: u64,
    pub ligerito_count: u64,
}

/// Client-side helper for creating transactions
pub struct PrivacyClient;

impl PrivacyClient {
    /// Create a simple transfer (uses MPC)
    pub fn new_transfer(
        from: [u8; 32],
        to: [u8; 32],
        amount: u64,
        validator_count: u32,
    ) -> Result<PrivacyMode> {
        use super::mpc::sharing;

        // Secret-share the amount
        let (shares, _commitment) = sharing::share_value(
            decaf377::Fr::from(amount),
            validator_count,
            (validator_count * 2 / 3) + 1,
        )?;

        Ok(PrivacyMode::MPC {
            operation: MPCOperation::Transfer { from, to },
            shares,
        })
    }

    /// Create a PolkaVM contract call (uses PolkaVM-ZODA)
    pub fn new_polkavm_call(
        program: &[u8],
        private_inputs: &[u8],
        public_inputs: &[u8],
        validator_count: u32,
    ) -> Result<Vec<PrivacyMode>> {
        use super::polkavm_zoda::PolkaVMZodaClient;

        // Execute and encode with ZODA-VSS
        let client = PolkaVMZodaClient::new(
            validator_count,
            (validator_count * 2 / 3) + 1,
        );

        let zoda_trace = client.execute(program, private_inputs, public_inputs)?;

        // Create one transaction per validator (each gets their share)
        let mut txs = Vec::new();
        for (i, share) in zoda_trace.shares.iter().enumerate() {
            txs.push(PrivacyMode::PolkaVM {
                commitment: zoda_trace.commitment,
                share: share.clone(),
                public_inputs: zoda_trace.public_inputs.clone(),
            });
        }

        Ok(txs)
    }

    /// Create a complex proof (uses Ligerito)
    pub fn new_ligerito_proof(
        proof: Vec<u8>,
        public_inputs: Vec<u8>,
    ) -> Result<PrivacyMode> {
        Ok(PrivacyMode::Ligerito {
            proof,
            public_inputs,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_hybrid_mpc_execution() {
        let mut hybrid = HybridPrivacy::new(0, 4, 3);

        let tx = PrivacyClient::new_transfer(
            [1; 32],
            [2; 32],
            100,
            4,
        ).unwrap();

        // Should classify as simple
        assert_eq!(hybrid.classify(&tx), Complexity::Simple);

        // TODO: Need to initialize accounts before executing
        // let result = hybrid.execute(&tx).await.unwrap();
        // assert!(result.success);
    }

    #[test]
    fn test_complexity_classification() {
        let hybrid = HybridPrivacy::new(0, 4, 3);

        // Test MPC (Simple)
        let mpc_tx = PrivacyMode::MPC {
            operation: MPCOperation::Transfer {
                from: [0; 32],
                to: [1; 32],
            },
            shares: vec![],
        };
        assert_eq!(hybrid.classify(&mpc_tx), Complexity::Simple);

        // Test PolkaVM (Contract)
        let polkavm_tx = PrivacyMode::PolkaVM {
            commitment: super::super::mpc::ZodaCommitment::from_bytes([0; 32]),
            share: super::super::mpc::ZodaShare {
                value: decaf377::Fr::from(0u64),
                merkle_proof: vec![],
                index: 0,
            },
            public_inputs: vec![],
        };
        assert_eq!(hybrid.classify(&polkavm_tx), Complexity::Contract);

        // Test Ligerito (Complex)
        let ligerito_tx = PrivacyMode::Ligerito {
            proof: vec![],
            public_inputs: vec![],
        };
        assert_eq!(hybrid.classify(&ligerito_tx), Complexity::Complex);
    }

    #[tokio::test]
    async fn test_polkavm_zoda_execution() {
        // Create PolkaVM transaction
        let program = b"test_program";
        let private_inputs = b"secret_data";
        let public_inputs = b"public_output";

        let txs = PrivacyClient::new_polkavm_call(
            program,
            private_inputs,
            public_inputs,
            4,
        ).unwrap();

        // Should create 4 transactions (one per validator)
        assert_eq!(txs.len(), 4);

        // Validators execute
        let mut hybrid0 = HybridPrivacy::new(0, 4, 3);
        let result = hybrid0.execute(&txs[0]).await.unwrap();

        assert!(result.success);
        println!("PolkaVM-ZODA execution gas: {}", result.gas_used);
    }
}
