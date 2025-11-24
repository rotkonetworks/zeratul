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
pub mod dkg;  // DKG abstraction layer (FROST MVP, golden_decaf377 future)
pub mod privacy;  // 3-tier privacy: MPC-ZODA, PolkaVM-ZODA, Ligerito
pub mod dkg_coordinator;  // Golden DKG for epoch-based threshold key generation (commonware-p2p) - OLD, will migrate
pub mod dkg_scheme_provider;  // Epoch-aware signing scheme from Golden DKG - OLD, will migrate
pub mod dkg_litep2p;  // Golden DKG over litep2p (MVP network) - OLD, will migrate
pub mod network;  // Network layer (litep2p TCP, QUIC future)

pub use application::{Actor as Application, Config as ApplicationConfig, Mailbox as ApplicationMailbox};
pub use block::Block;
pub use engine::{Config as EngineConfig, Engine};
pub use lending::{LendingPool, PoolState, Position, LendingAction};
pub use light_client::{LightClient, LightClientConfig, LigeritoSuccinctProof, extract_succinct_proof};
pub use dkg_coordinator::DKGCoordinator;
pub use dkg_scheme_provider::{DKGSchemeProvider, create_bootstrap_provider};
pub use governance::{DKGGovernanceManager, SlashingEvent, SlashingReason};

use commonware_consensus::marshal::SchemeProvider;
use commonware_consensus::simplex::signing_scheme::bls12381_threshold;
use commonware_cryptography::bls12381::primitives::{group, poly::Poly, variant::MinSig};
use commonware_cryptography::ed25519::PublicKey;
use serde::{Deserialize, Serialize};
use std::{collections::HashMap, net::SocketAddr, sync::Arc};

/// BLS12-381 variant for signatures
pub type Variant = MinSig;

/// Concrete signing scheme for consensus (BLS12-381 threshold signatures)
pub type SigningScheme = bls12381_threshold::Scheme<PublicKey, Variant>;

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
pub struct StaticSchemeProvider(pub Arc<SigningScheme>);

impl SchemeProvider for StaticSchemeProvider {
    type Scheme = SigningScheme;

    fn scheme(&self, _epoch: u64) -> Option<Arc<Self::Scheme>> {
        Some(self.0.clone())
    }
}

impl From<SigningScheme> for StaticSchemeProvider {
    fn from(scheme: SigningScheme) -> Self {
        Self(Arc::new(scheme))
    }
}
