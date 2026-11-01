//! HerdR Terminal: the terminal client for Herdr Core.
//!
//! The TUI is not implemented yet. Phase 0 starts with the core connection
//! and one attached terminal; see `docs/02-architecture.md`.

fn main() {
    println!(
        "HerdR Terminal {}\n\nThe terminal client for Herdr Core.\nThis is the design-phase skeleton; the TUI is not implemented yet.",
        env!("CARGO_PKG_VERSION")
    );
}
