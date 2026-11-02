//! The workspace's live loop: one core connection, one event subscription,
//! one attach stream and one VT per pane.
//!
//! The pure layers ([`super::model`], [`super::geometry`],
//! [`super::commands`], [`super::state`], [`super::ui`]) decide what the
//! workspace means. This module only performs IO: it starts terminals, keeps
//! attach streams flowing, sends RESIZE/FOCUS/FACTS/credit, routes input to the
//! focused pane, applies core events, reconnects, and draws frames.
//!
//! The loop is split by seam: [`panes`] owns sizing, attach and per-pane
//! protocol; [`events`] owns streams, events and lifecycle outcomes;
//! [`input`] owns keys, mouse, commands and terminal start/close;
//! [`agents`] owns agent rows, shared seen/pin labels, the sidebar, peek,
//! jumps and alerts; and [`reconnect`] owns a dropped connection. Lifecycle
//! rules are shared with the single-pane screen: a kept terminal whose
//! program was lost waits for `restarted`, and a dropped connection is retried
//! with bounded backoff without starting duplicate terminals.

mod agents;
mod events;
mod input;
mod panes;
mod reconnect;
mod sync;

use agents::AgentRuntime;

use super::geometry;
use super::local::ViewStore;
use super::model::{PaneId, StartRequest, Workspace};
use super::state::UiState;
use super::sync::DocState;
use super::ui;
use crate::core::{CoreClient, Error, EventStream, Result, StreamItem};
use crate::model::Model;
use crate::session::{self, Session};
use crate::tui::{self, ScreenGuard, ScreenTerminal, display, vt_error};
use crate::vt::{Screen as VtScreen, Vt};
use crossterm::cursor::SetCursorStyle;
use crossterm::event::KeyCode;
use crossterm::execute;
use ratatui::layout::Rect;
use std::collections::{BTreeMap, BTreeSet};
use std::io::stdout;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

/// How long the loop waits for input when no output arrived.
const TICK: Duration = Duration::from_millis(10);
/// The most frames per second the workspace redraws at.
const FRAME: Duration = Duration::from_millis(16);
/// The most attach bytes one pane gives up before the loop moves on.
const DRAIN_BYTES: usize = 64 * 1024;
/// The longest one pane's drain may take, for floods of many small chunks.
const DRAIN_TIME: Duration = Duration::from_millis(8);
/// The most event notifications one loop iteration applies before drawing.
const EVENT_BATCH: usize = 256;
/// How long a parked or restarting terminal waits before another attach.
const ATTACH_RETRY: Duration = Duration::from_millis(500);
/// The fallback attach size for a pane with no visible geometry.
const HIDDEN_COLS: u16 = crate::screen::DEFAULT_COLS;
const HIDDEN_ROWS: u16 = crate::screen::DEFAULT_ROWS;

/// Everything needed to enter the loop: the connection, the pure model, the
/// restored workspace and its adopted sessions, and the document/view state.
pub(super) struct Boot {
    pub client: Arc<CoreClient>,
    pub events: EventStream,
    pub model: Model,
    pub workspace: Workspace,
    pub sessions: Vec<(PaneId, Session)>,
    pub doc: DocState,
    pub view: ViewStore,
    pub seen: BTreeSet<PaneId>,
    pub notice: Option<String>,
}

/// How the loop ended, and every session it was managing.
pub(super) struct Finish {
    pub result: Result<()>,
    /// The loop ended while core was unreachable (or trying to reconnect), so
    /// the programs must be left alone rather than detached or cleaned up.
    pub reconnecting: bool,
    pub sessions: Vec<Session>,
}

/// One pane's IO state. The workspace model holds ids and labels; the link
/// holds the live session, stream, VT and per-pane protocol bookkeeping.
struct Link {
    session: Session,
    stream: Option<std::sync::mpsc::Receiver<StreamItem>>,
    vt: Vt,
    vt_size: (u16, u16),
    /// When to try `terminal.attach` again; `None` when not waiting.
    retry_at: Option<Instant>,
    /// Core said the program finished; the CLOSE reason is still to come.
    finished: bool,
    /// The program ended and this pane shows its final screen. The record is
    /// kept (and `terminal.rm` is only sent when the person closes the pane).
    finalized: bool,
    /// Core said the terminal was removed.
    removed: bool,
    needs_facts: bool,
    last_facts: Option<crate::core::Display>,
    focus_sent: Option<bool>,
}

struct Reconnect {
    attempt: u32,
    next: Instant,
}

