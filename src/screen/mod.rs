//! The single-pane screen behind `ht screen`: one terminal, rendered with
//! ratatui.
//!
//! It is built from the M0 wire client, the M1 local Ghostty VT and one
//! `events.subscribe` stream. The VT and the pure [`crate::model::Model`] are
//! kept here; `play` owns the lifecycle state machine (attached, waiting for
//! a restarted shell, reconnecting).
//!
//! With no program argument the screen reattaches to the terminal a previous
//! client left running (a small pointer under `XDG_STATE_HOME`, or the single
//! live terminal carrying `herdr.client=herdr-terminal`). With a program it
//! starts a new kept terminal. Ctrl-] detaches: the program keeps running and
//! the terminal record stays, so a later `ht` finds it again. The terminal is
//! restored on every exit path by [`ScreenGuard`].

pub(crate) mod lifecycle;
mod play;
mod pointer;
pub(crate) mod view;

use crate::core::{CoreClient, Error, Result, RunOptions};
use crate::model::Model;
use crate::session::{self, Session};
use std::io::stdout;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

// The outer-terminal guard, facts and VT error mapping are shared with the
// workspace screen. `play` uses them through `super::`.
pub(crate) use crate::tui::{ScreenGuard, ScreenTerminal, display, vt_error};

/// Default attach size when there is no terminal and no explicit size.
pub const DEFAULT_COLS: u16 = 80;
pub const DEFAULT_ROWS: u16 = 24;

/// How long the loop waits for input when no output arrived.
const TICK: Duration = Duration::from_millis(10);
/// The most frames per second the screen redraws at.
const FRAME: Duration = Duration::from_millis(16);
/// The most attach bytes one loop iteration consumes before it handles input
/// and draws. A program that floods output must not starve the viewer.
const DRAIN_BYTES: usize = 256 * 1024;
/// The longest one drain may take, for floods of many small chunks.
const DRAIN_TIME: Duration = Duration::from_millis(8);

#[derive(Clone, Debug, Default)]
pub struct Options {
    pub cols: Option<u16>,
    pub rows: Option<u16>,
    pub hc: Option<PathBuf>,
    /// Program and arguments; empty starts core's shell or reattaches to the
    /// terminal a previous client left.
    pub argv: Vec<String>,
    pub verbose: bool,
}

/// Runs one full-screen terminal. Returns the program's exit code, or 0 when
/// the viewer detaches with Ctrl-].
pub fn run(options: Options) -> Result<i32> {
    crate::tui::require_tty()?;
    let (cols, rows) = crate::tui::viewport(options.cols, options.rows);

    let client = Arc::new(CoreClient::connect(options.hc.as_deref())?);
    let events = client.subscribe()?;
    let model = Model::from_snapshot(events.snapshot());
    let core_id = client.hello().core.id.clone();
    let mut session = prepare_target(&options, &client, &model, &core_id, (cols, rows))?;
    // The pointer lets the next client find this terminal even when it was
    // discovered by its label; it is not the M4 workspace document.
    pointer::pointer(&core_id, session.address()).save();
    let first = match session.attach(cols, rows, 0) {
        Ok(attached) => play::First::Attached(Box::new(attached)),
        Err(Error::Rpc { error, .. }) if error.kind() == Some("finished") => {
            // The program ended before attach landed. There is no TUI to
            // restore yet, so print its final screen and return its status.
            let result = session::recover_finished(&mut session, &mut stdout(), &error);
            if result.is_ok() {
                pointer::pointer(&core_id, session.address()).clear();
            }
            return result;
        }
        Err(Error::Rpc { error, .. }) if error.kind() == Some("unavailable") => {
            // A kept terminal is parked until core starts its fresh shell.
            play::First::Waiting
        }
        Err(error) => return Err(initial_error(&mut session, &core_id, error)),
    };

    let mut guard = ScreenGuard::enter()?;
    let screen = play::Screen::new(
        cols, rows, &options, &mut guard, client, events, model, session,
    )?;
    let (result, mut session, core_id) = screen.run(first);
    match result {
        Ok(play::Outcome::Program { reason, record }) => {
            let code = reason
                .as_ref()
                .map(|reason| session::exit_code_from(Some(reason)))
                .or_else(|| record.as_deref().map(session::record_exit_code))
                .unwrap_or(0);
            if let Err(error) = session.remove_finished() {
                eprintln!(
                    "ht: warning: could not remove terminal {}: {error}",
                    session.address()
                );
            }
            pointer::pointer(&core_id, session.address()).clear();
            Ok(code)
        }
        Ok(play::Outcome::Quit) => {
            // Detach only: the program keeps running for the next `ht`.
            let _ = session.detach();
            Ok(0)
        }
        Ok(play::Outcome::Removed) => {
            let address = session.address().to_owned();
            session.lost();
            pointer::pointer(&core_id, &address).clear();
            Err(Error::Protocol(format!("terminal {address} was removed")))
        }
        Err(error) => Err(finish_error(&mut session, &core_id, error)),
    }
}

