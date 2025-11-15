//! Lending Pool Module
//!
//! This module implements a privacy-preserving multi-asset lending pool
//! that enables margin trading on Penumbra assets.
//!
//! ## Key Features
//!
//! - **Privacy**: All positions are encrypted commitments in NOMT
//! - **Multi-asset**: Support for any Penumbra asset via IBC
//! - **Dynamic rates**: Interest rates adjust based on utilization
//! - **Cross-collateral**: Use multiple assets as collateral
//! - **MEV-resistant**: Batch liquidations prevent front-running
//!
//! ## Architecture
//!
//! ```text
//! User Action → ZK Proof → Batch Execution → NOMT Update
//!      ↓            ↓              ↓              ↓
//!   Supply      Verify      Pool State      Encrypted
//!   Borrow      Proof       Update          Commitment
//!   Repay       Valid       Interest        Storage
//!   Withdraw                Accrual
//! ```

pub mod actions;
pub mod margin;
pub mod privacy;
pub mod types;
pub mod liquidation;

pub use actions::*;
pub use margin::*;
pub use privacy::*;
pub use types::*;
pub use liquidation::*;
