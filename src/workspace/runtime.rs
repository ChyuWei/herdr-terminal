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
mod file;
mod input;
mod machines;
mod manager;
mod mouse;
mod overlays;
mod panes;
mod reconnect;
mod resize;
mod sync;

use super::machines::MachineRow;
use agents::AgentRuntime;
use manager::ManagerPreview;

use super::geometry;
use super::local::ViewStore;
use super::manager::ManagerRow;
use super::model::{PaneId, StartRequest, Workspace};
use super::sidebar;
use super::state::UiState;
use super::sync::DocState;
use super::ui;
use crate::config::Config;
use crate::core::{CoreClient, Error, EventStream, Result, StreamItem};
use crate::model::Model;
use crate::session::{self, Session};
use crate::theme::Theme;
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
/// How long a fresh replacement shell must live before its own exit earns
/// another replacement. An instantly exiting shell gets exactly one try.
const REPLACEMENT_GRACE: Duration = Duration::from_secs(2);
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
    /// This stream is an observer's: core sends it RESIZE, and the client
    /// must not send keys, RESIZE, FOCUS or FACTS back.
    observed: bool,
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

/// A split boundary being dragged. The path names the split inside the active
/// tab's layout; `last` is the pointer's row or column on the previous mouse
/// event, so each drag applies only the cells it actually moved.
struct ResizeDrag {
    axis: super::model::Axis,
    path: Vec<usize>,
    index: usize,
    last: u16,
    area: Rect,
}

/// A press on a pane grid that may become a local selection. It stays a plain
/// focus click until the pointer moves to another cell; only then does the
/// selection start at `anchor`.
#[derive(Clone, Copy)]
struct SelectionPress {
    pane: PaneId,
    /// The screen cell where the button went down.
    origin: (u16, u16),
    /// The pane-local cell where the button went down.
    anchor: (u16, u16),
}

/// The local settings resolved at startup: behavior and the resolved theme
/// and key map.
pub(super) struct Settings {
    pub config: Config,
    pub theme: Theme,
    pub keys: super::keys::KeyMap,
}

/// A `terminal.run` whose reply was lost with the connection. The name is the
/// unique one sent with the run, so a terminal that did reach core can be
/// reclaimed instead of duplicated or orphaned.
struct PendingStart {
    request: StartRequest,
    name: String,
    /// The machine the start was for, so a retry never falls back to local.
    on: Option<String>,
    /// The start directory already chosen, kept so a retry starts where the
    /// first attempt would have.
    cwd: Option<String>,
}

/// A fresh shell owed to a tab whose last pane exited: where it goes, the
/// machine the exited pane ran on (so a remote shell stays remote), and the
/// directory it starts in.
pub(super) struct Replacement {
    pub request: StartRequest,
    /// The machine prefix of the exited pane's address, if it was remote.
    pub on: Option<String>,
    /// The exited terminal's recorded cwd, captured before its record was
    /// forgotten.
    pub cwd: Option<String>,
}

