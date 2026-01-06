//! ligerito shielded pool
//!
//! penumbra-style utxo model with ligerito proofs for p2p shielded rollups
//!
//! # architecture
//!
//! ```text
//! ┌─────────────────────────────────────────────────────────────┐
//! │                    P2P SHIELDED ROLLUP                       │
//! ├─────────────────────────────────────────────────────────────┤
//! │                                                              │
//! │  L1 (chain)                                                 │
//! │  ├─ note commitment tree (merkle root only)                 │
//! │  ├─ nullifier set (spent notes)                             │
//! │  └─ xcm bridge for deposits/withdrawals                     │
//! │                                                              │
//! │  L2 (p2p between users)                                     │
//! │  ├─ state channels with per-action proofs                   │
//! │  ├─ ligerito proofs (~20ms prove, ~5ms verify)              │
//! │  └─ settlement: update notes on L1                          │
//! │                                                              │
//! └─────────────────────────────────────────────────────────────┘
//! ```

#![cfg_attr(not(feature = "std"), no_std)]

#[cfg(not(feature = "std"))]
extern crate alloc;

pub mod note;
pub mod nullifier;
pub mod commitment;
pub mod keys;
pub mod value;
pub mod channel;
pub mod proof;
pub mod poker;
pub mod dispute;

pub use note::{Note, NoteCommitment};
pub use nullifier::Nullifier;
pub use commitment::StateCommitmentTree;
pub use keys::{SpendKey, ViewKey, Address};
pub use value::{Value, AssetId, Amount};
pub use channel::{Channel, ChannelState, Action};
pub use proof::{SpendProof, OutputProof, StateTransitionProof};
pub use poker::{PokerGameState, PokerAction, BetAction, GamePhase, PokerChannel};
pub use dispute::{DisputeProof, DisputeType, DisputeBuilder, DisputeVerdict};

/// domain separator for note commitments
pub const NOTE_DOMAIN: &[u8] = b"ligerito.shielded-pool.note.v1";
/// domain separator for nullifiers
pub const NULLIFIER_DOMAIN: &[u8] = b"ligerito.shielded-pool.nullifier.v1";
/// domain separator for state transitions
pub const STATE_DOMAIN: &[u8] = b"ligerito.shielded-pool.state.v1";
