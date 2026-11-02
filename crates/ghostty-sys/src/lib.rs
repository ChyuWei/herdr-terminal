//! Raw bindings to the upstream Ghostty VT C library.
//!
//! The safe wrapper lives in the `herdr_terminal::vt` module; this crate only
//! builds the static library from `vendor/ghostty` and exposes its C API.

#![allow(
    non_camel_case_types,
    non_snake_case,
    non_upper_case_globals,
    dead_code,
    clippy::all
)]

include!(concat!(env!("OUT_DIR"), "/bindings.rs"));