/// One open file, its pre-editor text and the local temp file an editor may
/// still be using. Contents live only in memory here: they never enter the
/// workspace document or a label.
struct FileSession {
    pane: super::file::Pane,
    /// The full text first read, for the content-based conflict check.
    original: String,
    /// The editor's buffer, while a diff is pending.
    edited: Option<String>,
    /// The local temp file, until the editor returns and it is removed.
    temp: Option<PathBuf>,
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
    /// Stream-less panes whose `removed` waits one event batch for a
    /// same-rename `created` to move them first.
    deferred_removals: Vec<(PaneId, String)>,
    /// The shared document's base, version and CAS state.
    doc: DocState,
    /// The per-screen focus/zoom file.
    view: ViewStore,
    /// Every pane id this screen has seen; recovered orphans never resurrect
    /// a pane the person closed or another screen removed.
    seen: BTreeSet<PaneId>,
    /// Fresh shells owed to tabs whose last pane exited. They are queued and
    /// started by the loop, never inline while an exit is being handled: a
    /// program that exits the instant it starts must not recurse.
    replacements: Vec<Replacement>,
    /// When this client started each replacement shell, by address. An exit
    /// inside [`REPLACEMENT_GRACE`] means the shell itself is broken, so no
    /// further replacement is started. Entries are dropped when they age out.
    replacement_started: BTreeMap<String, Instant>,
    /// A start interrupted by the connection loss; resolved after reconnect.
    pending: Option<PendingStart>,
    ui: UiState,
    /// Agent rows, seen/pin bookkeeping, sidebar selection and alerts.
    agents: AgentRuntime,
    /// The Core-wide manager's inventory, refreshed once per frame while open.
    manager_rows: Vec<ManagerRow>,
    /// Core's machines, refreshed on open, after each action and on a timer
    /// while the machine manager stays open.
    machine_rows: Vec<MachineRow>,
    /// When the open machine manager should ask core again.
    machine_next_refresh: Option<Instant>,
    /// The read-only observer preview, when one is open.
    preview: Option<ManagerPreview>,
    /// The open file overlay's contents, when a path was read.
    file: Option<FileSession>,
    /// Names this screen's editor temp files uniquely.
    file_serial: u64,
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
    /// The local settings resolved at startup.
    config: Config,
    theme: Theme,
    /// A split boundary drag in progress.
    resize_drag: Option<ResizeDrag>,
    /// The pane whose local selection a mouse drag is extending.
    selection_drag: Option<PaneId>,
    /// A grid press that becomes a selection only once the pointer moves.
    selection_press: Option<SelectionPress>,
    /// A narrow drawer was dismissed by an outside press; the rest of that
    /// gesture (drag/release/wheel) must not reach the pane underneath.
    drawer_gesture: bool,
}

