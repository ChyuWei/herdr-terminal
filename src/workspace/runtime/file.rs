//! The file overlay's IO: bounded reads, the external editor, the line diff
//! and the write-back with a content-based conflict check.
//!
//! Every read and write goes through this client's one core connection.
//! Remote paths are never opened locally; only the editor's temp file is
//! local. Nothing here writes the workspace document or a label.

use super::{FileSession, Screen};
use crate::clipboard;
use crate::core::{Error, FileEdit, FileRead, FileWrite, Result};
use crate::session;
use crate::workspace::file::{self, Context, Fetch, Move, Pane, Window};
use crate::workspace::state::{FileState, Line};
use crate::workspace::ui;
use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use std::io::Write;
use std::path::PathBuf;

impl Screen<'_> {
    // --- opening -------------------------------------------------------------

    /// Prefix `o`: open the file overlay for the focused pane's machine and
    /// cwd, or close it when it is already open. Opening closes every other
    /// overlay; the path prompt starts at the pane's cwd.
    pub(super) fn toggle_files(&mut self) {
        if self.ui.files.open {
            self.close_files();
            return;
        }
        self.clear_modes();
        self.ui.help = false;
        self.ui.prompt = None;
        self.ui.palette = None;
        self.ui.search = None;
        self.ui.selecting = None;
        self.ui.peek = None;
        self.ui.sidebar.open = false;
        self.ui.manager.open = false;
        self.ui.machines.open = false;
        if self.preview.is_some() {
            self.close_preview();
        }
        let context = self.file_context();
        self.file = None;
        self.ui.files = FileState {
            open: true,
            prompt: Some(Line::new(&context.prompt())),
            context,
            error: None,
        };
        self.dirty = true;
    }

    pub(super) fn close_files(&mut self) {
        if let Some(session) = self.file.take()
            && let Some(temp) = session.temp
        {
            let _ = std::fs::remove_file(temp);
        }
        if self.ui.files.open || self.ui.files.error.is_some() {
            self.ui.files = FileState::default();
            self.dirty = true;
        }
    }

    /// The focused pane's machine and cwd at the moment the overlay opens.
    fn file_context(&self) -> Context {
        let pane = self
            .workspace
            .focused_pane()
            .and_then(|id| self.workspace.pane(id));
        let address = pane.map(|pane| pane.address.as_str());
        let cwd = pane.and_then(|pane| self.model.terminal(&pane.address));
        Context::from_pane(address, cwd.map(|terminal| terminal.cwd.as_str()))
    }

    // --- keys ----------------------------------------------------------------

    pub(super) fn file_key(&mut self, key: &KeyEvent) -> Result<()> {
        if key.kind == KeyEventKind::Release {
            return Ok(());
        }
        if self.ui.files.prompt.is_some() {
            return self.file_prompt_key(key);
        }
        let has_diff = self
            .file
            .as_ref()
            .is_some_and(|session| session.pane.diff.is_some());
        if has_diff {
            return self.file_diff_key(key);
        }
        self.file_viewer_key(key)
    }

    fn file_prompt_key(&mut self, key: &KeyEvent) -> Result<()> {
        match key.code {
            KeyCode::Esc => {
                self.close_files();
                return Ok(());
            }
            KeyCode::Enter => {
                let path = self
                    .ui
                    .files
                    .prompt
                    .as_ref()
                    .map(|line| line.text().trim().to_owned())
                    .unwrap_or_default();
                if path.is_empty() {
                    self.ui.files.error = Some("a file path is required".into());
                    self.dirty = true;
                    return Ok(());
                }
                if path.ends_with('/') || path.ends_with('\\') {
                    self.ui.files.error = Some(format!("{path} is a directory, not a file"));
                    self.dirty = true;
                    return Ok(());
                }
                let context = self.ui.files.context.clone();
                let request = FileRead::at_line(context.file_at(&path), 1, file::PAGE);
                match self.client.file_read(&request) {
                    Ok(result) => {
                        self.file = Some(FileSession {
                            pane: Pane::viewing(path, context, Window::from_result(result)),
                            original: String::new(),
                            edited: None,
                            temp: None,
                        });
                        self.ui.files.prompt = None;
                        self.ui.files.error = None;
                        self.dirty = true;
                    }
                    Err(error) if session::is_lost(&error) => self.begin_reconnect(),
                    Err(error) => {
                        self.ui.files.error = Some(format!("{}: {error}", context.location(&path)));
                        self.dirty = true;
                    }
                }
                return Ok(());
            }
            _ => {}
        }
        if let Some(edit) = super::overlays::text_edit(key)
            && let Some(line) = self.ui.files.prompt.as_mut()
            && line.edit(edit)
        {
            self.dirty = true;
        }
        Ok(())
    }

    fn file_viewer_key(&mut self, key: &KeyEvent) -> Result<()> {
        let plain = !key.modifiers.contains(KeyModifiers::CONTROL)
            && !key.modifiers.contains(KeyModifiers::ALT);
        let movement = match key.code {
            KeyCode::Esc => {
                self.close_files();
                return Ok(());
            }
            KeyCode::Char('e') if plain => return self.open_editor(),
            KeyCode::Up | KeyCode::Char('k') if plain => Move::Up,
            KeyCode::Down | KeyCode::Char('j') if plain => Move::Down,
            KeyCode::PageUp => Move::PageUp,
            KeyCode::PageDown => Move::PageDown,
            KeyCode::Home | KeyCode::Char('g') if plain => Move::Home,
            KeyCode::End | KeyCode::Char('G') if plain => Move::End,
            _ => return Ok(()),
        };
        self.scroll_viewer(movement)
    }

    fn file_diff_key(&mut self, key: &KeyEvent) -> Result<()> {
        let plain = !key.modifiers.contains(KeyModifiers::CONTROL)
            && !key.modifiers.contains(KeyModifiers::ALT);
        let conflict = self
            .file
            .as_ref()
            .is_some_and(|session| session.pane.conflict);
        match key.code {
            KeyCode::Esc => {
                self.close_files();
                self.ui.message = Some("file changes discarded".into());
                self.dirty = true;
                return Ok(());
            }
            KeyCode::Char('q') if plain => {
                self.close_files();
                self.ui.message = Some("file changes discarded".into());
                self.dirty = true;
                return Ok(());
            }
            KeyCode::Char('w') | KeyCode::Enter if plain => {
                self.write_file(false);
                return Ok(());
            }
            KeyCode::Char('o') if plain && conflict => {
                self.write_file(true);
                return Ok(());
            }
            KeyCode::Up if plain => return self.scroll_diff(Move::Up),
            KeyCode::Down if plain => return self.scroll_diff(Move::Down),
            KeyCode::PageUp => return self.scroll_diff(Move::PageUp),
            KeyCode::PageDown => return self.scroll_diff(Move::PageDown),
            KeyCode::Home => return self.scroll_diff(Move::Home),
            KeyCode::End => return self.scroll_diff(Move::End),
            _ => {}
        }
        Ok(())
    }

    // --- viewer --------------------------------------------------------------

    /// Rows of file text the body can show, after the facts line and any
    /// error line.
    fn file_text_height(&self) -> usize {
        let reserved = usize::from(self.ui.files.error.is_some());
        ui::file_body_height(self.area())
            .saturating_sub(reserved)
            .max(1)
    }

    fn scroll_viewer(&mut self, movement: Move) -> Result<()> {
        let height = self.file_text_height();
        let (path, context, fetch) = {
            let Some(session) = self.file.as_mut() else {
                return Ok(());
            };
            let Some(viewer) = session.pane.viewer.as_mut() else {
                return Ok(());
            };
            let fetch = viewer.scroll(movement, height);
            (
                session.pane.path.clone(),
                session.pane.context.clone(),
                fetch,
            )
        };
        if let Some(fetch) = fetch {
            match self.read_window(&path, &context, fetch) {
                Ok(window) => {
                    if let Some(session) = self.file.as_mut()
                        && let Some(viewer) = session.pane.viewer.as_mut()
                    {
                        viewer.apply(window, movement, height);
                    }
                    self.ui.files.error = None;
                }
                Err(error) if session::is_lost(&error) => {
                    self.begin_reconnect();
                    return Ok(());
                }
                Err(error) => {
                    self.ui.files.error = Some(format!("{}: {error}", context.location(&path)));
                }
            }
        }
        self.dirty = true;
        Ok(())
    }

    fn read_window(&self, path: &str, context: &Context, fetch: Fetch) -> Result<Window> {
        let params = FileRead::at_line(context.file_at(path), fetch.offset, fetch.limit);
        self.client.file_read(&params).map(Window::from_result)
    }

    fn scroll_diff(&mut self, movement: Move) -> Result<()> {
        let height = self.file_text_height();
        if let Some(session) = self.file.as_mut()
            && file::scroll_diff(&mut session.pane, movement, height)
        {
            self.dirty = true;
        }
        Ok(())
    }

    // --- editor --------------------------------------------------------------

    /// Runs `$VISUAL` or `$EDITOR` on a local copy of the file read through
    /// core, then shows the line diff. The TUI leaves the alternate screen and
    /// raw mode for the editor and restores both on return.
    fn open_editor(&mut self) -> Result<()> {
        let Some(session) = self.file.as_ref() else {
            return Ok(());
        };
        let path = session.pane.path.clone();
        let context = session.pane.context.clone();
        let at = context.file_at(&path);
        let full = match self.client.file_read(&FileRead::whole(at)) {
            Ok(result) => result,
            Err(error) if session::is_lost(&error) => {
                self.begin_reconnect();
                return Ok(());
            }
            Err(error) => {
                self.ui.files.error = Some(format!("{}: {error}", context.location(&path)));
                self.dirty = true;
                return Ok(());
            }
        };
        if !file::is_complete(&full) {
            // Core caps an unbounded read; a prefix must never become the
            // editor's buffer or the conflict baseline.
            self.ui.files.error = Some(file::too_large_message(&context, &path, &full));
            self.dirty = true;
            return Ok(());
        }
        let visual = std::env::var("VISUAL").ok();
        let editor = std::env::var("EDITOR").ok();
        let Some(argv) = file::editor(visual.as_deref(), editor.as_deref()) else {
            self.ui.files.error = Some("no editor: set $VISUAL or $EDITOR".into());
            self.dirty = true;
            return Ok(());
        };
        let temp = match self.write_temp(&path, &full.text) {
            Ok(temp) => temp,
            Err(error) => {
                self.ui.files.error = Some(format!("cannot write a temp file: {error}"));
                self.dirty = true;
                return Ok(());
            }
        };
        if let Some(session) = self.file.as_mut() {
            session.temp = Some(temp.clone());
        }

        self.guard.suspend();
        let status = std::process::Command::new(&argv[0])
            .args(&argv[1..])
            .arg(&temp)
            .stdin(std::process::Stdio::inherit())
            .stdout(std::process::Stdio::inherit())
            .stderr(std::process::Stdio::inherit())
            .status();
        let resumed = self.guard.resume();
        // The outer screen was restored for the editor. Re-entering the
        // alternate screen brings back this screen's last frame; a fresh
        // ratatui terminal has no "previous" buffer, so the next frame paints
        // every cell without asking the terminal for a cursor position.
        self.mouse_on = false;
        self.last_cursor_style = None;
        if let Some(session) = self.file.as_mut() {
            session.temp = None;
        }
        self.terminal = crate::tui::terminal()?;
        self.dirty = true;
        if let Err(error) = resumed {
            let _ = std::fs::remove_file(&temp);
            return Err(error);
        }
        let status = match status {
            Ok(status) => status,
            Err(error) => {
                let _ = std::fs::remove_file(&temp);
                self.ui.files.error = Some(format!("could not run {}: {error}", argv[0]));
                return Ok(());
            }
        };
        if !status.success() {
            let _ = std::fs::remove_file(&temp);
            self.ui.files.error = Some(format!(
                "{} exited ({status}); the file was not changed",
                argv[0]
            ));
            return Ok(());
        }
        let edited = match std::fs::read_to_string(&temp) {
            Ok(text) => text,
            Err(error) => {
                let _ = std::fs::remove_file(&temp);
                self.ui.files.error = Some(format!("could not read the edited temp file: {error}"));
                return Ok(());
            }
        };
        let _ = std::fs::remove_file(&temp);
        if edited == full.text {
            self.ui.message = Some(format!("no changes in {}", context.location(&path)));
            self.ui.files.error = None;
            self.dirty = true;
            return Ok(());
        }
        if let Some(session) = self.file.as_mut() {
            session.original = full.text.clone();
            session.edited = Some(edited.clone());
            session.pane.viewer = None;
            session.pane.diff = Some(file::diff(&full.text, &edited));
            session.pane.conflict = false;
            session.pane.diff_top = 0;
        }
        self.ui.files.error = None;
        self.ui.message = None;
        self.dirty = true;
        Ok(())
    }

    /// A unique local temp file with the source name and extension, created
    /// readable only by this user where the platform supports it.
    fn write_temp(&mut self, path: &str, text: &str) -> Result<PathBuf> {
        for _ in 0..100 {
            self.file_serial += 1;
            let name = file::temp_file_name(std::process::id(), self.file_serial, path);
            let temp = std::env::temp_dir().join(name);
            let mut options = std::fs::OpenOptions::new();
            options.write(true).create_new(true);
            #[cfg(unix)]
            {
                use std::os::unix::fs::OpenOptionsExt;
                options.mode(0o600);
            }
            match options.open(&temp) {
                Ok(mut handle) => {
                    if let Err(error) = handle.write_all(text.as_bytes()) {
                        let _ = std::fs::remove_file(&temp);
                        return Err(Error::Io(error));
                    }
                    return Ok(temp);
                }
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
                Err(error) => return Err(Error::Io(error)),
            }
        }
        Err(Error::Protocol(
            "could not create a unique editor temp file".into(),
        ))
    }

    // --- write-back ----------------------------------------------------------

    /// Re-reads the file and compares it with the first full read, then writes
    /// the edited buffer. A changed file is never overwritten without `force`
    /// (the explicit overwrite); the buffer stays on any failure.
    fn write_file(&mut self, force: bool) {
        let Some(session) = self.file.as_ref() else {
            return;
        };
        let Some(edited) = session.edited.clone() else {
            return;
        };
        let path = session.pane.path.clone();
        let context = session.pane.context.clone();
        let original = session.original.clone();
        let at = context.file_at(&path);

        let current = match self.client.file_read(&FileRead::whole(at.clone())) {
            Ok(result) => result,
            Err(error) if session::is_lost(&error) => {
                self.begin_reconnect();
                return;
            }
            Err(error) => {
                self.ui.files.error = Some(format!("{}: {error}", context.location(&path)));
                self.dirty = true;
                return;
            }
        };
        if !file::is_complete(&current) {
            // The re-read is the whole conflict check; a capped prefix cannot
            // prove the file is unchanged, so no comparison and no write,
            // including the explicit `o` overwrite.
            self.ui.files.error = Some(file::too_large_message(&context, &path, &current));
            self.dirty = true;
            return;
        }
        if !force && file::changed_since(&original, &current.text) {
            if let Some(session) = self.file.as_mut() {
                session.pane.conflict = true;
            }
            self.ui.files.error = Some(format!(
                "{} changed since it was read; o overwrite or q discard",
                context.location(&path)
            ));
            self.dirty = true;
            return;
        }

        let result = if !force && let Some(op) = file::single_replacement(&original, &edited) {
            // One exact replacement fits `file.edit`; core still verifies the
            // old text occurs exactly once in the file as it is.
            self.client.file_edit(&FileEdit {
                at,
                edits: vec![op],
            })
        } else {
            self.client.file_write(&FileWrite {
                at,
                data: clipboard::base64(edited.as_bytes()),
            })
        };
        match result {
            Ok(written) => {
                self.close_files();
                self.ui.message = Some(format!("wrote {} ({} bytes)", written.path, written.bytes));
                self.dirty = true;
            }
            Err(error) if session::is_lost(&error) => {
                self.begin_reconnect();
            }
            Err(error) => {
                self.ui.files.error = Some(format!("write {}: {error}", context.location(&path)));
                self.dirty = true;
            }
        }
    }
}
