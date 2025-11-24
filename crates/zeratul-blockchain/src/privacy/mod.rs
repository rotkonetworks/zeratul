//! Zeratul Privacy Layer - Hybrid MPC + ZK Architecture
//!
//! ## Design Philosophy
//!
//! Different operations need different privacy techniques:
//!
//! - **Simple operations** (90% of transactions): MPC with ZODA-VSS
//!   - Transfers, swaps, voting
//!   - Fast (no proof generation)
//!   - Validators compute on secret shares
//!
//! - **Complex computation** (10% of transactions): Ligerito ZK proofs
//!   - Smart contracts, DeFi logic
//!   - Flexible (arbitrary computation)
//!   - Client generates succinct proof
//!
//! ## Architecture
//!
//! ```text
//! Transaction
//!     │
//!     ├─ Simple? ──→ MPC Layer
//!     │              ├─ Secret-shared state (ZODA-VSS)
//!     │              ├─ Validators compute on shares
//!     │              └─ No proof needed (Merkle verification)
//!     │
//!     └─ Complex? ──→ Ligerito Layer
//!                    ├─ Client generates ZK proof
//!                    ├─ Validators verify proof
//!                    └─ Succinct (small proof size)
//! ```

pub mod mpc;           // MPC with ZODA-VSS (simple operations)
pub mod polkavm_zoda;  // PolkaVM execution with ZODA verification
pub mod ligerito;      // Ligerito proofs (complex computation)
pub mod hybrid;        // Unified interface

pub use mpc::{MPCState, MPCOperation, ZodaShare, ZodaCommitment};
pub use polkavm_zoda::{ZodaTrace, PolkaVMZodaClient, PolkaVMZodaValidator};
pub use ligerito::{LigeritoProof, LigeritoProver};
pub use hybrid::{HybridPrivacy, PrivacyClient, ExecutionResult};

use anyhow::Result;
use serde::{Deserialize, Serialize};

/// Privacy mode for a transaction
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum PrivacyMode {
    /// MPC on secret shares (fast, for simple ops)
    MPC {
        operation: mpc::MPCOperation,
        shares: Vec<mpc::ZodaShare>,
    },

    /// PolkaVM execution with ZODA verification (smart contracts)
    PolkaVM {
        commitment: mpc::ZodaCommitment,
        share: mpc::ZodaShare,
        public_inputs: Vec<u8>,
    },

    /// Ligerito proof (flexible, for complex ops)
    Ligerito {
        proof: Vec<u8>,  // Serialized Ligerito proof
        public_inputs: Vec<u8>,
    },
}

/// Transaction with privacy
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PrivateTransaction {
    /// Privacy mode (MPC or Ligerito)
    pub privacy_mode: PrivacyMode,

    /// Nonce (to prevent replay)
    pub nonce: u64,

    /// Signature (authorizes transaction)
    pub signature: Vec<u8>,
}

impl PrivateTransaction {
    /// Create a simple transfer (uses MPC)
    pub fn new_transfer(
        from: [u8; 32],
        to: [u8; 32],
        amount_shares: Vec<mpc::ZodaShare>,
        nonce: u64,
    ) -> Self {
        Self {
            privacy_mode: PrivacyMode::MPC {
                operation: mpc::MPCOperation::Transfer { from, to },
                shares: amount_shares,
            },
            nonce,
            signature: Vec::new(), // TODO: Sign
        }
    }

    /// Create a complex operation (uses Ligerito)
    pub fn new_complex(
        proof: Vec<u8>,
        public_inputs: Vec<u8>,
        nonce: u64,
    ) -> Self {
        Self {
            privacy_mode: PrivacyMode::Ligerito {
                proof,
                public_inputs,
            },
            nonce,
            signature: Vec::new(), // TODO: Sign
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_privacy_mode_serialization() {
        let tx = PrivateTransaction::new_transfer(
            [0; 32],
            [1; 32],
            vec![],
            0,
        );

        let serialized = bincode::serialize(&tx).unwrap();
        let deserialized: PrivateTransaction = bincode::deserialize(&serialized).unwrap();

        assert_eq!(tx.nonce, deserialized.nonce);
    }
}
