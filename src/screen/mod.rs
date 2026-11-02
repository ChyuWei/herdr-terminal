//! `ht screen`: one full-screen terminal, rendered with ratatui.
//!
//! This is the M1 client. It uses the M0 [`CoreClient`] and one attach
//! stream: core's first paint and live bytes are fed into a local Ghostty VT
//! ([`crate::vt`]), the visible grid is drawn with ratatui, and keys, mouse,
//! resize and focus are sent back. The program itself never knows a TUI is in
//! front of it.
//!
//! Ctrl-] leaves the screen (detaches, ends the program and removes its
//! record); every other key goes to the program, including Ctrl-C. The
//! terminal is restored on every exit path by [`ScreenGuard`].

mod view;

use crate::core::{Attached, CoreClient, Display, Error, Result, RunOptions, StreamItem};
use crate::session::{self, Session};
use crate::term::{self, RawMode};
use crate::vt::{self, Color, Screen, Vt};
use crossterm::cursor::SetCursorStyle;
use crossterm::event::{
    self, DisableFocusChange, DisableMouseCapture, EnableFocusChange, EnableMouseCapture, Event,
    KeyboardEnhancementFlags, PopKeyboardEnhancementFlags, PushKeyboardEnhancementFlags,
};
use crossterm::execute;
use crossterm::terminal::{EnterAlternateScreen, LeaveAlternateScreen};
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;
use serde_json::Value;
use std::io::{IsTerminal, stdout};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::mpsc::TryRecvError;
use std::time::{Duration, Instant};

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
    /// Program and arguments; empty starts core's own shell.
    pub argv: Vec<String>,
    pub verbose: bool,
}

