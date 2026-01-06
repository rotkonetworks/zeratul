//! proactive secret sharing (pss) module
//!
//! uses zeratul/osst for threshold crypto:
//! - OSST verification (non-interactive threshold proofs)
//! - reshare protocol (dealer/aggregator pattern)
//!
//! uses HTTP for provider coordination (same as vault API)

pub mod client;
pub mod config;
pub mod http;
pub mod recovery;
pub mod reshare;
