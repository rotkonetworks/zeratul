//! Terminator - Trading Terminal
//! 
//! Crux-style architecture:
//! - Core: Platform-agnostic business logic
//! - Shell: UI implementations (TUI/GUI/Web)

// Core - platform agnostic
pub mod core;

// Shell implementations
pub mod shell;

// Capabilities (side effects) - moved to shell/tui/executor
// pub mod capabilities;

// Integrations
pub mod network;
pub mod wallet;
pub mod command;

// Re-exports for convenience
pub use core::{AppCore, Event, Effect, ViewModel};
