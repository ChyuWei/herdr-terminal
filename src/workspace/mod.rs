//! The workspace: spaces, tabs and panes over one core connection, persisted
//! in the `herdr-terminal` core document and restored on the next start.
//!
//! The pure layers are [`model`] (spaces, tabs, layouts, stable ids, labels,
//! the shared document and the local view), [`geometry`] (pane rectangles,
//! minimums, hit testing), [`commands`] (semantic actions with effects),
//! [`keys`] (the prefix key map) and [`state`] (help, rename prompt, message,
//! connection). [`ui`] composes a frame from them; [`runtime`] performs all
//! IO: one framed connection, one event subscription, one attach stream and
//! one VT per pane.
//!
//! `ht` opens a workspace by reading (or claiming) the shared document,
//! adopting the terminals it names, and applying the per-screen local view.
//! New panes are core terminals started with `keep: true`, `rm: false` and
//! HerdR Terminal labels. Ctrl-] and `prefix q` detach every pane, leaving the
//! programs and the document intact.

pub mod agents;
pub mod commands;
pub mod file;
pub mod geometry;
pub mod keys;
mod local;
pub mod machines;
pub mod manager;
pub mod model;
pub mod palette;
mod runtime;
pub mod sidebar;
mod startup;
pub mod state;
mod sync;
pub mod ui;

use crate::config;
use crate::core::Result;
use crate::session;
use crate::theme::{self, ColorDepth};
use crate::tui::ScreenGuard;
use runtime::{Finish, Screen, Settings};
use std::path::PathBuf;

#[derive(Clone, Debug, Default)]
pub struct Options {
    pub cols: Option<u16>,
    pub rows: Option<u16>,
    pub hc: Option<PathBuf>,
    /// The program for the first pane; empty starts core's shell.
    pub argv: Vec<String>,
    pub verbose: bool,
}

/// The start directory a new local terminal gets, from the process's own
/// inputs. The decision itself is the pure [`model::start_cwd`]; a remote
/// start (`remote`) sends no directory at all.
pub(super) fn local_start_cwd(pane_cwd: Option<&str>, remote: bool) -> Option<String> {
    let client_cwd = std::env::current_dir().ok();
    let home = std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(PathBuf::from)
        .filter(|home| !home.as_os_str().is_empty());
    model::start_cwd(remote, pane_cwd, client_cwd.as_deref(), home.as_deref())
}

/// The focused pane's recorded cwd, when core reported one and the pane is
/// local. A qualified (`machine/t7`) address holds a path on that machine and
/// must never reach a local start.
pub(crate) fn focused_pane_cwd(
    workspace: &model::Workspace,
    model: &crate::model::Model,
) -> Option<String> {
    let pane = workspace.focused_pane()?;
    let address = workspace.pane(pane)?.address.clone();
    if machines::machine(&address).is_some() {
        return None;
    }
    model.terminal(&address).map(|record| record.cwd.clone())
}

/// Opens the workspace and runs it until detach. Returns 0 when the person
/// detaches; programs keep running and the document stays.
pub fn run(options: Options) -> Result<i32> {
    crate::tui::require_tty()?;
    let size = crate::tui::viewport(options.cols, options.rows);

    // Settings are startup-only: the file is read once, before the alternate
    // screen so warnings are plain lines. A bad file never stops the UI.
    let loaded = config::load();
    let config = loaded.config;
    let theme = theme::theme(config.theme, ColorDepth::detect());
    let (keys, key_warnings) = keys::KeyMap::from_config(&config.keys);
    let mut warnings = loaded.warnings;
    warnings.extend(key_warnings);
    for warning in &warnings {
        eprintln!("ht: warning: {warning}");
    }
    let settings = Settings {
        config,
        theme,
        keys,
    };

    // Restore or claim the shared document before the alternate screen, so a
    // failure is a plain error message rather than a broken TUI.
    let viewport = ratatui::layout::Rect::new(0, 0, size.0, size.1);
    let mut opened = startup::open(&options, viewport)?;
    if !warnings.is_empty() {
        let text = warnings.join("; ");
        opened.boot.notice = Some(match opened.boot.notice.take() {
            Some(notice) => format!("{notice}; {text}"),
            None => text,
        });
    }
    let claim = opened.claim.take();
    let mut guard = match ScreenGuard::enter() {
        Ok(guard) => guard,
        Err(error) => {
            // The sessions drop with Boot: owned terminals are ended, adopted
            // ones only detached. A claim this screen created is removed.
            drop(opened.boot);
            if let Some(claim) = claim
                && let Some(warning) = claim.rollback()
            {
                eprintln!("ht: warning: {warning}");
            }
            return Err(error);
        }
    };
    let screen = match Screen::new(size, &options, settings, &mut guard, opened.boot) {
        Ok(screen) => screen,
        Err(error) => {
            if let Some(claim) = claim
                && let Some(warning) = claim.rollback()
            {
                eprintln!("ht: warning: {warning}");
            }
            return Err(error);
        }
    };
    // The document is now owned by the running screen; a normal detach keeps
    // it, and the runtime flushes any last change before returning.
    drop(claim);
    end_sessions(screen.run()).map(|_| 0)
}

/// Puts every pane's terminal back in the state the exit calls for: all
/// detach on a normal exit, all are left alone when core was unreachable,
/// and a fatal local error cleans up only what this client owns.
fn end_sessions(finish: Finish) -> Result<()> {
    let Finish {
        result,
        reconnecting,
        sessions,
    } = finish;
    for mut session in sessions {
        if reconnecting || result.as_ref().err().is_some_and(session::is_lost) {
            // The connection is gone: the programs may still be running, and
            // this process cannot detach or end them.
            session.lost();
        } else if result.is_ok() {
            if let Err(error) = session.detach() {
                eprintln!(
                    "ht: warning: could not detach terminal {}: {error}",
                    session.address()
                );
            }
        } else if let Err(cleanup) = session.cleanup() {
            eprintln!(
                "ht: warning: could not clean up terminal {}: {cleanup}",
                session.address()
            );
        }
    }
    result
}
