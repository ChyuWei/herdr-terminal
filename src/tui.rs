//! Outer-terminal plumbing shared by the single-pane screen and the
//! workspace: the alternate-screen guard, display facts and VT error mapping.
//! Both screens own exactly one guard per process.

use crate::core::{Display, Error, Result};
use crate::term::{self, RawMode};
use crate::vt::{Color, Screen};
use crossterm::cursor::SetCursorStyle;
use crossterm::event::{
    DisableBracketedPaste, DisableFocusChange, DisableMouseCapture, EnableBracketedPaste,
    EnableFocusChange, EnableMouseCapture, KeyboardEnhancementFlags, PopKeyboardEnhancementFlags,
    PushKeyboardEnhancementFlags,
};
use crossterm::execute;
use crossterm::terminal::{EnterAlternateScreen, LeaveAlternateScreen};
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;
use std::io::stdout;

pub(crate) type ScreenTerminal = Terminal<CrosstermBackend<std::io::Stdout>>;

/// The ratatui terminal for this process.
pub(crate) fn terminal() -> Result<ScreenTerminal> {
    Terminal::new(CrosstermBackend::new(stdout())).map_err(Error::Io)
}

/// The viewer's display facts: the terminal's current default colors. A TUI
/// has no cell pixel size, so `cell` is left out.
pub(crate) fn display(screen: &Screen) -> Display {
    Display {
        fg: hex(screen.default_fg),
        bg: hex(screen.default_bg),
        cursor: None,
        cell: None,
    }
}

pub(crate) fn hex(color: Color) -> Option<String> {
    match color {
        Color::Rgb(r, g, b) => Some(format!("#{r:02x}{g:02x}{b:02x}")),
        Color::Default => None,
    }
}

pub(crate) fn vt_error(error: crate::vt::VtError) -> Error {
    Error::Protocol(error.to_string())
}

/// Owns the outer terminal: raw mode, the alternate screen, focus reporting,
/// keyboard enhancement, bracketed paste and mouse capture. `restore` is
/// idempotent and runs on drop, so every return path puts the terminal back.
pub(crate) struct ScreenGuard {
    raw: Option<RawMode>,
    alternate: bool,
    focus: bool,
    keyboard: bool,
    paste: bool,
    mouse: bool,
    restored: bool,
}

impl ScreenGuard {
    pub(crate) fn enter() -> Result<ScreenGuard> {
        let raw = RawMode::enter()?
            .ok_or_else(|| Error::Usage("ht needs a terminal on stdin and stdout".into()))?;
        let mut guard = ScreenGuard {
            raw: Some(raw),
            alternate: false,
            focus: false,
            keyboard: false,
            paste: false,
            mouse: false,
            restored: false,
        };
        let mut out = stdout();
        execute!(out, EnterAlternateScreen).map_err(Error::Io)?;
        guard.alternate = true;
        // Focus, keyboard enhancement and bracketed paste are nice-to-have: a
        // terminal that does not know them still renders and accepts input.
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
        // Bracketed paste framing is decided per program by its own DEC 2004
        // mode; this only lets the outer terminal report a paste as one event
        // instead of a burst of keys.
        if execute!(out, EnableBracketedPaste).is_ok() {
            guard.paste = true;
        }
        Ok(guard)
    }

    /// Turns the outer terminal's mouse reporting on or off to match the
    /// focused program's tracking mode.
    pub(crate) fn set_mouse(&mut self, on: bool) -> Result<()> {
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

    pub(crate) fn restore(&mut self) {
        if self.restored {
            return;
        }
        self.restored = true;
        self.leave();
    }

    /// Hands the outer terminal back for a child process (an editor): leave
    /// the alternate screen, raw mode, mouse, keyboard enhancement, focus and
    /// paste reporting. [`Self::resume`] puts the TUI back. Idempotent, and a
    /// guard that is dropped while suspended still restores cleanly.
    pub(crate) fn suspend(&mut self) {
        if self.restored {
            return;
        }
        self.leave();
    }

    /// Returns to the alternate screen and every mode the TUI needs after
    /// [`Self::suspend`]. A no-op before the guard is suspended or after it
    /// was restored.
    pub(crate) fn resume(&mut self) -> Result<()> {
        if self.restored || self.raw.is_some() {
            return Ok(());
        }
        let raw = RawMode::enter()?
            .ok_or_else(|| Error::Usage("ht needs a terminal on stdin and stdout".into()))?;
        self.raw = Some(raw);
        let mut out = stdout();
        execute!(out, EnterAlternateScreen).map_err(Error::Io)?;
        self.alternate = true;
        if execute!(out, EnableFocusChange).is_ok() {
            self.focus = true;
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
            self.keyboard = true;
        }
        if execute!(out, EnableBracketedPaste).is_ok() {
            self.paste = true;
        }
        Ok(())
    }

    fn leave(&mut self) {
        let mut out = stdout();
        if self.paste {
            let _ = execute!(out, DisableBracketedPaste);
            self.paste = false;
        }
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

/// Whether the local terminal can host a full-screen UI.
pub(crate) fn require_tty() -> Result<()> {
    use std::io::IsTerminal;
    if !std::io::stdin().is_terminal() || !std::io::stdout().is_terminal() {
        return Err(Error::Usage(
            "ht needs a terminal on stdin and stdout (use ht probe for pipes)".into(),
        ));
    }
    Ok(())
}

/// The window size, or the documented fallback.
pub(crate) fn viewport(options_cols: Option<u16>, options_rows: Option<u16>) -> (u16, u16) {
    let size = term::window_size();
    (
        options_cols
            .or(size.map(|(cols, _)| cols))
            .unwrap_or(crate::screen::DEFAULT_COLS),
        options_rows
            .or(size.map(|(_, rows)| rows))
            .unwrap_or(crate::screen::DEFAULT_ROWS),
    )
}
