//! The default single-pane screen: one terminal, rendered with ratatui.
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

mod lifecycle;
mod play;
mod pointer;
mod view;

use crate::core::{CoreClient, Display, Error, Result, RunOptions};
use crate::model::Model;
use crate::session::{self, Session};
use crate::term::{self, RawMode};
use crate::vt::{Color, Screen};
use crossterm::cursor::SetCursorStyle;
use crossterm::event::{
    DisableFocusChange, DisableMouseCapture, EnableFocusChange, EnableMouseCapture,
    KeyboardEnhancementFlags, PopKeyboardEnhancementFlags, PushKeyboardEnhancementFlags,
};
use crossterm::execute;
use crossterm::terminal::{EnterAlternateScreen, LeaveAlternateScreen};
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;
use std::io::{IsTerminal, stdout};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

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

type ScreenTerminal = Terminal<CrosstermBackend<std::io::Stdout>>;

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
    if !std::io::stdin().is_terminal() || !std::io::stdout().is_terminal() {
        return Err(Error::Usage(
            "ht needs a terminal on stdin and stdout (use ht probe for pipes)".into(),
        ));
    }
    let size = term::window_size();
    let cols = options
        .cols
        .or(size.map(|(cols, _)| cols))
        .unwrap_or(DEFAULT_COLS);
    let rows = options
        .rows
        .or(size.map(|(_, rows)| rows))
        .unwrap_or(DEFAULT_ROWS);

    let client = Arc::new(CoreClient::connect(options.hc.as_deref())?);
    let events = client.subscribe()?;
    let model = Model::from_snapshot(events.snapshot());
    let core_id = client.hello().core.id.clone();
    let mut session = prepare_target(&options, &client, &model, &core_id)?;
    // The pointer lets the next client find this terminal even when it was
    // discovered by its label; it is not the M4 workspace document.
    pointer::pointer(&core_id, session.address()).save();
    let first = match session.attach(cols, rows) {
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
) -> Result<Session> {
    if !options.argv.is_empty() {
        let run = RunOptions {
            argv: options.argv.clone(),
            keep: true,
            rm: false,
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

/// The viewer's display facts: the terminal's current default colors. A TUI
/// has no cell pixel size, so `cell` is left out.
fn display(screen: &Screen) -> Display {
    Display {
        fg: hex(screen.default_fg),
        bg: hex(screen.default_bg),
        cursor: None,
        cell: None,
    }
}

fn hex(color: Color) -> Option<String> {
    match color {
        Color::Rgb(r, g, b) => Some(format!("#{r:02x}{g:02x}{b:02x}")),
        Color::Default => None,
    }
}

fn vt_error(error: crate::vt::VtError) -> Error {
    Error::Protocol(error.to_string())
}

/// Owns the outer terminal: raw mode, the alternate screen, focus reporting,
/// keyboard enhancement and mouse capture. `restore` is idempotent and runs
/// on drop, so every return path puts the terminal back.
struct ScreenGuard {
    raw: Option<RawMode>,
    alternate: bool,
    focus: bool,
    keyboard: bool,
    mouse: bool,
    restored: bool,
}

impl ScreenGuard {
    fn enter() -> Result<ScreenGuard> {
        let raw = RawMode::enter()?
            .ok_or_else(|| Error::Usage("ht needs a terminal on stdin and stdout".into()))?;
        let mut guard = ScreenGuard {
            raw: Some(raw),
            alternate: false,
            focus: false,
            keyboard: false,
            mouse: false,
            restored: false,
        };
        let mut out = stdout();
        execute!(out, EnterAlternateScreen).map_err(Error::Io)?;
        guard.alternate = true;
        // Focus and keyboard enhancement are nice-to-have: a terminal that
        // does not know them still renders and accepts input.
        if execute!(out, EnableFocusChange).is_ok() {
            guard.focus = true;
        }
        if execute!(
            out,
            PushKeyboardEnhancementFlags(
                KeyboardEnhancementFlags::DISAMBIGUATE_ESCAPE_CODES
                    | KeyboardEnhancementFlags::REPORT_EVENT_TYPES
            )
        )
        .is_ok()
        {
            guard.keyboard = true;
        }
        Ok(guard)
    }

    /// Turns the outer terminal's mouse reporting on or off to match the
    /// program's tracking mode.
    fn set_mouse(&mut self, on: bool) -> Result<()> {
        if self.mouse == on {
            return Ok(());
        }
        let mut out = stdout();
        if on {
            execute!(out, EnableMouseCapture).map_err(Error::Io)?;
        } else {
            execute!(out, DisableMouseCapture).map_err(Error::Io)?;
        }
        self.mouse = on;
        Ok(())
    }

    fn restore(&mut self) {
        if self.restored {
            return;
        }
        self.restored = true;
        let mut out = stdout();
        if self.mouse {
            let _ = execute!(out, DisableMouseCapture);
            self.mouse = false;
        }
        if self.keyboard {
            let _ = execute!(out, PopKeyboardEnhancementFlags);
            self.keyboard = false;
        }
        if self.focus {
            let _ = execute!(out, DisableFocusChange);
            self.focus = false;
        }
        // Ratatui and the program's DECSCUSR both change the outer cursor.
        // Show it again and hand the shape back to the user's default: there
        // is no portable query for the shape or visibility from before.
        let _ = execute!(
            out,
            crossterm::cursor::Show,
            SetCursorStyle::DefaultUserShape
        );
        if self.alternate {
            let _ = execute!(out, LeaveAlternateScreen);
            self.alternate = false;
        }
        // Restores the termios captured before raw mode.
        self.raw.take();
    }
}

impl Drop for ScreenGuard {
    fn drop(&mut self) {
        self.restore();
    }
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
