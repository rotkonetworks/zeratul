//! network adapters for different blockchains
//!
//! each network has its own:
//! - osst curve (for threshold signatures)
//! - transaction format
//! - address scheme
//! - rpc interface
//!
//! # supported networks
//!
//! | network          | curve       | sig scheme    | notes                    |
//! |------------------|-------------|---------------|--------------------------|
//! | polkadot-ah      | ristretto255| sr25519-like  | asset hub, multi-asset   |
//! | penumbra         | decaf377    | native        | private defi             |
//! | zcash            | pallas      | sapling/orchard | shielded txs          |
//! | osmosis/noble    | secp256k1   | cosmos sdk    | ibc, usdc                |
//!
//! # usage
//!
//! ```ignore
//! use narsil::networks::polkadot::PolkadotAdapter;
//! use narsil::{SyndicateRuntime, ActionRegistry};
//!
//! let adapter = PolkadotAdapter::new("wss://asset-hub.polkadot.io");
//! let runtime = SyndicateRuntime::new(adapter, osst_scheme, state_backend);
//! ```

pub mod polkadot;
pub mod penumbra;
pub mod zcash;
pub mod cosmos;

// re-export adapters
pub use polkadot::PolkadotAdapter;
pub use penumbra::PenumbraAdapter;
pub use zcash::ZcashAdapter;
pub use cosmos::CosmosAdapter;

/// curve requirements for each network
pub mod curves {
    //! curve mappings for network integration
    //!
    //! each network requires a specific elliptic curve for threshold signatures.
    //! osst provides implementations for all supported curves.

    /// polkadot uses ristretto255 (schnorr signatures compatible with sr25519)
    #[cfg(feature = "ristretto255")]
    pub type PolkadotCurve = osst::Ristretto255;

    /// penumbra uses decaf377 (native curve)
    #[cfg(feature = "decaf377")]
    pub type PenumbraCurve = osst::Decaf377Curve;

    /// zcash uses pallas (orchard circuit)
    #[cfg(feature = "pallas")]
    pub type ZcashCurve = osst::PallasCurve;

    /// cosmos chains use secp256k1
    #[cfg(feature = "secp256k1")]
    pub type CosmosCurve = osst::Secp256k1Curve;
}
