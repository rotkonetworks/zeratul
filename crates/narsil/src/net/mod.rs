//! networking for syndicate coordination
//!
//! relay-based coordination using HTTP/WebSocket.
//! members post/fetch from public relays using pseudonymous mailboxes.
//! this prevents metadata leakage from direct P2P connections.
//!
//! # architecture
//!
//! ```text
//! ┌─────────┐     HTTPS      ┌─────────┐     HTTPS      ┌─────────┐
//! │ alice   │ ◄────────────► │  RELAY  │ ◄────────────► │  bob    │
//! │         │                │         │                │         │
//! │ mailbox │                │ mailbox │                │ mailbox │
//! │ POST/GET│                │ storage │                │ POST/GET│
//! └─────────┘                └─────────┘                └─────────┘
//! ```
//!
//! # components
//!
//! - `RelayClient`: trait for relay backends (HTTP, IPFS, S3)
//! - `MockRelay`: in-memory testing implementation
//! - `HttpRelayClient`: reqwest-based HTTP client (native)
//!
//! # message types
//!
//! - **Proposal**: member proposes an action
//! - **Vote**: member votes on proposal (governance)
//! - **Contribution**: OSST contribution for approved action
//! - **Sync**: state synchronization between members

// relay-based coordination
pub mod relay;
pub use relay::{RelayMessage, FetchOptions, RelayError, RelayErrorDetail};
#[cfg(feature = "std")]
pub use relay::MockRelay;
#[cfg(feature = "net")]
pub use relay::{RelayClient, BroadcastSubscription, HttpRelayClient};

// peer info (for tracking syndicate members)
#[cfg(feature = "net")]
pub mod peer;
#[cfg(feature = "net")]
pub mod message;

#[cfg(feature = "net")]
pub use peer::{PeerInfo, PeerId};
#[cfg(feature = "net")]
pub use message::{SyndicateMessage, MessageType};

/// protocol version for narsil relay messages
pub const PROTOCOL_VERSION: u8 = 1;
