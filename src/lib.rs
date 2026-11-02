//! HerdR Terminal: the terminal client of Herdr Core.
//!
//! The reusable part is [`core`]: discovery, one long-lived framed
//! connection, calls, events and attach streams. [`probe`] is the M0 raw
//! byte path; [`screen`] is the M1 full-screen client, built on [`vt`] and
//! ratatui.

pub mod core;
pub mod probe;
pub mod screen;
pub mod session;
pub mod term;
pub mod vt;
