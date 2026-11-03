//! The Core-wide terminal manager's IO: the inventory rows, the modal keys,
//! Show/Hide (adopt or drop a pane, never run or kill), and the read-only
//! observer preview.
//!
//! The decisions live in [`crate::workspace::manager`]. This file only calls
//! core: `terminal.label` to release this client's identity labels when a
//! terminal is hidden, `terminal.attach` with `observe` for the preview, and
//! the normal session attach path when a shown terminal gets a pane.

use super::{DRAIN_BYTES, DRAIN_TIME, Screen};
use crate::core::{Result, StreamItem};
use crate::session::{self, Session};
use crate::tui::vt_error;
use crate::vt::Vt;
use crate::workspace::agents;
use crate::workspace::manager::{self, ManagerRow};
use crate::workspace::model::AdoptedTerminal;
use crate::workspace::state::PreviewState;
use crate::workspace::ui;
use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use std::sync::mpsc::{Receiver, TryRecvError};
use std::time::{Instant, SystemTime, UNIX_EPOCH};

/// The observer preview's stream and local VT. It is deliberately not a
/// [`super::Link`]: it has no pane, never sends input, RESIZE, FOCUS or FACTS,
/// and never takes the terminal's size.
pub(super) struct ManagerPreview {
    pub(super) address: String,
    pub(super) session: Session,
    pub(super) stream: Receiver<StreamItem>,
    pub(super) vt: Vt,
    pub(super) vt_size: (u16, u16),
}

