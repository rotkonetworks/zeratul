//! chain-client: substrate chain interaction for ghettobox poker
//!
//! supports both RPC (default) and light client modes
//!
//! ## endpoints
//!
//! - ghettobox: `wss://ghettobox.rotko.net`
//! - asset hub polkadot: `wss://asset-hub-polkadot.rotko.net`
//!
//! ## usage
//!
//! ```rust,ignore
//! // RPC mode (default, faster startup)
//! let client = ChainClient::connect_rpc("wss://ghettobox.rotko.net").await?;
//!
//! // Light client mode (trustless, slower startup)
//! let client = ChainClient::connect_light("ghettobox").await?;
//! ```

pub mod client;
pub mod config;
pub mod error;
pub mod ibc;
pub mod transfers;
pub mod xcm;

pub use client::*;
pub use config::*;
pub use error::*;
pub use ibc::*;
pub use transfers::*;
