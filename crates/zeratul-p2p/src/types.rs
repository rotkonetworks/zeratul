//! Common P2P types

use serde::{Deserialize, Serialize};

/// Peer identifier
pub type PeerId = litep2p::PeerId;

/// Generic message trait
pub trait Message: Serialize + for<'de> Deserialize<'de> + Send + Sync + 'static {}

impl<T> Message for T where T: Serialize + for<'de> Deserialize<'de> + Send + Sync + 'static {}
