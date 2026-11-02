//! The M3 workspace: spaces, tabs and panes over one core connection.
//!
//! The pure layers are [`model`] (spaces, tabs, layouts, stable pane ids and
//! labels), [`geometry`] (pane rectangles, minimums, hit testing),
//! [`commands`] (semantic actions with effects), [`keys`] (the prefix key
//! map) and [`state`] (help, rename prompt, message, connection). [`ui`]
//! composes a frame from them; [`runtime`] performs all IO: one framed
//! connection, one event subscription, one attach stream and one VT per pane.
//!
//! `ht` opens a workspace with one pane. New panes are core terminals started
//! with `keep: true`, `rm: false` and HerdR Terminal labels. Ctrl-] and
//! `prefix q` detach every pane, leaving the programs running and their
//! records intact; a later workspace does not rediscover them (M4 owns
//! persistence and restore).

pub mod commands;
pub mod geometry;
pub mod keys;
pub mod model;
mod runtime;
pub mod state;
pub mod ui;

use crate::core::{CoreClient, Result, RunOptions};
use crate::model::Model;
use crate::session::{self, Session};
use crate::tui::ScreenGuard;
use model::{StartKind, Workspace};
use runtime::{Boot, Finish, Screen};
use std::path::PathBuf;
use std::sync::Arc;

#[derive(Clone, Debug, Default)]
pub struct Options {
    pub cols: Option<u16>,
    pub rows: Option<u16>,
    pub hc: Option<PathBuf>,
    /// The program for the first pane; empty starts core's shell.
    pub argv: Vec<String>,
    pub verbose: bool,
}

/// Opens the workspace and runs it until detach. Returns 0 when the person
/// detaches; programs keep running.
pub fn run(options: Options) -> Result<i32> {
    crate::tui::require_tty()?;
    let size = crate::tui::viewport(options.cols, options.rows);

    let client = Arc::new(CoreClient::connect(options.hc.as_deref())?);
    let events = client.subscribe()?;
    let model = Model::from_snapshot(events.snapshot());

    // The first pane is started before the alternate screen, so a failure is
    // a plain error message rather than a broken TUI. Its labels reserve the
    // workspace and tab ids that the model uses once the run succeeds.
    let mut workspace = Workspace::new();
    let request = workspace.start_request(StartKind::Initial);
    let run = RunOptions {
        argv: options.argv.clone(),
        name: Some(session::start_name()),
        labels: request.labels(),
        keep: true,
        rm: false,
        ..RunOptions::default()
    };
    let mut session = Session::start(client.clone(), &run)?;
    let address = session.address().to_owned();
    let pane = workspace.insert_terminal(&request, address.clone(), address);

    let mut guard = match ScreenGuard::enter() {
        Ok(guard) => guard,
        Err(error) => {
            let _ = session.cleanup();
            return Err(error);
        }
    };
    let boot = Boot {
        client,
        events,
        model,
        workspace,
        pane,
        session,
    };
    let screen = Screen::new(size, &options, &mut guard, boot)?;
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