/// Runs one full-screen terminal. Returns the program's exit code, or 0 when
/// the viewer detaches with Ctrl-].
pub fn run(options: Options) -> Result<i32> {
    if !std::io::stdin().is_terminal() || !std::io::stdout().is_terminal() {
        return Err(Error::Usage(
            "ht screen needs a terminal on stdin and stdout (use ht probe for pipes)".into(),
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
    let run = RunOptions {
        argv: options.argv.clone(),
        // Keep the finished record until this screen has read it: attach can
        // lose the race with a program that exits at once.
        rm: false,
        ..RunOptions::default()
    };
    let mut session = Session::start(client, &run)?;
    let attached = match session.attach(cols, rows) {
        Ok(attached) => attached,
        Err(Error::Rpc { error, .. }) if error.kind() == Some("finished") => {
            // The program ended before attach landed. There is no TUI to
            // restore yet, so print its final screen and return its status.
            return session::recover_finished(&mut session, &mut stdout(), &error);
        }
        Err(error) => return Err(error),
    };
    if options.verbose {
        eprintln!(
            "attached to {} on channel {} at {cols}x{rows}",
            session.address(),
            attached.channel
        );
    }

    let mut guard = ScreenGuard::enter()?;
    let result = play(&mut session, attached, &mut guard, Watch { cols, rows });
    match result {
        Ok(Exit::Program(reason)) => {
            let code = session::exit_code_from(reason.as_ref());
            if let Err(error) = session.remove_finished() {
                eprintln!(
                    "ht: warning: could not remove terminal {}: {error}",
                    session.address()
                );
            }
            Ok(code)
        }
        Ok(Exit::Quit) => {
            session.end()?;
            Ok(0)
        }
        Err(error) => {
            if session::is_lost(&error) {
                // Core is gone, not the program: leave it running.
                session.lost();
            }
            Err(error)
        }
    }
}

/// How the screen loop ended.
enum Exit {
    /// Core closed the stream; the JSON reason carries the program's status.
    Program(Option<Value>),
    /// The person pressed Ctrl-].
    Quit,
}

struct Watch {
    cols: u16,
    rows: u16,
}

/// Feeds core bytes into the VT, draws, and forwards input until the program
/// ends or the viewer detaches.
fn play(
    session: &mut Session,
    attached: Attached,
    guard: &mut ScreenGuard,
    watch: Watch,
) -> Result<Exit> {
    let channel = attached.channel;
    let stream = attached.stream;
    let mut vt = Vt::new(watch.cols, watch.rows).map_err(vt_error)?;
    let mut terminal: ScreenTerminal =
        Terminal::new(CrosstermBackend::new(stdout())).map_err(Error::Io)?;

    let mut dirty = true;
    let mut last_draw = Instant::now() - FRAME;
    let mut last_facts: Option<Display> = None;
    let mut last_cursor_style: Option<SetCursorStyle> = None;
    let mut mouse_on = false;
    // Show the first paint before a fast program's live output can scroll it
    // away; a flood must not hide the screen that attach handed us.
    let mut drew_first_frame = false;

    loop {
        // Take what core has sent, but only up to a budget: a program that
        // never stops writing must not keep input and drawing from running.
        let mut had_data = false;
        let mut closing = None;
        let mut drained = 0usize;
        let drain_start = Instant::now();
        loop {
            match stream.try_recv() {
                Ok(StreamItem::Data(bytes)) => {
                    had_data = true;
                    drained = drained.saturating_add(bytes.len());
                    vt.feed(&bytes);
                    session.client().credit(channel, bytes.len())?;
                    dirty = true;
                    if !drew_first_frame
                        || drained >= DRAIN_BYTES
                        || drain_start.elapsed() >= DRAIN_TIME
                    {
                        break;
                    }
                }
                Ok(StreamItem::Closed(reason)) => {
                    closing = Some(reason);
                    break;
                }
                Ok(StreamItem::Lost) | Err(TryRecvError::Disconnected) => {
                    // Core is gone, not the program: leave it running.
                    session.lost();
                    return Err(session::lost_error());
                }
                Err(TryRecvError::Empty) => break,
            }
        }

        if let Some(reason) = closing {
            // Bytes fed in the same drain as CLOSE are still the program's
            // last output. Force the final frame past the throttle so they
            // reach the outer terminal before it is restored.
            draw_frame(
                &mut terminal,
                &mut vt,
                session.client(),
                channel,
                &mut last_cursor_style,
                &mut last_facts,
                false,
            )?;
            return Ok(Exit::Program(reason));
        }

        // The program's mouse mode decides whether the outer terminal should
        // report the mouse at all; the encoder filters what it receives.
        let tracking = vt.mouse_tracking();
        if tracking != mouse_on {
            guard.set_mouse(tracking)?;
            mouse_on = tracking;
        }

        let timeout = if had_data { Duration::ZERO } else { TICK };
        if event::poll(timeout).map_err(Error::Io)? {
            match event::read().map_err(Error::Io)? {
                Event::Key(key) => {
                    if vt::is_detach(&key) {
                        return Ok(Exit::Quit);
                    }
                    let bytes = vt.encode_key(&key);
                    if !bytes.is_empty() {
                        session.client().keys(channel, &bytes)?;
                    }
                }
                Event::Mouse(mouse) => {
                    let bytes = vt.encode_mouse(&mouse);
                    if !bytes.is_empty() {
                        session.client().keys(channel, &bytes)?;
                    }
                }
                Event::Resize(cols, rows) if cols > 0 && rows > 0 => {
                    vt.resize(cols, rows).map_err(vt_error)?;
                    session.client().resize(channel, cols, rows)?;
                    dirty = true;
                }
                Event::FocusGained => session.client().focus(channel, true)?,
                Event::FocusLost => session.client().focus(channel, false)?,
                _ => {}
            }
        }

        // The first frame that follows attach is drawn at once, before a
        // flood can scroll it away; later frames keep the frame throttle.
        let first_frame = !drew_first_frame && had_data;
        if dirty && (first_frame || last_draw.elapsed() >= FRAME) {
            draw_frame(
                &mut terminal,
                &mut vt,
                session.client(),
                channel,
                &mut last_cursor_style,
                &mut last_facts,
                true,
            )?;
            last_draw = Instant::now();
            dirty = false;
            if first_frame {
                drew_first_frame = true;
            }
        }
    }
}

/// Draws one snapshot, updates the outer cursor shape, and reports display
/// facts. `send_facts` is false for the final frame of a finished program:
/// the stream is closing, and the frame matters more than its metadata.
fn draw_frame(
    terminal: &mut ScreenTerminal,
    vt: &mut Vt,
    client: &CoreClient,
    channel: u32,
    last_cursor_style: &mut Option<SetCursorStyle>,
    last_facts: &mut Option<Display>,
    send_facts: bool,
) -> Result<()> {
    let screen = vt.snapshot().map_err(vt_error)?;
    terminal
        .draw(|frame| view::draw(frame, &screen))
        .map_err(Error::Io)?;
    let shape = view::cursor_shape(&screen.cursor);
    if *last_cursor_style != Some(shape) {
        execute!(stdout(), shape).map_err(Error::Io)?;
        *last_cursor_style = Some(shape);
    }
    if send_facts {
        let facts = display(&screen);
        if last_facts.as_ref() != Some(&facts) {
            client.facts(channel, &facts)?;
            *last_facts = Some(facts);
        }
    }
    Ok(())
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

fn vt_error(error: vt::VtError) -> Error {
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
            .ok_or_else(|| Error::Usage("ht screen needs a terminal on stdin and stdout".into()))?;
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
