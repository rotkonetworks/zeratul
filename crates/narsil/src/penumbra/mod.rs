//! penumbra integration for narsil syndicates
//!
//! a narsil syndicate on penumbra is:
//! - a decaf377 spending key split via OSST
//! - members hold shares, threshold required to sign
//! - actions limited to penumbra wallet capabilities
//! - all coordination happens via relays
//!
//! # key hierarchy
//!
//! ```text
//! syndicate spending key (OSST group key)
//!     │
//!     ├── full viewing key (shared with all members for scanning)
//!     │
//!     └── address (where syndicate receives funds)
//! ```
//!
//! members also have personal penumbra accounts used for:
//! - identity/authentication in relay layer
//! - contributing capital to syndicate
//! - receiving distributions
//!
//! # action flow
//!
//! ```text
//! 1. member proposes action (spend, swap, delegate)
//! 2. other members verify and vote (governance)
//! 3. approving members generate OSST contributions
//! 4. aggregate into threshold signature
//! 5. build penumbra transaction
//! 6. submit to chain (looks like normal tx)
//! ```
//!
//! # feature flags
//!
//! - `penumbra`: enables full transaction building with penumbra-sdk crates

pub mod action;
pub mod keys;
pub mod note;

#[cfg(feature = "penumbra")]
pub mod transaction;

pub use action::{SyndicateAction, ActionPlan, SpendPlan, SwapPlan, DelegatePlan};
pub use keys::{SyndicateKeys, MemberKeys, SyndicateId};
pub use note::{SyndicateNote, NoteSet};

#[cfg(feature = "penumbra")]
pub use transaction::{TransactionBuilder, TransactionError, AuthorizationData};
