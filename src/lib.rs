//! HerdR Terminal: the terminal client of Herdr Core.
//!
//! The reusable part is [`core`]: discovery, one long-lived framed
//! connection, calls, events and attach streams. [`probe`] is the M0 raw
//! byte path; [`screen`] is the M1/M2 single-pane client; [`workspace`] is the
//! M3 spaces/tabs/panes client. Both full-screen clients are built on [`vt`],
//! [`session`] and ratatui.

pub mod clipboard;
pub mod config;
pub mod core;
pub mod model;
pub mod probe;
pub mod screen;
pub mod session;
pub mod term;
pub mod theme;
mod tui;
pub mod vt;
pub mod workspace;
