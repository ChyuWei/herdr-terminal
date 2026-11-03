//! Modal overlays and local clipboard actions: the command palette, the
//! scrollback search bar, selection mode, and paste.
//!
//! These are UI operations only. They never write the shared document, never
//! call `terminal.read`, and never change pane geometry. Search progress is
//! driven in bounded steps from the live loop so a large history cannot block
//! input.

use super::Screen;
use crate::clipboard;
use crate::config::ClipboardMode;
use crate::core::{Error, Result};
use crate::tui::vt_error;
use crate::workspace::palette;
use crate::workspace::state::{Edit, PaletteState, SearchState};
use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use std::io::Write;

impl Screen<'_> {
    // --- opening ---------------------------------------------------------

    pub(super) fn open_palette(&mut self) {
        self.ui.help = false;
        self.ui.prompt = None;
        self.ui.peek = None;
        self.ui.search = None;
        self.ui.selecting = None;
        self.ui.palette = Some(PaletteState::default());
        self.dirty = true;
    }

    pub(super) fn open_search(&mut self) {
        let Some(pane) = self.workspace.focused_pane() else {
            self.ui.message = Some("no pane to search".into());
            self.dirty = true;
            return;
        };
        if !self.links.contains_key(&pane) {
            self.ui.message = Some("the pane is not attached yet".into());
            self.dirty = true;
            return;
        }
        self.ui.help = false;
        self.ui.prompt = None;
        self.ui.peek = None;
        self.ui.palette = None;
        self.ui.selecting = None;
        if let Some(link) = self.links.get_mut(&pane) {
            link.vt.search_clear();
        }
        self.ui.search = Some(SearchState::new(pane));
        self.dirty = true;
    }

    /// Enters selection mode at the focused cursor, or leaves and clears it.
    pub(super) fn toggle_select(&mut self) -> Result<()> {
        if self.ui.selecting.is_some() {
            self.exit_selection(true);
            return Ok(());
        }
        let Some(pane) = self.workspace.focused_pane() else {
            self.ui.message = Some("no pane to select in".into());
            self.dirty = true;
            return Ok(());
        };
        let Some(link) = self.links.get_mut(&pane) else {
            self.ui.message = Some("the pane is not attached yet".into());
            self.dirty = true;
            return Ok(());
        };
        let cursor = link.vt.snapshot().map_err(vt_error)?.cursor;
        let (cols, rows) = link.vt.size();
        let x = cursor.x.min(cols.saturating_sub(1));
        let y = cursor.y.min(rows.saturating_sub(1));
        if !link.vt.start_selection(x, y) {
            self.ui.message = Some("could not begin a selection".into());
            self.dirty = true;
            return Ok(());
        }
        self.ui.selecting = Some(pane);
        self.ui.help = false;
        self.ui.palette = None;
        self.ui.search = None;
        self.ui.message = Some("selection: h/j/k/l extend · y copy · Esc cancel".into());
        self.dirty = true;
        Ok(())
    }

    pub(super) fn exit_selection(&mut self, clear: bool) {
        if clear
            && let Some(pane) = self.ui.selecting
            && let Some(link) = self.links.get_mut(&pane)
        {
            link.vt.clear_selection();
        }
        self.ui.selecting = None;
        if clear {
            self.ui.message = None;
        }
        self.dirty = true;
    }

    // --- keys ------------------------------------------------------------

    pub(super) fn palette_key(&mut self, key: &KeyEvent) -> Result<()> {
        if key.kind == KeyEventKind::Release {
            return Ok(());
        }
        let query = self
            .ui
            .palette
            .as_ref()
            .map(|state| state.line.text())
            .unwrap_or_default();
        let rows = palette::filter(&palette::entries(&self.ui.keymap), &query);
        let plain = !key.modifiers.contains(KeyModifiers::CONTROL)
            && !key.modifiers.contains(KeyModifiers::ALT);
        match key.code {
            KeyCode::Esc => {
                self.ui.palette = None;
                self.dirty = true;
            }
            KeyCode::Enter => {
                let selected = self
                    .ui
                    .palette
                    .as_ref()
                    .map(|state| state.selected)
                    .unwrap_or(0);
                if let Some(entry) = palette::selected(&rows, selected) {
                    self.ui.palette = None;
                    self.dirty = true;
                    self.dispatch(entry.command)?;
                }
            }
            KeyCode::Up if plain => self.move_palette(-1, rows.len()),
            KeyCode::Down if plain => self.move_palette(1, rows.len()),
            _ => {
                if let Some(edit) = text_edit(key)
                    && let Some(state) = self.ui.palette.as_mut()
                    && state.line.edit(edit)
                {
                    state.selected = 0;
                    self.dirty = true;
                }
            }
        }
        Ok(())
    }

    fn move_palette(&mut self, delta: isize, len: usize) {
        if let Some(state) = self.ui.palette.as_mut() {
            state.move_by(delta, len);
            self.dirty = true;
        }
    }

    pub(super) fn search_key(&mut self, key: &KeyEvent) -> Result<()> {
        if key.kind == KeyEventKind::Release {
            return Ok(());
        }
        let plain = !key.modifiers.contains(KeyModifiers::CONTROL)
            && !key.modifiers.contains(KeyModifiers::ALT);
        match key.code {
            KeyCode::Esc => {
                self.close_search();
                return Ok(());
            }
            KeyCode::Enter | KeyCode::Down if plain => {
                self.search_select(true);
                return Ok(());
            }
            KeyCode::Up if plain => {
                self.search_select(false);
                return Ok(());
            }
            _ => {}
        }
        let Some(edit) = text_edit(key) else {
            return Ok(());
        };
        let (pane, query) = {
            let Some(state) = self.ui.search.as_mut() else {
                return Ok(());
            };
            if !state.line.edit(edit) {
                return Ok(());
            }
            (state.pane, state.line.text())
        };
        self.search_apply(pane, &query);
        self.dirty = true;
        Ok(())
    }

    fn close_search(&mut self) {
        if let Some(state) = self.ui.search.take()
            && let Some(link) = self.links.get_mut(&state.pane)
        {
            link.vt.search_clear();
        }
        self.dirty = true;
    }

    fn search_apply(&mut self, pane: crate::workspace::model::PaneId, query: &str) {
        let Some(link) = self.links.get_mut(&pane) else {
            return;
        };
        let progress = link.vt.search_set(query);
        let Some(state) = self.ui.search.as_mut() else {
            return;
        };
        state.total = progress.total;
        state.selected = progress.selected;
        state.complete = progress.complete;
        state.pending = !progress.complete && !query.is_empty();
        if progress.failed {
            state.complete = true;
            self.ui.message = Some("scrollback search failed".into());
        }
    }

    fn search_select(&mut self, next: bool) {
        let Some(state) = self.ui.search.as_ref() else {
            return;
        };
        let pane = state.pane;
        let Some(link) = self.links.get_mut(&pane) else {
            return;
        };
        if !link.vt.search_select(next) {
            if self
                .ui
                .search
                .as_ref()
                .is_some_and(|state| state.total == 0)
            {
                self.ui.message = Some("no matches".into());
                self.dirty = true;
            }
            return;
        }
        let progress = link.vt.search_progress();
        if let Some(state) = self.ui.search.as_mut() {
            state.total = progress.total;
            state.selected = progress.selected;
            state.complete = progress.complete;
        }
        self.ui.message = None;
        self.dirty = true;
    }

    /// One bounded search step per loop iteration. The UI is updated only when
    /// the observable counts change.
    pub(super) fn drive_search(&mut self) {
        let Some(state) = self.ui.search.as_ref() else {
            return;
        };
        let pane = state.pane;
        let Some(link) = self.links.get_mut(&pane) else {
            return;
        };
        let progress = link.vt.search_step(64);
        let Some(state) = self.ui.search.as_mut() else {
            return;
        };
        if progress.failed {
            state.complete = true;
            state.total = 0;
            state.selected = None;
            self.ui.message = Some("scrollback search failed".into());
            self.dirty = true;
            return;
        }
        if state.total != progress.total
            || state.selected != progress.selected
            || state.complete != progress.complete
        {
            state.total = progress.total;
            state.selected = progress.selected;
            state.complete = progress.complete;
            state.pending = !progress.complete;
            self.dirty = true;
        }
    }

    /// One key in selection mode. Returns whether the key was a selection
    /// key; anything else leaves selection mode and is passed on to the
    /// focused program by the caller, so a selection is never a keyboard trap.
    pub(super) fn selection_key(&mut self, key: &KeyEvent) -> Result<bool> {
        if key.kind == KeyEventKind::Release {
            return Ok(true);
        }
        let Some(pane) = self.ui.selecting else {
            return Ok(true);
        };
        let plain = !key.modifiers.contains(KeyModifiers::CONTROL)
            && !key.modifiers.contains(KeyModifiers::ALT);
        let delta = match key.code {
            KeyCode::Esc => {
                self.exit_selection(true);
                return Ok(true);
            }
            KeyCode::Enter => {
                self.copy_selection()?;
                self.exit_selection(false);
                return Ok(true);
            }
            KeyCode::Char('y') if plain => {
                self.copy_selection()?;
                self.exit_selection(false);
                return Ok(true);
            }
            KeyCode::Left | KeyCode::Char('h') if plain => (-1, 0),
            KeyCode::Right | KeyCode::Char('l') if plain => (1, 0),
            KeyCode::Up | KeyCode::Char('k') if plain => (0, -1),
            KeyCode::Down | KeyCode::Char('j') if plain => (0, 1),
            _ => return Ok(false),
        };
        if let Some(link) = self.links.get_mut(&pane)
            && link.vt.move_selection(delta.0, delta.1)
        {
            self.dirty = true;
        }
        Ok(true)
    }

    // --- copy and paste --------------------------------------------------

    /// Copies the current local selection. Empty or missing text never reaches
    /// a clipboard backend.
    pub(super) fn copy_selection(&mut self) -> Result<()> {
        let Some(pane) = self.ui.selecting.or_else(|| self.workspace.focused_pane()) else {
            self.ui.message = Some("nothing selected".into());
            self.dirty = true;
            return Ok(());
        };
        let Some(link) = self.links.get_mut(&pane) else {
            return Ok(());
        };
        let Some(text) = link.vt.selection_text() else {
            self.ui.message = Some("nothing selected".into());
            self.dirty = true;
            return Ok(());
        };
        link.vt.clear_selection();
        self.copy_text(&text)
    }

    fn copy_text(&mut self, text: &str) -> Result<()> {
        if text.is_empty() {
            self.ui.message = Some("nothing selected".into());
            self.dirty = true;
            return Ok(());
        }
        let characters = text.chars().count();
        match self.config.clipboard {
            ClipboardMode::Off => {
                self.ui.message = Some("clipboard is off (config: clipboard)".into());
            }
            ClipboardMode::Osc52 => {
                self.write_osc52(text)?;
                self.ui.message = Some(format!("copied {characters} characters"));
            }
            ClipboardMode::Native => match clipboard::native_copy(text) {
                Ok(()) => {
                    self.ui.message = Some(format!("copied {characters} characters"));
                }
                Err(error) => {
                    self.write_osc52(text)?;
                    self.ui.message = Some(format!(
                        "native clipboard unavailable ({error}); copied with OSC 52"
                    ));
                }
            },
            ClipboardMode::Both => {
                let native = clipboard::native_copy(text);
                self.write_osc52(text)?;
                self.ui.message = Some(match native {
                    Ok(()) => format!("copied {characters} characters (native and OSC 52)"),
                    Err(error) => format!("copied with OSC 52 (native: {error})"),
                });
            }
        }
        self.dirty = true;
        Ok(())
    }

    fn write_osc52(&self, text: &str) -> Result<()> {
        let bytes = clipboard::osc52(text);
        let mut out = std::io::stdout();
        out.write_all(&bytes).map_err(Error::Io)?;
        out.flush().map_err(Error::Io)
    }

    /// Sends pasted text to the focused program. Bracketed paste framing is
    /// used only when that program enabled it (DEC 2004); an overlay, including
    /// an active selection, consumes the paste instead. Selection mode is not a
    /// trap: any key other than the selection keys leaves it and reaches the
    /// program.
    pub(super) fn handle_paste(&mut self, text: String) -> Result<()> {
        if self.overlay_open() || self.ui.prefix {
            return Ok(());
        }
        let Some(pane) = self.workspace.focused_pane() else {
            return Ok(());
        };
        let Some(link) = self.links.get_mut(&pane) else {
            return Ok(());
        };
        link.vt.scroll_to_bottom();
        let bytes = if link.vt.mode(2004) {
            let mut framed = Vec::with_capacity(text.len() + 12);
            framed.extend_from_slice(b"\x1b[200~");
            framed.extend_from_slice(text.as_bytes());
            framed.extend_from_slice(b"\x1b[201~");
            framed
        } else {
            text.into_bytes()
        };
        self.dirty = true;
        self.forward_bytes(&bytes)
    }

    /// Snap the focused pane back to the live bottom. Any ordinary key does
    /// this before it reaches the program.
    pub(super) fn snap_to_bottom(&mut self) {
        let Some(pane) = self.workspace.focused_pane() else {
            return;
        };
        if let Some(link) = self.links.get_mut(&pane)
            && !link.vt.at_bottom()
        {
            link.vt.scroll_to_bottom();
            self.dirty = true;
        }
    }

    /// Keyboard scrolling of the focused pane's history.
    pub(super) fn scroll_key(&mut self, key: &KeyEvent) -> Result<bool> {
        if !key.modifiers.contains(KeyModifiers::SHIFT) {
            return Ok(false);
        }
        let Some(pane) = self.workspace.focused_pane() else {
            return Ok(true);
        };
        let Some(link) = self.links.get_mut(&pane) else {
            return Ok(true);
        };
        let (_, rows) = link.vt.size();
        let page = i64::from(rows.max(1)) / 2;
        let page = page.max(1) as isize;
        match key.code {
            KeyCode::PageUp => link.vt.scroll_lines(-page),
            KeyCode::PageDown => link.vt.scroll_lines(page),
            KeyCode::Home => link.vt.scroll_to_top(),
            KeyCode::End => link.vt.scroll_to_bottom(),
            _ => return Ok(false),
        }
        self.dirty = true;
        Ok(true)
    }

    /// The status-bar hint for the local scroll position. `None` at live
    /// bottom.
    pub(super) fn scroll_hint(&self) -> Option<String> {
        let pane = self.workspace.focused_pane()?;
        let link = self.links.get(&pane)?;
        let bar = link.vt.scrollbar()?;
        if bar.at_bottom() {
            return None;
        }
        let above = bar.above();
        let total = bar.total.saturating_sub(bar.len);
        Some(format!("scroll {above}/{total} · Shift-PgDn live"))
    }
}

/// Editing actions the palette, search and terminal-manager filter share.
/// Commit and Cancel are handled by their owners.
pub(super) fn text_edit(key: &KeyEvent) -> Option<Edit> {
    match key.code {
        KeyCode::Backspace => Some(Edit::Backspace),
        KeyCode::Delete => Some(Edit::Delete),
        KeyCode::Left => Some(Edit::Left),
        KeyCode::Right => Some(Edit::Right),
        KeyCode::Home => Some(Edit::Home),
        KeyCode::End => Some(Edit::End),
        KeyCode::Char(c)
            if !key.modifiers.contains(KeyModifiers::CONTROL)
                && !key.modifiers.contains(KeyModifiers::ALT) =>
        {
            Some(Edit::Insert(c))
        }
        _ => None,
    }
}
