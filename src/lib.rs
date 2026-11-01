//! HerdR Terminal: the terminal client of Herdr Core.
//!
//! The reusable part is [`core`]: discovery, one long-lived framed
//! connection, calls, events and attach streams. [`probe`] is the M0 CLI
//! path that proves it; the TUI arrives in later milestones.

pub mod core;
pub mod probe;
pub mod term;
