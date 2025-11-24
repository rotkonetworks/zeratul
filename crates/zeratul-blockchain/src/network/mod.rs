//! Zeratul Network Layer
//!
//! Dual-transport architecture:
//! - **QUIC (JAMNP-S)**: Validator-to-validator P2P network
//! - **HTTP/WebSocket**: Light client access (future)
//!
//! ## Architecture
//!
//! ```text
//! Validators (QUIC)          Light Clients (HTTP/WS)
//!     │                              │
//!     ├─ UP 0: Block announcements   │
//!     ├─ CE 128: Block requests      ├─ GET /block/{hash}
//!     ├─ CE 129: State requests      ├─ GET /state/{key}
//!     ├─ CE 200: DKG broadcasts      ├─ WS /subscribe/blocks
//!     └─ ... (JAM protocols)         └─ POST /tx/submit
//! ```

pub mod quic;      // QUIC transport (JAMNP-S)
pub mod streams;   // Stream protocols (UP/CE)
pub mod dkg;       // DKG over QUIC (CE 200-202)
pub mod types;     // Common network types
pub mod crypto_compat; // Crypto type wrappers
pub mod protocols; // Block sync, ticket gossip
pub mod time_sync; // Time synchronization

pub use quic::{NetworkService, NetworkConfig, NetworkHandles};
pub use streams::{StreamKind, StreamHandler};
pub use dkg::{DKGProtocol, DKGBroadcast};
pub use types::{ValidatorId, PeerId, Message};
pub use crypto_compat::{Ed25519PublicKey, Ed25519PrivateKey, BlsPublicKey};
pub use protocols::{NetworkMessage, BlockAnnounce, BlockRequest, BlockResponse, BlockSync, Vote, FinalityCertificate, ConsensusMessage};
pub use time_sync::{TimeSync, TimeSyncConfig};
