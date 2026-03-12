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

pub mod engine;
pub mod rendezvous;
pub mod protocol;
pub mod table;
pub mod session;
pub mod webrtc;

pub use rendezvous::{
    generate_code, TableCode, TableVisibility, PublicTableEntry,
    register_public_table, list_public_tables, unregister_public_table,
};
pub use protocol::*;
pub use table::{Table, TableHost, TableClient, TableEvent, TableError};
pub use session::Session;
pub use webrtc::{RtcManager, RtcEvent, IceConnectionState, AudioProcessor};

/// ALPN protocol identifier
pub const ALPN: &[u8] = b"poker/1";