impl Screen<'_> {
    // --- opening and rows -------------------------------------------------

    /// Prefix `t` (or the palette): open the manager, or close it if it is
    /// already the modal overlay. Opening closes every other overlay. (`m`
    /// stays reserved for the machine manager.)
    pub(super) fn toggle_manager(&mut self) {
        if self.ui.manager.open {
            self.close_manager();
            return;
        }
        let focused = self
            .workspace
            .focused_pane()
            .and_then(|pane| self.workspace.pane(pane))
            .map(|pane| pane.address.clone());
        self.ui.help = false;
        self.ui.prompt = None;
        self.ui.palette = None;
        self.ui.search = None;
        self.ui.selecting = None;
        self.ui.peek = None;
        self.ui.sidebar.open = false;
        self.ui.machines.open = false;
        self.ui.machines.prompt = None;
        self.ui.machines.confirm = None;
        self.ui.manager.open = true;
        self.ui.manager.filtering = false;
        self.ui.manager.selected = focused;
        self.refresh_manager();
        self.dirty = true;
    }

    pub(super) fn close_manager(&mut self) {
        if self.ui.manager.open {
            self.ui.manager.open = false;
            self.ui.manager.filtering = false;
            self.dirty = true;
        }
    }

    /// Recomputes the inventory from the event model and repairs the local
    /// selection and scroll offset. Runs once per frame, so a created,
    /// removed, finished, status or labels change is visible immediately.
    pub(super) fn refresh_manager(&mut self) {
        if !self.ui.manager.open {
            self.manager_rows.clear();
            return;
        }
        self.manager_rows = manager::rows(&self.model, &self.workspace, now_ms());
        let query = self.ui.manager.filter.text();
        let filtered = manager::filter(&self.manager_rows, &query);
        let selected = manager::ensure_selection(&filtered, self.ui.manager.selected.as_deref());
        let index = selected
            .as_deref()
            .and_then(|address| filtered.iter().position(|row| row.address == address));
        let height = ui::manager_list_height(self.area());
        self.ui.manager.offset =
            agents::scroll_to(self.ui.manager.offset, index, height, filtered.len());
        self.ui.manager.selected = selected;
    }

    // --- keys --------------------------------------------------------------

    /// One key while the manager owns the keyboard. Nothing here is forwarded
    /// to a program; every key is consumed.
    pub(super) fn manager_key(&mut self, key: &KeyEvent) -> Result<()> {
        if key.kind == KeyEventKind::Release {
            return Ok(());
        }
        if self.ui.manager.filtering {
            match key.code {
                KeyCode::Esc => {
                    self.close_manager();
                    return Ok(());
                }
                KeyCode::Enter => {
                    self.ui.manager.filtering = false;
                    self.dirty = true;
                    return Ok(());
                }
                _ => {
                    if let Some(edit) = super::overlays::text_edit(key)
                        && self.ui.manager.filter.edit(edit)
                    {
                        self.refresh_manager();
                        self.dirty = true;
                    }
                    return Ok(());
                }
            }
        }
        let plain = !key.modifiers.contains(KeyModifiers::CONTROL)
            && !key.modifiers.contains(KeyModifiers::ALT);
        match key.code {
            KeyCode::Esc => self.close_manager(),
            KeyCode::Down | KeyCode::Char('j') if plain => self.manager_move(1),
            KeyCode::Up | KeyCode::Char('k') if plain => self.manager_move(-1),
            KeyCode::Home | KeyCode::Char('g') if plain => self.manager_edge(false),
            KeyCode::End | KeyCode::Char('G') if plain => self.manager_edge(true),
            KeyCode::Enter => self.manager_toggle()?,
            KeyCode::Char('p') if plain => self.preview_selected()?,
            KeyCode::Char('/') if plain => {
                self.ui.manager.filtering = true;
                self.dirty = true;
            }
            _ => {}
        }
        Ok(())
    }

    fn manager_move(&mut self, delta: isize) {
        let height = ui::manager_list_height(self.area());
        let query = self.ui.manager.filter.text();
        let filtered = manager::filter(&self.manager_rows, &query);
        let next = manager::move_selection(&filtered, self.ui.manager.selected.as_deref(), delta);
        let index = next
            .as_deref()
            .and_then(|address| filtered.iter().position(|row| row.address == address));
        self.ui.manager.offset =
            agents::scroll_to(self.ui.manager.offset, index, height, filtered.len());
        self.ui.manager.selected = next;
        self.dirty = true;
    }

    fn manager_edge(&mut self, last: bool) {
        let height = ui::manager_list_height(self.area());
        let query = self.ui.manager.filter.text();
        let filtered = manager::filter(&self.manager_rows, &query);
        let next = manager::edge_selection(&filtered, last);
        let index = next
            .as_deref()
            .and_then(|address| filtered.iter().position(|row| row.address == address));
        self.ui.manager.offset =
            agents::scroll_to(self.ui.manager.offset, index, height, filtered.len());
        self.ui.manager.selected = next;
        self.dirty = true;
    }

    fn selected_manager_row(&self) -> Option<ManagerRow> {
        let query = self.ui.manager.filter.text();
        let filtered = manager::filter(&self.manager_rows, &query);
        manager::selected_row(&filtered, self.ui.manager.selected.as_deref()).cloned()
    }

    /// Enter: show a hidden terminal (adopt it, never run it), or hide a
    /// terminal that is already in this workspace (detach, never kill). Either
    /// way the manager closes so the result is visible.
    fn manager_toggle(&mut self) -> Result<()> {
        let Some(row) = self.selected_manager_row() else {
            self.ui.message = Some("no terminal selected".into());
            self.dirty = true;
            return Ok(());
        };
        if row.shown {
            self.hide_address(&row.address)?;
            if self.reconn.is_none() {
                self.close_manager();
            }
            return Ok(());
        }
        if self.show_address(&row.address)? {
            self.close_manager();
        }
        Ok(())
    }

    // --- show / hide -------------------------------------------------------

    /// Shows an existing core terminal in the current tab. No `terminal.run`
    /// is sent: the pane references the address and attaches to the program
    /// that is already running. Returns whether the terminal is now shown.
    pub(super) fn show_address(&mut self, address: &str) -> Result<bool> {
        if let Some(pane) = self.workspace.pane_id_by_address(address) {
            self.workspace.jump_to(pane);
            self.dirty = true;
            return Ok(true);
        }
        let Some(record) = self.model.terminal(address).cloned() else {
            self.ui.message = Some(format!("terminal {address} is no longer in core"));
            self.dirty = true;
            return Ok(false);
        };
        if !record.is_live() {
            self.ui.message = Some(format!("terminal {address} is not live"));
            self.dirty = true;
            return Ok(false);
        }
        let request = self.workspace.adopt_request();
        let adopted = AdoptedTerminal {
            address: address.to_owned(),
            name: manager::display_name(&record),
            labels: record.labels.clone(),
            title: record.title.clone(),
            status: record
                .status
                .as_ref()
                .map(|status| status.state.clone())
                .filter(|state| !state.is_empty()),
            keep: record.keep,
        };
        let Some(pane) = self.workspace.insert_adopted(&request, adopted) else {
            self.ui.message = Some(format!("{address} is already shown"));
            self.dirty = true;
            return Ok(false);
        };
        self.seen.insert(pane);
        // The terminal's labels may still name the pane it had in another
        // workspace state. This screen has now witnessed that pane id, so a
        // later merge that drops the new pane must not recover the terminal
        // under the stale label.
        if let Some(stale) = manager::stale_pane_label(&record.labels, pane) {
            self.seen.insert(stale);
        }
        let session = Session::adopt(self.client.clone(), address.to_owned());
        if let Err(error) = self.add_link(pane, session) {
            self.workspace.remove_pane(pane);
            return Err(error);
        }
        self.attach_pane(pane)?;
        self.ui.message = Some(format!("showing {address}"));
        self.dirty = true;
        Ok(true)
    }

    /// Hides the pane that shows `address`: the attach stream is detached and
    /// the pane reference leaves the workspace. No `terminal.kill` or
    /// `terminal.rm` is ever sent, so the program keeps running. Only identity
    /// labels this screen itself wrote are cleared; another client's or
    /// another ht screen's labels are never touched.
    pub(super) fn hide_address(&mut self, address: &str) -> Result<()> {
        let Some(pane) = self.workspace.pane_id_by_address(address) else {
            return Ok(());
        };
        let pane_labels = self
            .workspace
            .pane(pane)
            .map(|pane| pane.labels.clone())
            .unwrap_or_default();
        if let Some(mut link) = self.links.remove(&pane)
            && let Err(error) = link.session.detach()
            && !session::is_lost(&error)
        {
            self.ui.message = Some(format!(
                "could not detach {address}: {error}; it keeps running"
            ));
        }
        self.workspace.remove_pane(pane);
        let namespace = self.workspace.allocator().namespace();
        self.clear_identity_labels(address, namespace, pane_labels)?;
        if self.reconn.is_none() {
            self.ui.message = Some(format!(
                "hid {address}; it keeps running (Enter in the manager shows it again)"
            ));
        }
        self.dirty = true;
        Ok(())
    }

    /// Clears the four labels this screen wrote, and only when the terminal
    /// carries this client's `herdr.client` value and its pane label's
    /// namespace is this screen's (the high 32 bits of the hidden workspace's
    /// ids). Without that, startup orphan recovery could resurrect a hidden
    /// terminal; with another screen's labels, clearing would break that
    /// screen's workspace. The write is optimistic and core's reply is
    /// authoritative; a failure is reported, not fatal.
    fn clear_identity_labels(
        &mut self,
        address: &str,
        namespace: u64,
        pane_labels: std::collections::BTreeMap<String, String>,
    ) -> Result<()> {
        let labels = self
            .model
            .terminal(address)
            .map(|record| record.labels.clone())
            .unwrap_or(pane_labels);
        let Some(removal) = manager::release_labels(&labels, namespace) else {
            return Ok(());
        };
        let next: std::collections::BTreeMap<String, String> = labels
            .into_iter()
            .filter(|(key, _)| !removal.contains_key(key))
            .collect();
        self.model.set_labels(address, next);
        match self.client.label(address, &removal) {
            Ok(record) => self.model.set_labels(address, record.labels),
            Err(error) if session::is_lost(&error) => self.begin_reconnect(),
            Err(error) => {
                self.ui.message = Some(format!(
                    "hid {address}; its own labels could not be cleared: {error} \
                     (a restart may show it again)"
                ));
                self.dirty = true;
            }
        }
        Ok(())
    }

    // --- preview -----------------------------------------------------------

    fn preview_selected(&mut self) -> Result<()> {
        let Some(row) = self.selected_manager_row() else {
            self.ui.message = Some("no terminal selected".into());
            self.dirty = true;
            return Ok(());
        };
        self.open_preview(&row.address)
    }

    /// Opens a read-only observer stream for `address`, rendered through a
    /// local VT. The manager stays underneath; any key closes the preview and
    /// returns to it. No pane, document or workspace change is made.
    pub(super) fn open_preview(&mut self, address: &str) -> Result<()> {
        let Some(record) = self.model.terminal(address).cloned() else {
            self.ui.message = Some(format!("terminal {address} is no longer in core"));
            self.dirty = true;
            return Ok(());
        };
        if !record.is_live() {
            self.ui.message = Some(format!("terminal {address} is not live"));
            self.dirty = true;
            return Ok(());
        }
        let (cols, rows) = ui::preview_grid_size(self.area());
        if cols == 0 || rows == 0 {
            self.ui.message = Some("the terminal is too small to preview".into());
            self.dirty = true;
            return Ok(());
        }
        self.close_preview();
        let mut session = Session::adopt(self.client.clone(), address.to_owned());
        let attached = match session.attach_observer(cols, rows, self.config.scrollback) {
            Ok(attached) => attached,
            Err(error) if session::is_lost(&error) => {
                self.begin_reconnect();
                return Ok(());
            }
            Err(error) => {
                self.ui.message = Some(format!("could not preview {address}: {error}"));
                self.dirty = true;
                return Ok(());
            }
        };
        let vt = Vt::new_with_scrollback(cols, rows, self.config.scrollback).map_err(vt_error)?;
        let title = if record.title.is_empty() {
            manager::display_name(&record)
        } else {
            record.title
        };
        self.preview = Some(ManagerPreview {
            address: address.to_owned(),
            session,
            stream: attached.stream,
            vt,
            vt_size: (cols, rows),
        });
        self.ui.preview = Some(PreviewState {
            address: address.to_owned(),
            title,
        });
        self.ui.manager.filtering = false;
        self.dirty = true;
        Ok(())
    }

    /// Closes the preview stream with a normal detach. Never sends input,
    /// RESIZE, FOCUS or FACTS.
    pub(super) fn close_preview(&mut self) {
        if let Some(mut preview) = self.preview.take()
            && let Err(error) = preview.session.detach()
            && !session::is_lost(&error)
        {
            self.ui.message = Some(format!("could not close the preview: {error}"));
        }
        if self.ui.preview.take().is_some() {
            self.dirty = true;
        }
    }

    /// Drops the preview after the connection is gone: no detach is possible.
    pub(super) fn drop_preview(&mut self) {
        if let Some(mut preview) = self.preview.take() {
            preview.session.lost();
        }
        self.ui.preview = None;
    }

    /// Drains the observer stream: RESIZE resizes the local VT and is applied
    /// before the DATA that follows it; DATA feeds the VT and pays credit.
    /// Nothing is ever written back except credit and the final CLOSE.
    pub(super) fn drain_preview(&mut self) -> Result<bool> {
        let Some(preview) = self.preview.as_mut() else {
            return Ok(false);
        };
        let client = self.client.clone();
        let start = Instant::now();
        let mut drained = 0usize;
        let mut had_data = false;
        let mut closed = None;
        let mut lost = false;
        loop {
            match preview.stream.try_recv() {
                Ok(StreamItem::Data(bytes)) => {
                    had_data = true;
                    self.dirty = true;
                    drained += bytes.len();
                    preview.vt.feed(&bytes);
                    if let Some(channel) = preview.session.channel()
                        && let Err(error) = client.credit(channel, bytes.len())
                    {
                        if session::is_lost(&error) {
                            lost = true;
                            break;
                        }
                        return Err(error);
                    }
                    if drained >= DRAIN_BYTES || start.elapsed() >= DRAIN_TIME {
                        break;
                    }
                }
                Ok(StreamItem::Resize { cols, rows }) => {
                    if cols > 0 && rows > 0 {
                        preview.vt.resize(cols, rows).map_err(vt_error)?;
                        preview.vt_size = (cols, rows);
                        self.dirty = true;
                    }
                }
                Ok(StreamItem::Closed(reason)) => {
                    closed = Some(reason);
                    break;
                }
                Ok(StreamItem::Lost) | Err(TryRecvError::Disconnected) => {
                    lost = true;
                    break;
                }
                Err(TryRecvError::Empty) => break,
            }
        }
        if lost {
            self.begin_reconnect();
            return Ok(had_data);
        }
        if let Some(reason) = closed {
            let address = self
                .preview
                .as_ref()
                .map(|preview| preview.address.clone())
                .unwrap_or_default();
            self.close_preview();
            self.ui.message = Some(match &reason {
                Some(reason) => format!(
                    "preview of {address} ended ({})",
                    session::exit_code_from(Some(reason))
                ),
                None => format!("preview of {address} ended"),
            });
            self.dirty = true;
        }
        Ok(had_data)
    }

    /// A finished or removed event for the previewed terminal: close the
    /// observer stream and say so. The manager underneath stays open.
    pub(super) fn preview_gone(&mut self, address: &str) {
        if self
            .ui
            .preview
            .as_ref()
            .is_none_or(|preview| preview.address != address)
        {
            return;
        }
        self.close_preview();
        self.ui.message = Some(format!("preview of {address} ended"));
        self.dirty = true;
    }

    /// After a fresh snapshot or `missed`: a preview whose terminal is no
    /// longer live has no stream left to show.
    pub(super) fn check_preview(&mut self) {
        let Some(address) = self
            .ui
            .preview
            .as_ref()
            .map(|preview| preview.address.clone())
        else {
            return;
        };
        let live = self
            .model
            .terminal(&address)
            .is_some_and(|record| record.is_live());
        if !live {
            self.preview_gone(&address);
        }
    }
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| elapsed.as_millis() as u64)
        .unwrap_or(0)
}
