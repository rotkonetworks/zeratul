//! # ghettobox
//!
//! pluggable pin-protected secret recovery using tpm and distributed shares.
//!
//! inspired by juicebox protocol but simplified for single-operator deployments
//! using commodity hardware (tpm) instead of distributed hsm infrastructure.
//!
//! ## architecture (distributed VSS mode)
//!
//! ```text
//! ┌─────────────────┐
//! │   email + PIN   │
//! └────────┬────────┘
//!          │ argon2id
//!          ▼
//!    ┌─────────────┐
//!    │  unlock key │
//!    └─────┬───────┘
//!          │
//!    ┌─────┴─────┐
//!    ▼     ▼     ▼
//!  ┌───┐ ┌───┐ ┌───┐
//!  │TPM│ │TPM│ │TPM│   (3 nodes, 2-of-3 threshold)
//!  │ 1 │ │ 2 │ │ 3 │
//!  └─┬─┘ └─┬─┘ └─┬─┘
//!    │     │     │
//!    └──┬──┴──┬──┘
//!       ▼     ▼
//!    ┌───────────┐
//!    │   seed    │  (VSS reconstruction)
//!    └─────┬─────┘
//!          │ hkdf
//!          ▼
//!    ┌───────────┐
//!    │  ed25519  │  (signing key)
//!    └───────────┘
//! ```
//!
//! ## security properties
//!
//! - pin never leaves client in usable form
//! - shares sealed to TPM hardware on each node
//! - 2-of-3 threshold: any 2 nodes can recover, 1 node leak is safe
//! - rate limiting enforced by TPM dictionary attack protection
//! - neither share alone reveals anything about secret
//!
//! ## usage
//!
//! ```rust,ignore
//! use ghettobox::{vss, account::Account, crypto};
//!
//! // generate seed and split into shares
//! let seed: [u8; 32] = crypto::random_bytes();
//! let shares = vss::split_secret(&seed)?;
//!
//! // distribute shares to 3 TPM nodes
//! // ... network calls ...
//!
//! // recover seed from 2 of 3 shares
//! let recovered = vss::combine_shares(&[shares[0], shares[1]])?;
//!
//! // derive account from seed
//! let account = Account::from_seed(&recovered)?;
//! println!("address: {}", account.address_hex());
//! ```

pub mod crypto;
pub mod error;
pub mod realm;
pub mod share;
pub mod protocol;
pub mod vss;
pub mod account;
pub mod client;

#[cfg(feature = "network")]
pub mod network;

pub use error::{Error, Result};
pub use protocol::Ghettobox;
pub use realm::Realm;
pub use share::Share;
pub use account::Account;
pub use client::Client;
pub use vss::{split_secret, combine_shares};

#[cfg(feature = "software")]
pub use realm::software::SoftwareRealm;

#[cfg(feature = "tpm")]
pub use realm::tpm::{TpmRealm, TpmInfo, TpmType};

#[cfg(feature = "network")]
pub use network::NetworkClient;
