//! Consensus Module
//!
//! Implements Safrole block production (JAM-style simplified SASSAFRAS)

pub mod safrole;
pub mod entropy;
pub mod tickets;

pub use safrole::{SafroleState, SafroleConfig};
pub use entropy::EntropyAccumulator;
pub use tickets::{SafroleTicket, TicketExtrinsic, SealTickets};
