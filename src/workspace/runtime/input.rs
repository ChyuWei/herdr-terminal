//! Local input: the prefix state machine, overlays, forwarding keys and mouse
//! to the focused pane, and the effects of commands (start, close, detach).

use super::{PendingStart, Screen, TICK};
use crate::core::{Error, Result, RunOptions};
use crate::session::{self, Session};
use crate::vt;
use crate::workspace::commands::{Command, Effect, RenameTarget};
use crate::workspace::keys;
use crate::workspace::model::{CloseAction, ClosedPane, StartRequest};
use crate::workspace::state::{Edit, Prompt, PromptEdit};
use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use std::time::Duration;

impl Screen<'_> {
    pub(super) fn poll_input(&mut self, had_data: bool) -> Result<()> {
        let timeout = if had_data { Duration::ZERO } else { TICK };
        if !event::poll(timeout).map_err(Error::Io)? {
            return Ok(());
        }
        let event = event::read().map_err(Error::Io)?;
        match event {
            Event::Key(key) => self.handle_key(key)?,
            Event::Mouse(mouse) => self.handle_mouse(mouse)?,
            Event::Paste(text) => self.handle_paste(text)?,
            Event::Resize(cols, rows) if cols > 0 && rows > 0 => {
                self.size = (cols, rows);
                self.dirty = true;
            }
            Event::FocusGained => {
                self.outer_focused = true;
                self.dirty = true;
            }
            Event::FocusLost => {
                self.outer_focused = false;
                self.dirty = true;
            }
            _ => {}
        }
        Ok(())
    }

    pub(super) fn handle_key(&mut self, key: KeyEvent) -> Result<()> {
        if key.kind == KeyEventKind::Release {
            if self.swallow == Some(key.code) {
                self.swallow = None;
                return Ok(());
            }
            if self.overlay_open() || self.ui.prefix {
                return Ok(());
            }
            return self.forward_key(&key);
        }
        if vt::is_detach(&key) {
            self.detach = true;
            return Ok(());
        }
        // A key ends a click that never became a drag.
        self.selection_press = None;
        if self.ui.peek.is_some() {
            // Peek is modal: any key other than detach closes it, and none of
            // them reaches the program underneath.
            self.close_peek();
            self.swallow = Some(key.code);
            return Ok(());
        }
        if self.ui.help {
            self.ui.help = false;
            self.ui.prefix = false;
            self.swallow = Some(key.code);
            self.dirty = true;
            return Ok(());
        }
        if self.ui.prompt.is_some() {
            self.swallow = Some(key.code);
            return self.edit_prompt(&key);
        }
        if self.ui.palette.is_some() {
            self.swallow = Some(key.code);
            return self.palette_key(&key);
        }
        if self.ui.search.is_some() {
            self.swallow = Some(key.code);
            return self.search_key(&key);
        }
        if self.ui.selecting.is_some() {
            if self.selection_key(&key)? {
                // A selection key is handled locally; its release must not
                // reach the program.
                self.swallow = Some(key.code);
                return Ok(());
            }
            // A selection is not a keyboard trap. Any key that is not a
            // selection key leaves the selection and reaches the program. Its
            // release is forwarded too; the encoder emits one only when the
            // program asked for key events.
            self.exit_selection(true);
            self.snap_to_bottom();
            return self.forward_key(&key);
        }
        if self.ui.sidebar.open {
            self.swallow = Some(key.code);
            return self.sidebar_key(&key);
        }
        if self.ui.prefix {
            self.ui.prefix = false;
            self.swallow = Some(key.code);
            self.dirty = true;
            if key.modifiers.contains(KeyModifiers::CONTROL) {
                if key.code == KeyCode::Char(keys::PREFIX) {
                    // Prefix twice sends the literal prefix byte.
                    self.forward_bytes(&[keys::PREFIX_BYTE])?;
                }
                return Ok(());
            }
            if key.modifiers.contains(KeyModifiers::ALT) {
                return Ok(());
            }
            if let KeyCode::Char(c) = key.code
                && let Some(command) = self.ui.keymap.command(c)
            {
                self.dispatch(command)?;
            }
            return Ok(());
        }
        if self.scroll_key(&key)? {
            self.swallow = Some(key.code);
            return Ok(());
        }
        if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char(keys::PREFIX)
        {
            self.ui.prefix = true;
            self.swallow = Some(key.code);
            self.dirty = true;
            return Ok(());
        }
        // Ordinary input resumes live output: a program must not receive keys
        // while the person is reading history.
        self.snap_to_bottom();
        self.forward_key(&key)
    }

    pub(super) fn overlay_open(&self) -> bool {
        self.ui.modal()
    }

    /// One key while the sidebar is modal. Only its documented keys act;
    /// everything else is consumed without reaching the program.
    fn sidebar_key(&mut self, key: &KeyEvent) -> Result<()> {
        let plain = !key.modifiers.contains(KeyModifiers::CONTROL)
            && !key.modifiers.contains(KeyModifiers::ALT);
        match key.code {
            KeyCode::Esc => self.close_sidebar(),
            KeyCode::Char('a') if plain => self.close_sidebar(),
            KeyCode::Down | KeyCode::Char('j') if plain => self.sidebar_move(1),
            KeyCode::Up | KeyCode::Char('k') if plain => self.sidebar_move(-1),
            KeyCode::Home | KeyCode::Char('g') if plain => self.sidebar_edge(false),
            KeyCode::End | KeyCode::Char('G') if plain => self.sidebar_edge(true),
            KeyCode::Enter => self.sidebar_enter()?,
            KeyCode::Char('i') if plain => self.sidebar_peek()?,
            KeyCode::Char('p') if plain => self.toggle_pin()?,
            KeyCode::Char('u') if plain => self.mark_unread()?,
            _ => {}
        }
        Ok(())
    }

    pub(super) fn edit_prompt(&mut self, key: &KeyEvent) -> Result<()> {
        let edit = match (key.code, key.modifiers) {
            (KeyCode::Enter, _) => Edit::Commit,
            (KeyCode::Esc, _) => Edit::Cancel,
            (KeyCode::Backspace, _) => Edit::Backspace,
            (KeyCode::Delete, _) => Edit::Delete,
            (KeyCode::Left, _) => Edit::Left,
            (KeyCode::Right, _) => Edit::Right,
            (KeyCode::Home, _) => Edit::Home,
            (KeyCode::End, _) => Edit::End,
            (KeyCode::Char(c), modifiers)
                if !modifiers.contains(KeyModifiers::CONTROL)
                    && !modifiers.contains(KeyModifiers::ALT) =>
            {
                Edit::Insert(c)
            }
            _ => return Ok(()),
        };
        let (target, outcome) = {
            let Some(prompt) = self.ui.prompt.as_mut() else {
                return Ok(());
            };
            let target = prompt.target;
            (target, prompt.edit(edit))
        };
        match outcome {
            PromptEdit::Changed => self.dirty = true,
            PromptEdit::Commit(name) => {
                self.ui.prompt = None;
                if !name.is_empty() {
                    let changed = match target {
                        RenameTarget::Tab { space, tab } => {
                            self.workspace.rename_tab(space, tab, &name)
                        }
                        RenameTarget::Space(space) => self.workspace.rename_space(space, &name),
                    };
                    if changed {
                        self.dirty = true;
                    }
                }
            }
            PromptEdit::Cancel => {
                self.ui.prompt = None;
                self.dirty = true;
            }
        }
        Ok(())
    }

    pub(super) fn forward_key(&mut self, key: &KeyEvent) -> Result<()> {
        let prepared = {
            let Some(pane) = self.workspace.focused_pane() else {
                return Ok(());
            };
            self.links.get_mut(&pane).and_then(|link| {
                if link.observed {
                    return None;
                }
                let bytes = link.vt.encode_key(key);
                link.session.channel().map(|channel| (channel, bytes))
            })
        };
        let Some((channel, bytes)) = prepared else {
            return Ok(());
        };
        if bytes.is_empty() {
            return Ok(());
        }
        let client = self.client.clone();
        if let Err(error) = client.keys(channel, &bytes) {
            if session::is_lost(&error) {
                self.begin_reconnect();
                return Ok(());
            }
            return Err(error);
        }
        Ok(())
    }

    pub(super) fn forward_bytes(&mut self, bytes: &[u8]) -> Result<()> {
        let Some(pane) = self.workspace.focused_pane() else {
            return Ok(());
        };
        let Some(channel) = self
            .links
            .get(&pane)
            .filter(|link| !link.observed)
            .and_then(|link| link.session.channel())
        else {
            return Ok(());
        };
        let client = self.client.clone();
        if let Err(error) = client.keys(channel, bytes) {
            if session::is_lost(&error) {
                self.begin_reconnect();
                return Ok(());
            }
            return Err(error);
        }
        Ok(())
    }

    // --- commands ----------------------------------------------------------

    pub(super) fn dispatch(&mut self, command: Command) -> Result<()> {
        let area = self.area();
        let effect = self.workspace.dispatch(command, area);
        match effect {
            Effect::None => {}
            Effect::Changed => self.dirty = true,
            Effect::Start(request) => self.start_terminal(request)?,
            Effect::Close(closed) => self.close_panes(closed),
            Effect::Rename(target) => {
                let current = match target {
                    RenameTarget::Tab { space, tab } => {
                        self.workspace.tab(space, tab).map(|tab| tab.name.clone())
                    }
                    RenameTarget::Space(space) => {
                        self.workspace.space(space).map(|space| space.name.clone())
                    }
                }
                .unwrap_or_default();
                self.ui.prompt = Some(Prompt::new(target, &current));
                self.ui.help = false;
                self.dirty = true;
            }
            Effect::Help => {
                self.ui.help = true;
                self.ui.prompt = None;
                self.ui.prefix = false;
                self.ui.peek = None;
                self.ui.palette = None;
                self.ui.search = None;
                self.ui.selecting = None;
                self.dirty = true;
            }
            Effect::Agents => self.toggle_sidebar(),
            Effect::Peek => self.peek_focused(),
            Effect::JumpAgent(jump) => self.jump_agent(jump),
            Effect::Palette => self.open_palette(),
            Effect::Search => self.open_search(),
            Effect::Select => self.toggle_select()?,
            Effect::Copy => self.copy_selection()?,
            Effect::Detach => self.detach = true,
        }
        Ok(())
    }

    pub(super) fn start_terminal(&mut self, request: StartRequest) -> Result<()> {
        let name = session::start_name();
        self.start_with_name(request, name)
    }

    /// Runs one terminal under a name that is unique among live terminals. If
    /// the reply is lost with the connection, the name is how the reconnect
    /// finds (or safely redoes) the start instead of orphaning a terminal.
    fn start_with_name(&mut self, request: StartRequest, name: String) -> Result<()> {
        let size = self.workspace.pending_attach_size(&request, self.area());
        let run = RunOptions {
            argv: Vec::new(),
            name: Some(name.clone()),
            labels: request.labels(),
            keep: true,
            rm: false,
            size: Some(size),
            ..RunOptions::default()
        };
        let session = match Session::start(self.client.clone(), &run) {
            Ok(session) => session,
            Err(error) if session::is_lost(&error) => {
                // The terminal may exist under this pane id; never let orphan
                // recovery adopt it before the pending start is resolved.
                self.seen.insert(request.pane);
                self.pending = Some(PendingStart { request, name });
                self.begin_reconnect();
                return Ok(());
            }
            Err(error) => {
                self.ui.message = Some(format!("could not start a terminal: {error}"));
                self.dirty = true;
                return Ok(());
            }
        };
        self.finish_start(request, session)
    }

    /// Inserts a successfully started or reclaimed terminal and attaches it.
    fn finish_start(&mut self, request: StartRequest, session: Session) -> Result<()> {
        let address = session.address().to_owned();
        let pane = self
            .workspace
            .insert_terminal(&request, address.clone(), address);
        self.seen.insert(pane);
        if let Err(error) = self.add_link(pane, session) {
            // The session is cleaned up as it drops; do not leave a pane in
            // the model with no link behind it.
            self.workspace.remove_pane(pane);
            return Err(error);
        }
        self.attach_pane(pane)?;
        self.dirty = true;
        Ok(())
    }

    /// Completes a start whose `terminal.run` reply was lost. A terminal with
    /// the unique name in the fresh snapshot was created by that call and is
    /// reclaimed; otherwise the call never reached core and is repeated.
    pub(super) fn resolve_pending(&mut self) -> Result<()> {
        let Some(pending) = self.pending.take() else {
            return Ok(());
        };
        let found = self
            .model
            .terminals()
            .find(|record| record.name.as_deref() == Some(pending.name.as_str()))
            .map(|record| record.address.clone());
        match found {
            Some(address) => {
                let session = Session::reclaim(self.client.clone(), address);
                self.finish_start(pending.request, session)
            }
            None => self.start_with_name(pending.request, pending.name),
        }
    }

    pub(super) fn close_panes(&mut self, closed: Vec<ClosedPane>) {
        for item in closed {
            let Some(mut link) = self.links.remove(&item.pane) else {
                continue;
            };
            let address = link.session.address().to_owned();
            let result = match item.action {
                CloseAction::Detach => link.session.detach(),
                CloseAction::End => link.session.end(),
                CloseAction::Forget => link.session.remove_finished(),
            };
            self.ui.message = Some(match (item.action, result) {
                (_, Err(error)) => format!(
                    "terminal {address}: {error}; it may still be running (hc attach {address})"
                ),
                (CloseAction::Detach, Ok(())) => {
                    format!("left {address} running (hc attach {address})")
                }
                (CloseAction::End, Ok(())) => format!("ended {address}"),
                (CloseAction::Forget, Ok(())) => {
                    format!("closed finished terminal {address}")
                }
            });
            self.dirty = true;
        }
    }

    // --- reconnect ---------------------------------------------------------
}
