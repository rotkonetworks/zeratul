//! poker state channels for ghettobox parachain
//!
//! off-chain p2p gameplay with on-chain dispute resolution
//! uses ligerito proofs for trustless shuffle verification

extern crate alloc;

pub mod channel;
pub mod dispute;
pub mod state;
pub mod transition;
pub mod types;

pub use channel::*;
pub use dispute::*;
pub use state::*;
pub use transition::*;
pub use types::*;
