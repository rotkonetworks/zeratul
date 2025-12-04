//! ZYNC Core - Zero-knowledge sYNChronization for Zcash
//!
//! Cross-platform wallet core using Crux architecture.
//! This crate contains all business logic shared across platforms:
//! - Desktop (egui)
//! - Android (Jetpack Compose)
//! - iOS (SwiftUI)
//! - Web (WASM)
//!
//! Key features:
//! - WASM-compatible parallel note scanning
//! - Ligerito-powered header chain verification
//! - Epoch-based proof composition

#![allow(dead_code)]
#![allow(unused_imports)]
#![allow(unused_variables)]

pub mod state;
pub mod error;
pub mod app;
pub mod verifier;
pub mod trustless;
pub mod scanner;

#[cfg(feature = "client")]
pub mod client;

// Re-export Crux app
pub use app::{ZafuCore, Event, Model, ViewModel, Effect, Contact, ChatMessage};

pub use error::{ZyncError, Result};
pub use state::{WalletState, WalletStateCommitment};
pub use scanner::{Scanner, BatchScanner, ScanAction, DecryptedNote};

#[cfg(feature = "client")]
pub use client::{ZidecarClient, LightwalletdClient};
// pub use trace::{SyncTrace, TraceField};
// pub use proof::EpochProof;

use ligerito::{ProverConfig, VerifierConfig};
use ligerito_binary_fields::{BinaryElem32, BinaryElem128};
use std::marker::PhantomData;

/// blocks per epoch (~21 hours at 75s/block)
pub const EPOCH_SIZE: u32 = 1024;

/// max orchard actions per block
pub const MAX_ACTIONS_PER_BLOCK: usize = 512;

/// fields encoded per action in trace polynomial
pub const FIELDS_PER_ACTION: usize = 8;

/// polynomial size exponent for tip proofs (2^20 config, max ~32K headers)
pub const TIP_TRACE_LOG_SIZE: usize = 20;

/// polynomial size exponent for gigaproofs (2^26 config)
pub const GIGAPROOF_TRACE_LOG_SIZE: usize = 26;

/// security parameter (bits)
pub const SECURITY_BITS: usize = 100;

/// orchard activation height (mainnet)
pub const ORCHARD_ACTIVATION_HEIGHT: u32 = 1_687_104;

/// orchard activation height (testnet)
pub const ORCHARD_ACTIVATION_HEIGHT_TESTNET: u32 = 1_842_420;

/// domain separator for wallet state commitment
pub const DOMAIN_WALLET_STATE: &[u8] = b"ZYNC_wallet_state_v1";

/// domain separator for epoch proof hash
pub const DOMAIN_EPOCH_PROOF: &[u8] = b"ZYNC_epoch_proof_v1";

/// domain separator for ivk commitment
pub const DOMAIN_IVK_COMMIT: &[u8] = b"ZYNC_ivk_commit_v1";

/// genesis epoch hash (all zeros)
pub const GENESIS_EPOCH_HASH: [u8; 32] = [0u8; 32];

/// empty sparse merkle tree root
pub const EMPTY_SMT_ROOT: [u8; 32] = [0u8; 32]; // todo: compute actual empty root

/// ligerito prover config for tip proofs (2^20, ~0.1s, max ~32K blocks)
pub fn tip_prover_config() -> ProverConfig<BinaryElem32, BinaryElem128> {
    ligerito::hardcoded_config_20(
        PhantomData::<BinaryElem32>,
        PhantomData::<BinaryElem128>,
    )
}

/// ligerito prover config for gigaproofs (2^26, ~10s, multi-epoch)
pub fn gigaproof_prover_config() -> ProverConfig<BinaryElem32, BinaryElem128> {
    ligerito::hardcoded_config_26(
        PhantomData::<BinaryElem32>,
        PhantomData::<BinaryElem128>,
    )
}

