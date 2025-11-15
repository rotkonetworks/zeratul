//! Core business logic - platform-agnostic
//! 
//! Following Crux architecture:
//! - Pure functions (no side effects)
//! - Serializable state
//! - Event-driven updates
//! - Effects for side effects

pub mod event;
pub mod effect;
pub mod app;
pub mod types;

pub use event::Event;
pub use effect::{Effect, NotificationLevel};
pub use app::AppCore;
pub use types::*;

// Re-export from penumbra_dex for convenience
pub use penumbra_dex::TradingPair;