/// A `terminal.run` whose reply was lost with the connection. The name is the
/// unique one sent with the run, so a terminal that did reach core can be
/// reclaimed instead of duplicated or orphaned.
struct PendingStart {
    request: StartRequest,
    name: String,
}

pub(super) struct Screen<'a> {
    size: (u16, u16),
    hc: Option<PathBuf>,
    verbose: bool,
    client: Arc<CoreClient>,
    events: EventStream,
    model: Model,
    workspace: Workspace,
    links: BTreeMap<PaneId, Link>,
    /// The shared document's base, version and CAS state.
    doc: DocState,
    /// The per-screen focus/zoom file.
    view: ViewStore,
    /// Every pane id this screen has seen; recovered orphans never resurrect
    /// a pane the person closed or another screen removed.
    seen: BTreeSet<PaneId>,
    /// A start interrupted by the connection loss; resolved after reconnect.
    pending: Option<PendingStart>,
    ui: UiState,
    /// Agent rows, seen/pin bookkeeping, sidebar selection and alerts.
    agents: AgentRuntime,
    terminal: ScreenTerminal,
    guard: &'a mut ScreenGuard,
    reconn: Option<Reconnect>,
    dirty: bool,
    last_draw: Instant,
    drew_first: bool,
    last_cursor_style: Option<SetCursorStyle>,
    outer_focused: bool,
    mouse_on: bool,
    /// A key press consumed by the UI; its release must not reach a program.
    swallow: Option<KeyCode>,
    detach: bool,
}