/// select the appropriate prover config for a given trace size
/// returns (config, required_trace_size) - trace must be padded to required_trace_size
pub fn prover_config_for_size(trace_len: usize) -> (ProverConfig<BinaryElem32, BinaryElem128>, usize) {
    let log_size = if trace_len == 0 { 12 } else { (trace_len as f64).log2().ceil() as u32 };

    // available configs: 12, 16, 20, 24, 26, 28, 30
    let (config_log, config) = if log_size <= 12 {
        (12, ligerito::hardcoded_config_12(PhantomData::<BinaryElem32>, PhantomData::<BinaryElem128>))
    } else if log_size <= 16 {
        (16, ligerito::hardcoded_config_16(PhantomData::<BinaryElem32>, PhantomData::<BinaryElem128>))
    } else if log_size <= 20 {
        (20, ligerito::hardcoded_config_20(PhantomData::<BinaryElem32>, PhantomData::<BinaryElem128>))
    } else if log_size <= 24 {
        (24, ligerito::hardcoded_config_24(PhantomData::<BinaryElem32>, PhantomData::<BinaryElem128>))
    } else if log_size <= 26 {
        (26, ligerito::hardcoded_config_26(PhantomData::<BinaryElem32>, PhantomData::<BinaryElem128>))
    } else if log_size <= 28 {
        (28, ligerito::hardcoded_config_28(PhantomData::<BinaryElem32>, PhantomData::<BinaryElem128>))
    } else {
        (30, ligerito::hardcoded_config_30(PhantomData::<BinaryElem32>, PhantomData::<BinaryElem128>))
    };

    (config, 1 << config_log)
}

/// select the appropriate verifier config for a given log size
pub fn verifier_config_for_log_size(log_size: u32) -> VerifierConfig {
    if log_size <= 12 {
        ligerito::hardcoded_config_12_verifier()
    } else if log_size <= 16 {
        ligerito::hardcoded_config_16_verifier()
    } else if log_size <= 20 {
        ligerito::hardcoded_config_20_verifier()
    } else if log_size <= 24 {
        ligerito::hardcoded_config_24_verifier()
    } else if log_size <= 26 {
        ligerito::hardcoded_config_26_verifier()
    } else if log_size <= 28 {
        ligerito::hardcoded_config_28_verifier()
    } else {
        ligerito::hardcoded_config_30_verifier()
    }
}

/// ligerito verifier config for tip proofs (2^24)
pub fn tip_verifier_config() -> VerifierConfig {
    ligerito::hardcoded_config_24_verifier()
}

/// ligerito verifier config for gigaproofs (2^28)
pub fn gigaproof_verifier_config() -> VerifierConfig {
    ligerito::hardcoded_config_28_verifier()
}

/// helper: calculate epoch number from block height
pub fn epoch_for_height(height: u32) -> u32 {
    height / EPOCH_SIZE
}

/// helper: get start height of epoch
pub fn epoch_start(epoch: u32) -> u32 {
    epoch * EPOCH_SIZE
}

/// helper: get end height of epoch (inclusive)
pub fn epoch_end(epoch: u32) -> u32 {
    epoch_start(epoch + 1) - 1
}

/// wallet identifier (random 16 bytes)
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct WalletId([u8; 16]);

impl WalletId {
    pub fn random() -> Self {
        let mut bytes = [0u8; 16];
        rand::RngCore::fill_bytes(&mut rand::thread_rng(), &mut bytes);
        Self(bytes)
    }

    pub fn from_bytes(bytes: &[u8]) -> Result<Self> {
        if bytes.len() != 16 {
            return Err(ZyncError::InvalidData("wallet id must be 16 bytes".into()));
        }
        let mut arr = [0u8; 16];
        arr.copy_from_slice(bytes);
        Ok(Self(arr))
    }

    pub fn to_bytes(&self) -> &[u8; 16] {
        &self.0
    }
}

impl std::fmt::Display for WalletId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", hex::encode(&self.0[..8])) // short form
    }
}

/// helper: hex encoding (inline to avoid dependency)
mod hex {
    pub fn encode(bytes: &[u8]) -> String {
        bytes.iter().map(|b| format!("{:02x}", b)).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_wallet_id_roundtrip() {
        let id = WalletId::random();
        let bytes = id.to_bytes();
        let id2 = WalletId::from_bytes(bytes).unwrap();
        assert_eq!(id, id2);
    }

    // TODO: restore when TRACE_DATA_LOG_SIZE is defined
    // #[test]
    // fn test_constants_consistency() {
    //     // verify trace size calculation
    //     let blocks = 1 << 10; // EPOCH_SIZE rounded up to power of 2
    //     let actions = 1 << 9; // MAX_ACTIONS_PER_BLOCK
    //     let fields = 1 << 3; // FIELDS_PER_ACTION = 8
    //     assert_eq!(blocks * actions * fields, 1 << TRACE_DATA_LOG_SIZE);
    // }
}
