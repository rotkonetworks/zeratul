//! poker-p2p: P2P networking for mental poker
//!
//! uses iroh for QUIC transport with NAT holepunching,
//! pkarr for mainline DHT discovery, and spake2 PAKE
//! for word-code authenticated connections.
//!
//! ## table creation flow
//!
//! ```text
//! host:   Table::create(rules) → "42-bison-lamp"
//! player: Table::join("42-bison-lamp") → sees rules → accept as Player/Spectator
//! ```
//!
//! ## roles
//!
//! - **Player**: opens channel, deposits buy-in, plays cards
//! - **Spectator**: watches game, receives delayed card reveals

pub mod rendezvous;
pub mod protocol;
pub mod table;
pub mod session;

pub use rendezvous::{generate_code, TableCode};
pub use protocol::*;
pub use table::{Table, TableHost, TableClient};
pub use session::Session;

/// ALPN protocol identifier
pub const ALPN: &[u8] = b"poker/1";
