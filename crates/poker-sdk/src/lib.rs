//! poker-sdk: client library for ghettobox mental poker
//!
//! provides:
//! - key derivation (penumbra-style hierarchy)
//! - encryption for operators (x25519 + chacha20poly1305)
//! - ligerito proof generation for anti-spam
//! - transaction building helpers
//!
//! ## key hierarchy
//!
//! ```text
//! master_seed (email+PIN via OPRF)
//!     └─ spend_key (full authority)
//!          ├─ nullifier_key (nk) - for card/bet nullifiers
//!          ├─ auth_key (ak) - for signing
//!          └─ full_viewing_key (fvk = ak + nk)
//!               ├─ outgoing_viewing_key (ovk) - see sent actions
//!               ├─ incoming_viewing_key (ivk) - see received cards
//!               ├─ diversifier_key (dk) - derive table addresses
//!               └─ detection_key (dtk) - probabilistic scanning
//!
//! per-table:
//!     table_seed = prf(fvk, table_id)
//!         └─ table_viewing_key
//!              └─ per-hand keys
//!
//! per-hand:
//!     hand_seed = prf(table_key, hand_number)
//!         ├─ hand_viewing_key (share for review)
//!         └─ card_keys (mental poker layer)
//! ```

pub mod keys;
pub mod encrypt;
pub mod proof;
pub mod types;
pub mod hand;

pub use keys::*;
pub use encrypt::*;
pub use proof::*;
pub use types::*;
pub use hand::*;
