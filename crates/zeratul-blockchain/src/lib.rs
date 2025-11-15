//! Zeratul - State Transition Blockchain
//!
//! A Byzantine fault-tolerant blockchain for privacy-preserving state transitions using:
//! - **Ligerito PCS** for fast zero-knowledge proofs on binary fields
//! - **AccidentalComputer** pattern to reuse ZODA encoding as polynomial commitments
//! - **NOMT** for authenticated state storage
//! - **Commonware** for production-grade distributed systems primitives

pub mod application;
pub mod block;
pub mod engine;
pub mod lending;
pub mod dos_prevention;
pub mod validator_reputation;
pub mod frost;  // FROST threshold signature integration
pub mod frost_zoda;  // ZODA-enhanced FROST with VSSS (malicious security)
pub mod penumbra;  // Penumbra integration (oracle, IBC, light client)
pub mod governance;  // NPoS with Phragmén election
pub mod consensus;  // Safrole block production (JAM-style)
pub mod light_client;  // Light client sync with PolkaVM verification
pub mod verifier;  // On-chain ZK proof verification via PolkaVM
pub mod execution;  // Execution layer: PolkaVM + Ligerito proofs

pub use application::{Actor as Application, Config as ApplicationConfig, Mailbox as ApplicationMailbox};
pub use block::Block;
pub use engine::{Config as EngineConfig, Engine};
pub use lending::{LendingPool, PoolState, Position, LendingAction};
pub use light_client::{LightClient, LightClientConfig, LigeritoSuccinctProof, extract_succinct_proof};

use commonware_consensus::marshal::SchemeProvider;
use commonware_cryptography::bls12381::primitives::{group, poly::Poly};
use commonware_cryptography::ed25519::PublicKey;
use serde::{Deserialize, Serialize};
use std::{collections::HashMap, net::SocketAddr, sync::Arc};

/// Signing scheme for consensus
pub type Scheme = commonware_cryptography::bls12381::primitives::variant::MinSig;

/// Evaluation type for BLS12-381
pub type Evaluation = commonware_cryptography::bls12381::primitives::group::G1;

/// Configuration for the blockchain
#[derive(Deserialize, Serialize)]
pub struct Config {
    pub private_key: String,
    pub share: String,
    pub polynomial: String,

    pub port: u16,
    pub metrics_port: u16,
    pub directory: String,
    pub worker_threads: usize,
    pub log_level: String,

    pub local: bool,
    pub allowed_peers: Vec<String>,
    pub bootstrappers: Vec<String>,

    pub message_backlog: usize,
    pub mailbox_size: usize,
    pub deque_size: usize,

    pub nomt_path: String,
}

/// A list of peers provided when a validator is run locally
#[derive(Deserialize, Serialize)]
pub struct Peers {
    pub addresses: HashMap<String, SocketAddr>,
}

/// A static provider that always returns the same signing scheme
#[derive(Clone)]
pub struct StaticSchemeProvider(Arc<commonware_consensus::simplex::Scheme<Scheme, PublicKey>>);

impl SchemeProvider for StaticSchemeProvider {
    type Scheme = commonware_consensus::simplex::Scheme<Scheme, PublicKey>;

    fn scheme(&self, _epoch: u64) -> Option<Arc<Self::Scheme>> {
        Some(self.0.clone())
    }
}

impl From<commonware_consensus::simplex::Scheme<Scheme, PublicKey>> for StaticSchemeProvider {
    fn from(scheme: commonware_consensus::simplex::Scheme<Scheme, PublicKey>) -> Self {
        Self(Arc::new(scheme))
    }
}