impl<'a> Screen<'a> {
    pub(super) fn new(
        size: (u16, u16),
        options: &super::Options,
        guard: &'a mut ScreenGuard,
        boot: Boot,
    ) -> Result<Screen<'a>> {
        let terminal = tui::terminal()?;
        let Boot {
            client,
            events,
            model,
            workspace,
            sessions,
            doc,
            view,
            seen,
            notice,
        } = boot;
        let mut ui = UiState {
            message: notice,
            ..UiState::default()
        };
        ui.sidebar.open = view.sidebar_open();
        let mut screen = Screen {
            size,
            hc: options.hc.clone(),
            verbose: options.verbose,
            client,
            events,
            model,
            workspace,
            links: BTreeMap::new(),
            doc,
            view,
            seen,
            pending: None,
            ui,
            agents: AgentRuntime::default(),
            terminal,
            guard,
            reconn: None,
            dirty: true,
            last_draw: Instant::now(),
            drew_first: false,
            last_cursor_style: None,
            outer_focused: true,
            mouse_on: false,
            swallow: None,
            detach: false,
        };
        let workspace_panes: Vec<PaneId> = screen.workspace.panes().map(|pane| pane.id).collect();
        screen.seen.extend(workspace_panes);
        for (pane, session) in sessions {
            screen.add_link(pane, session)?;
        }
        Ok(screen)
    }

    /// Runs until detach or a fatal error, and hands every session back so
    /// the caller can detach, leave them alone, or clean up.
    pub(super) fn run(mut self) -> Finish {
        let result = self.play();
        if self.reconn.is_none() {
            if let Some(error) = self.view.save_now(&self.workspace, self.ui.sidebar.open) {
                eprintln!("ht: warning: {error}");
            }
            if let Some(error) = self.flush_document() {
                eprintln!("ht: warning: {error}");
            }
        }
        let reconnecting = self.reconn.is_some();
        let sessions = std::mem::take(&mut self.links)
            .into_values()
            .map(|link| link.session)
            .collect();
        Finish {
            result,
            reconnecting,
            sessions,
        }
    }

    fn play(&mut self) -> Result<()> {
        while !self.detach {
            if self.reconn.is_some() {
                self.reconnect_step()?;
            } else {
                self.live_step()?;
            }
            self.draw_if_due()?;
        }
        Ok(())
    }

    // --- one live iteration ------------------------------------------------

    fn live_step(&mut self) -> Result<()> {
        self.sync_sizes()?;
        if self.reconn.is_some() {
            return Ok(());
        }
        self.resolve_pending()?;
        if self.reconn.is_some() {
            return Ok(());
        }
        self.attach_due()?;
        if self.reconn.is_some() {
            return Ok(());
        }
        self.sync_focus()?;
        if self.reconn.is_some() {
            return Ok(());
        }
        self.sync_mouse()?;
        let had_data = self.drain_panes()?;
        if self.reconn.is_some() {
            return Ok(());
        }
        self.apply_events()?;
        if self.reconn.is_some() {
            return Ok(());
        }
        self.poll_input(had_data)?;
        self.sync_mouse()?;
        self.refresh_agents()?;
        if self.reconn.is_some() {
            return Ok(());
        }
        self.sync_workspace()?;
        Ok(())
    }

    fn area(&self) -> Rect {
        Rect::new(0, 0, self.size.0, self.size.1)
    }

    fn rects(&self) -> geometry::Rects {
        geometry::compute(self.workspace.active_tab(), self.area())
    }

    /// The size a pane's attach stream should use. A pane hidden behind
    /// another tab keeps the size it last had; a new pane has a rectangle
    /// because inserting it made its tab active.
    fn pane_attach_size(&self, pane: PaneId) -> (u16, u16) {
        self.rects()
            .pane(pane)
            .map(geometry::attach_size)
            .or_else(|| self.links.get(&pane).map(|link| link.vt_size))
            .unwrap_or((HIDDEN_COLS, HIDDEN_ROWS))
    }

    // --- drawing -----------------------------------------------------------

    fn draw_if_due(&mut self) -> Result<()> {
        let first = !self.drew_first;
        if !self.dirty && !first {
            return Ok(());
        }
        if !first && self.last_draw.elapsed() < FRAME {
            return Ok(());
        }

        let rects = self.rects();
        let mut screens: BTreeMap<PaneId, VtScreen> = BTreeMap::new();
        for (pane, _) in &rects.panes {
            if let Some(link) = self.links.get_mut(pane) {
                screens.insert(*pane, link.vt.snapshot().map_err(vt_error)?);
            }
        }
        // Peek renders the existing local VT of its target, even when the
        // target lives in another tab; it never reads the screen from core.
        if let Some(pane) = self.ui.peek
            && !screens.contains_key(&pane)
            && let Some(link) = self.links.get_mut(&pane)
        {
            screens.insert(pane, link.vt.snapshot().map_err(vt_error)?);
        }

        // Facts go to every attached pane, including panes in background
        // tabs. A visible pane reuses the snapshot the frame is drawn from.
        let client = self.client.clone();
        let mut lost = false;
        let mut fatal = None;
        for (pane, link) in &mut self.links {
            if !link.needs_facts || lost {
                continue;
            }
            let Some(channel) = link.session.channel() else {
                link.needs_facts = false;
                continue;
            };
            let facts = match screens.get(pane) {
                Some(screen) => display(screen),
                None => match link.vt.snapshot() {
                    Ok(screen) => display(&screen),
                    Err(error) => {
                        fatal = Some(vt_error(error));
                        break;
                    }
                },
            };
            link.needs_facts = false;
            if link.last_facts.as_ref() == Some(&facts) {
                continue;
            }
            match client.facts(channel, &facts) {
                Ok(()) => link.last_facts = Some(facts),
                Err(error) if session::is_lost(&error) => lost = true,
                Err(error) => {
                    fatal = Some(error);
                    break;
                }
            }
        }
        if lost {
            self.begin_reconnect();
        }
        if let Some(error) = fatal {
            return Err(error);
        }

        let ui_state = self.ui.clone();
        let workspace = &self.workspace;
        let visible = &screens;
        let rows = &self.agents.rows;
        // A finished pane has no prompt to type into; its final cursor would
        // suggest otherwise.
        let finished: Vec<PaneId> = self
            .links
            .iter()
            .filter(|(_, link)| link.finalized)
            .map(|(pane, _)| *pane)
            .collect();
        let mut shape: Option<SetCursorStyle> = None;
        self.terminal
            .draw(|frame| {
                let area = frame.area();
                let rendered = ui::render_view(
                    frame.buffer_mut(),
                    area,
                    workspace,
                    &ui_state,
                    visible,
                    rows,
                );
                if let Some((x, y)) = rendered.prompt_cursor {
                    frame.set_cursor_position((x, y));
                    shape = Some(SetCursorStyle::DefaultUserShape);
                } else if let Some((pane, inner)) = rendered.focused
                    && !finished.contains(&pane)
                    && let Some(screen) = visible.get(&pane)
                {
                    let cursor = screen.cursor;
                    if cursor.visible && cursor.x < inner.width && cursor.y < inner.height {
                        frame.set_cursor_position((inner.x + cursor.x, inner.y + cursor.y));
                        shape = Some(crate::screen::view::cursor_shape(&cursor));
                    }
                }
            })
            .map_err(Error::Io)?;
        let shape = shape.unwrap_or(SetCursorStyle::DefaultUserShape);
        if self.last_cursor_style != Some(shape) {
            execute!(stdout(), shape).map_err(Error::Io)?;
            self.last_cursor_style = Some(shape);
        }
        self.drew_first = true;
        self.last_draw = Instant::now();
        self.dirty = false;
        Ok(())
    }
}