impl<'a> Screen<'a> {
    pub(super) fn new(
        size: (u16, u16),
        options: &super::Options,
        settings: Settings,
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
            keymap: settings.keys.clone(),
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
            deferred_removals: Vec::new(),
            doc,
            view,
            seen,
            pending: None,
            replacements: Vec::new(),
            replacement_started: BTreeMap::new(),
            ui,
            agents: AgentRuntime::default(),
            manager_rows: Vec::new(),
            machine_rows: Vec::new(),
            machine_next_refresh: None,
            preview: None,
            file: None,
            file_serial: 0,
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
            config: settings.config,
            theme: settings.theme,
            resize_drag: None,
            selection_drag: None,
            selection_press: None,
            drawer_gesture: false,
        };
        let workspace_panes: Vec<PaneId> = screen.workspace.panes().map(|pane| pane.id).collect();
        screen.seen.extend(workspace_panes);
        // A saved focus from a wide terminal must not survive on a narrow
        // one: the panel is width-driven, so there would be nothing to draw.
        screen.release_hidden_sidebar();
        for (pane, session) in sessions {
            screen.add_link(pane, session)?;
        }
        Ok(screen)
    }

    /// Runs until detach or a fatal error, and hands every session back so
    /// the caller can detach, leave them alone, or clean up.
    pub(super) fn run(mut self) -> Finish {
        let result = self.play();
        // The preview is not a workspace session: close its observer stream
        // before the sessions are handed back.
        if self.reconn.is_none() {
            self.close_preview();
        } else {
            self.drop_preview();
        }
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
        let preview_data = self.drain_preview()?;
        if self.reconn.is_some() {
            return Ok(());
        }
        self.apply_events()?;
        if self.reconn.is_some() {
            return Ok(());
        }
        self.run_replacements()?;
        if self.reconn.is_some() {
            return Ok(());
        }
        self.refresh_machines_if_due();
        self.poll_input(had_data || preview_data)?;
        self.drive_search();
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

    /// Releases sidebar keyboard focus when the panel no longer fits. The
    /// panel is visible by width, not by focus: hiding it must also stop it
    /// covering the panes and swallowing keys, and a later widening must not
    /// silently restore a focus the person can no longer see.
    pub(super) fn release_hidden_sidebar(&mut self) {
        if self.ui.sidebar.open && sidebar::width(self.area()) == 0 {
            self.close_sidebar();
        }
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

    /// The bottom line's focused identity: the address, then the app and cwd
    /// core reported, then the pane's own name/title when nothing was
    /// reported. The active space and tab are never repeated here.
    fn identity_line(&self) -> Option<String> {
        let id = self.workspace.focused_pane()?;
        let pane = self.workspace.pane(id)?;
        let record = self.model.terminal(&pane.address);
        let mut parts = vec![pane.address.clone()];
        if let Some(record) = record {
            let app = record
                .status
                .as_ref()
                .map(|status| status.app.as_str())
                .filter(|app| !app.is_empty())
                .or_else(|| {
                    record
                        .foreground
                        .as_ref()
                        .map(|foreground| foreground.name.as_str())
                        .filter(|name| !name.is_empty())
                });
            if let Some(app) = app {
                parts.push(app.to_owned());
            }
            let cwd = record
                .foreground
                .as_ref()
                .map(|foreground| foreground.cwd.as_str())
                .filter(|cwd| !cwd.is_empty())
                .or_else(|| (!record.cwd.is_empty()).then_some(record.cwd.as_str()));
            if let Some(cwd) = cwd {
                parts.push(cwd.to_owned());
            }
        }
        if parts.len() == 1 {
            let name = if pane.title.is_empty() || pane.title == pane.name {
                pane.name.clone()
            } else {
                format!("{}: {}", pane.name, pane.title)
            };
            parts.push(name);
        }
        Some(parts.join(" · "))
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

        self.refresh_manager();
        let preview_screen = match self.preview.as_mut() {
            Some(preview) => Some(preview.vt.snapshot().map_err(vt_error)?),
            None => None,
        };
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
            if link.observed {
                // An observer's colors come from the viewer that owns the
                // size; sending FACTS would take display ownership.
                link.needs_facts = false;
                continue;
            }
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

        // Local selections follow their cells; only visible panes need the
        // highlight rectangles this frame.
        let mut selections: BTreeMap<PaneId, Vec<(u16, u16, u16)>> = BTreeMap::new();
        for pane in screens.keys() {
            if let Some(link) = self.links.get_mut(pane) {
                let spans = link.vt.selection_rects();
                if !spans.is_empty() {
                    selections.insert(*pane, spans);
                }
            }
        }

        let ui_state = self.ui.clone();
        let workspace = &self.workspace;
        let visible = &screens;
        let rows = &self.agents.rows;
        let manager_rows = &self.manager_rows;
        let machine_rows = &self.machine_rows;
        let theme = &self.theme;
        let scroll_hint = self.scroll_hint();
        let file_pane = self.file.as_ref().map(|session| &session.pane);
        // A finished pane has no prompt to type into; its final cursor would
        // suggest otherwise.
        let finished: Vec<PaneId> = self
            .links
            .iter()
            .filter(|(_, link)| link.finalized)
            .map(|(pane, _)| *pane)
            .collect();
        let live_cursor = self
            .workspace
            .focused_pane()
            .and_then(|pane| self.links.get(&pane))
            .is_none_or(|link| link.vt.at_bottom());
        let identity = self.identity_line();
        let mut shape: Option<SetCursorStyle> = None;
        self.terminal
            .draw(|frame| {
                let area = frame.area();
                let view = ui::View {
                    screens: visible,
                    rows,
                    theme,
                    selections: &selections,
                    scroll_hint: scroll_hint.as_deref(),
                    manager: manager_rows,
                    machines: machine_rows,
                    preview: preview_screen.as_ref(),
                    files: file_pane,
                    identity: identity.as_deref(),
                };
                let rendered =
                    ui::render_with(frame.buffer_mut(), area, workspace, &ui_state, &view);
                if let Some((x, y)) = rendered.prompt_cursor {
                    frame.set_cursor_position((x, y));
                    shape = Some(SetCursorStyle::DefaultUserShape);
                } else if let Some((pane, inner)) = rendered.focused
                    && !finished.contains(&pane)
                    && live_cursor
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
