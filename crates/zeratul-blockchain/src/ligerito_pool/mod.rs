//! ligerito-based shielded pool for zeratul
//!
//! fast (~20ms) proof generation for p2p shielded transactions
//! uses ligerito instead of groth16 for ~100x speedup
//!
//! ## architecture
//!
//! ```text
//! ┌─────────────────────────────────────────────────────────────┐
//! │                  LIGERITO SHIELDED POOL                     │
//! ├─────────────────────────────────────────────────────────────┤
//! │                                                              │
//! │  On-Chain State (NOMT)                                      │
//! │  ├─ note_commitments: merkle tree root                      │
//! │  ├─ nullifier_set: spent notes                              │
//! │  └─ pending_channels: active p2p channels                   │
//! │                                                              │
//! │  Actions                                                     │
//! │  ├─ Deposit: XCM → shielded note                            │
//! │  ├─ Withdraw: shielded note → XCM                           │
//! │  ├─ Transfer: spend + output (private)                      │
//! │  ├─ OpenChannel: create p2p state channel                   │
//! │  └─ SettleChannel: finalize channel state                   │
//! │                                                              │
//! │  Proof Verification                                          │
//! │  └─ PolkaVM verifier (ligerito proofs)                      │
//! │                                                              │
//! └─────────────────────────────────────────────────────────────┘
//! ```

pub mod component;
pub mod state;
pub mod actions;
pub mod channel_manager;

pub use component::LigeritoPool;
pub use state::{PoolState, StateReadExt, StateWriteExt};
pub use actions::{PoolAction, ActionResult};
pub use channel_manager::ChannelManager;

use ligerito_shielded_pool::{
    note::NoteCommitment,
    nullifier::Nullifier,
    commitment::StateRoot,
    channel::ChannelId,
    proof::{SpendProof, OutputProof, StateTransitionProof},
};

/// domain separator for pool state
pub const POOL_STATE_KEY: &[u8] = b"ligerito_pool::state";

/// domain separator for nullifier set
pub const NULLIFIER_SET_KEY: &[u8] = b"ligerito_pool::nullifiers";

/// domain separator for channels
pub const CHANNEL_KEY: &[u8] = b"ligerito_pool::channels";