/// Chooses the terminal to show: the program argument starts a new one;
/// otherwise the pointer or a single labelled live terminal is adopted.
fn prepare_target(
    options: &Options,
    client: &Arc<CoreClient>,
    model: &Model,
    core_id: &str,
    size: (u16, u16),
) -> Result<Session> {
    if !options.argv.is_empty() {
        let run = RunOptions {
            argv: options.argv.clone(),
            keep: true,
            rm: false,
            size: Some(size),
            ..RunOptions::default()
        };
        return Session::start(client.clone(), &run);
    }
    let saved = pointer::Pointer::load();
    match pointer::choose(model, saved.as_ref(), core_id) {
        pointer::Choice::Existing(address) => Ok(Session::adopt(client.clone(), address)),
        pointer::Choice::New => {
            let run = RunOptions {
                argv: Vec::new(),
                keep: true,
                rm: false,
                size: Some(size),
                ..RunOptions::default()
            };
            Session::start(client.clone(), &run)
        }
        pointer::Choice::Ambiguous(addresses) => Err(Error::Protocol(format!(
            "found several live terminals labelled {label}={name} ({}); cannot tell which one \
             to reattach, and starting another would duplicate one of them. Kill or remove the \
             extra terminals (hc kill ADDRESS, hc rm ADDRESS) and try again",
            addresses.join(", "),
            label = pointer::CLIENT_LABEL,
            name = pointer::CLIENT_NAME,
        ))),
    }
}

/// A failure before the TUI started: clean up an owned terminal, leave an
/// adopted one running, and point the error message at the cause.
fn initial_error(session: &mut Session, core_id: &str, error: Error) -> Error {
    if session::is_lost(&error) {
        session.lost();
    } else {
        let owned = session.owned();
        let _ = session.cleanup();
        if owned {
            pointer::pointer(core_id, session.address()).clear();
        }
    }
    error
}

/// A failure after the TUI started: cleanup follows the same owned/adopted
/// rule, and a lost connection leaves the program alone.
fn finish_error(session: &mut Session, core_id: &str, error: Error) -> Error {
    if session::is_lost(&error) {
        session.lost();
        return error;
    }
    let owned = session.owned();
    if let Err(cleanup) = session.cleanup() {
        eprintln!(
            "ht: warning: could not clean up terminal {}: {cleanup}",
            session.address()
        );
    }
    if owned {
        pointer::pointer(core_id, session.address()).clear();
    }
    error
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::vt::{Color, Screen};

    #[test]
    fn facts_are_hex_colors_and_omit_unknown_values() {
        let screen = Screen {
            default_fg: Color::Rgb(0x12, 0x34, 0x56),
            default_bg: Color::Rgb(0xab, 0xcd, 0xef),
            ..Screen::default()
        };
        let facts = display(&screen);
        assert_eq!(facts.fg.as_deref(), Some("#123456"));
        assert_eq!(facts.bg.as_deref(), Some("#abcdef"));
        assert_eq!(facts.cursor, None);
        assert_eq!(facts.cell, None);
    }
}
